use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dcc_cua_browser::BrowserSession;
use dcc_cua_core::{
    ActionEvidenceEpoch, ComputerUseDesktopSession, ComputerUseErrorCode,
    ComputerUseInputReadiness, ComputerUseInputStatus, ComputerUseResult, ComputerUseSession,
    ComputerUseTargetAvailability, ComputerUseTargetStatus,
};
use dcc_cua_shm::SharedImage;
use serde_json::Value;

use crate::session_events::SessionInputEventQueue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HostEvidencePublication {
    None,
    BrowserSnapshot,
    BrowserSnapshotAttempt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HostEvidenceEpochSyncPlan {
    pub(super) epoch_changed: bool,
    pub(super) invalidate_browser_snapshot: bool,
    pub(super) bind_browser_snapshot: bool,
}

pub(super) fn evidence_epoch_sync_plan(
    synchronized: ActionEvidenceEpoch,
    current: ActionEvidenceEpoch,
    publication: HostEvidencePublication,
) -> HostEvidenceEpochSyncPlan {
    let epoch_changed = synchronized != current;
    HostEvidenceEpochSyncPlan {
        epoch_changed,
        invalidate_browser_snapshot: epoch_changed
            && publication != HostEvidencePublication::BrowserSnapshot,
        bind_browser_snapshot: publication == HostEvidencePublication::BrowserSnapshot,
    }
}

pub(super) struct HostSession {
    pub(super) runtime_session_id: String,
    pub(super) target_process_id: u32,
    pub(super) target_window_handle: u64,
    pub(super) task_grant_id: String,
    pub(super) observation_mode: crate::TaskObservationMode,
    pub(super) allow_raw_input: bool,
    pub(super) allow_app_terminate: bool,
    pub(super) allow_clipboard_read: bool,
    pub(super) allow_clipboard_write: bool,
    pub(super) allow_recording: bool,
    pub(super) allow_live_observation: bool,
    pub(super) allow_browser_input: bool,
    pub(super) allow_browser_prepare: bool,
    pub(super) allowed_browser_origins: Vec<String>,
    pub(super) allow_browser_download: bool,
    pub(super) allow_native_tool: bool,
    pub(super) allow_menu_invoke: bool,
    pub(super) allow_session_escalation: bool,
    pub(super) allow_trusted_confirmation: bool,
    pub(super) task_authorization: Option<crate::TrustedTaskAuthorizationLease>,
    pub(super) task_authorization_host: Option<Arc<dyn crate::TrustedTaskAuthorizationHost>>,
    pub(super) allow_restore_activate: bool,
    pub(super) capability: String,
    pub(super) interrupted: bool,
    pub(super) session: ComputerUseSession,
    pub(super) synchronized_action_evidence_epoch: ActionEvidenceEpoch,
    pub(super) browser_evidence_epoch: Option<ActionEvidenceEpoch>,
    pub(super) browser: BrowserSession,
    pub(super) latest_observation_id: Option<String>,
    pub(super) latest_accessibility_state_id: Option<String>,
    pub(super) latest_accessibility_root: Option<Value>,
    pub(super) latest_shared_image: Option<SharedImage>,
    /// Passive prepared pixels are never bound to actionable observations.
    pub(super) latest_preparation_image: Option<SharedImage>,
    pub(super) input_events: SessionInputEventQueue,
    pub(super) idle_timeout: Duration,
    pub(super) last_activity: Instant,
}

pub(super) fn task_browser_session(
    lease: &Option<crate::TrustedTaskAuthorizationLease>,
) -> Result<BrowserSession, crate::HostError> {
    let mut browser = BrowserSession::default();
    if let Some(scope) = lease
        .as_ref()
        .and_then(|lease| lease.browser_scope.as_ref())
    {
        browser.bind_authorized_exact_target(&scope.host_target_id)?;
    }
    Ok(browser)
}

pub(super) fn task_authorization_response(
    authorization: crate::TrustedTaskAuthorizationLease,
) -> Value {
    serde_json::json!({
        "status": "active",
        "authorization_id": authorization.authorization_id,
        "expires_at_unix_ms": authorization.expires_at_unix_ms,
        "allowed_host_methods": authorization.allowed_host_methods,
        "allowed_actions": authorization.allowed_actions,
        "browser_scope": authorization.browser_scope,
    })
}

pub(super) fn native_frame_lease_matches_binding(
    lease: Option<&crate::TrustedTaskAuthorizationLease>,
    session_id: &str,
    task_grant_id: &str,
    capability: &str,
    process_id: u32,
    window_handle: u64,
) -> bool {
    lease.is_some_and(|lease| {
        lease.session_id == session_id
            && lease.task_grant_id == task_grant_id
            && lease.window_capability == capability
            && lease.target_process_id == process_id
            && lease.target_window_handle == window_handle
            && ["get_window_state", "set_window_frame"]
                .iter()
                .all(|required| {
                    lease
                        .allowed_host_methods
                        .iter()
                        .any(|method| method == required)
                })
            && lease
                .allowed_actions
                .iter()
                .any(crate::TrustedTaskActionScope::is_window_frame)
    })
}

impl HostSession {
    pub(super) fn require_capture_preparation_grant(
        &self,
        session_id: &str,
        method: &str,
    ) -> Result<
        &dcc_cua_protocol::capture_preparation::CapturePreparationAuthorization,
        crate::HostError,
    > {
        let lease = self.bound_task_authorization(session_id, method)
            .filter(|lease| self.observation_mode == crate::TaskObservationMode::PixelsOnly
                && lease.allowed_actions.iter().any(crate::TrustedTaskActionScope::is_capture_preparation)
                && ["get_window_state", "capture_preparation_begin", "capture_preparation_state", "capture_preparation_stop"]
                    .iter().all(|required| lease.allowed_host_methods.iter().any(|method| method == required)))
            .and_then(|lease| lease.capture_preparation.as_ref())
            .ok_or_else(|| crate::HostError::coded_protocol(
                crate::HostProtocolErrorCode::TaskAuthorizationDenied,
                "capture preparation requires its exact trusted pixels_only lease, journal and closed lifecycle action scope",
            ))?;
        crate::TrustedTaskAuthorizationRegistration::validate_capture_preparation_directory(
            &lease.journal_directory,
        )
        .map_err(|error| {
            crate::HostError::coded_protocol(
                crate::HostProtocolErrorCode::TaskAuthorizationDenied,
                error.to_string(),
            )
        })?;
        Ok(lease)
    }
    fn bound_task_authorization(
        &self,
        session_id: &str,
        method: &str,
    ) -> Option<&crate::TrustedTaskAuthorizationLease> {
        self.task_authorization.as_ref().filter(|lease| {
            lease.session_id == session_id
                && lease.task_grant_id == self.task_grant_id
                && lease.window_capability == self.capability
                && lease.target_process_id == self.target_process_id
                && lease.target_window_handle == self.target_window_handle
                && lease
                    .allowed_host_methods
                    .iter()
                    .any(|allowed| allowed == method)
        })
    }

    pub(super) fn require_minimize_grant(&self, session_id: &str) -> Result<(), crate::HostError> {
        if !self
            .bound_task_authorization(session_id, "minimize_window")
            .is_some_and(|lease| {
                lease
                    .allowed_actions
                    .iter()
                    .any(crate::TrustedTaskActionScope::is_window_minimize)
            })
        {
            return Err(crate::HostError::coded_protocol(
                crate::HostProtocolErrorCode::TaskAuthorizationDenied,
                "minimize_window requires the trusted window_state/minimize_window action scope",
            ));
        }
        Ok(())
    }

    pub(super) fn require_window_frame_grant(
        &self,
        session_id: &str,
    ) -> Result<(), crate::HostError> {
        if !native_frame_lease_matches_binding(
            self.task_authorization.as_ref(),
            session_id,
            &self.task_grant_id,
            &self.capability,
            self.target_process_id,
            self.target_window_handle,
        ) {
            return Err(crate::HostError::coded_protocol(
                crate::HostProtocolErrorCode::TaskAuthorizationDenied,
                "set_window_frame requires its exact trusted window_state action scope and native state read method",
            ));
        }
        Ok(())
    }

    pub(super) fn require_pixels_input_grant(
        &self,
        session_id: &str,
        action: &crate::HostAction,
    ) -> Result<(), crate::HostError> {
        if action.input_kind != "raw_input"
            || !crate::TrustedTaskActionScope::PIXELS_INPUT_ACTIONS
                .contains(&action.action.as_str())
            || action.element_index.is_some()
            || action.element_token.is_some()
            || action.secret_handle.is_some()
            || action.input_backend_id.is_some()
            || action
                .delivery_mode
                .as_deref()
                .is_some_and(|mode| mode != "foreground")
        {
            return Err(crate::HostError::ComputerUse(
                dcc_cua_core::ComputerUseError::new(
                    ComputerUseErrorCode::InvalidAction,
                    "pixels_only input requires a supported raw action without semantic selectors, secrets, or alternate backends",
                ),
            ));
        }
        if !self.allow_raw_input
            || !self
                .bound_task_authorization(session_id, "execute_action")
                .is_some_and(|lease| {
                    lease
                        .allowed_actions
                        .iter()
                        .any(|scope| scope.is_pixels_input() && scope.action == action.action)
                })
        {
            return Err(crate::HostError::coded_protocol(
                crate::HostProtocolErrorCode::TaskAuthorizationDenied,
                "pixels_only input requires the exact supported raw_input action in the bound trusted task grant",
            ));
        }
        Ok(())
    }

    pub(super) fn require_latest_observation(
        &self,
        observation_id: &str,
    ) -> Result<(), crate::HostError> {
        if observation_id.is_empty()
            || self.latest_observation_id.as_deref() != Some(observation_id)
        {
            return Err(crate::HostError::ComputerUse(
                dcc_cua_core::ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "observation_id does not match the latest host snapshot",
                ),
            ));
        }
        Ok(())
    }

    pub(super) fn record_snapshot_observation(
        &mut self,
        observation_id: String,
        accessibility: &Value,
    ) {
        self.latest_observation_id = Some(observation_id.clone());
        if self.observation_mode == crate::TaskObservationMode::PixelsOnly {
            self.latest_accessibility_state_id = None;
            self.latest_accessibility_root = None;
        } else {
            self.latest_accessibility_state_id = Some(observation_id);
            self.latest_accessibility_root = Some(accessibility.clone());
        }
    }

    pub(super) fn require_task_authorized_method(
        &self,
        method: &str,
    ) -> Result<(), crate::HostError> {
        if self
            .task_authorization
            .as_ref()
            .is_some_and(|authorization| {
                !authorization
                    .allowed_host_methods
                    .iter()
                    .any(|allowed| allowed == method)
            })
        {
            return Err(crate::HostError::coded_protocol(
                crate::HostProtocolErrorCode::TaskAuthorizationDenied,
                format!("Host method {method} is outside the trusted task authorization"),
            ));
        }
        Ok(())
    }

    pub(super) fn require_allowed_browser_origin(
        &self,
        origin: &str,
    ) -> Result<(), crate::HostError> {
        if self.task_authorization.is_some()
            && !self
                .allowed_browser_origins
                .iter()
                .any(|allowed| allowed == origin)
        {
            return Err(crate::HostError::coded_protocol(
                crate::HostProtocolErrorCode::TaskAuthorizationDenied,
                "the browser origin is outside the task authorization",
            ));
        }
        Ok(())
    }

    pub(super) fn require_task_authorized_browser_target(
        &self,
        target_id: &str,
        tab_id: &str,
    ) -> Result<(), crate::HostError> {
        let Some(scope) = self
            .task_authorization
            .as_ref()
            .and_then(|lease| lease.browser_scope.as_ref())
        else {
            return Ok(());
        };
        if target_id == scope.host_target_id && tab_id == scope.tab_id {
            return Ok(());
        }
        Err(crate::HostError::coded_protocol(
            crate::HostProtocolErrorCode::TaskAuthorizationDenied,
            "the browser target or tab is outside the trusted task authorization",
        ))
    }

    pub(super) fn require_task_authorized_browser_document(
        &self,
        target_id: &str,
        tab_id: &str,
        document_generation: &str,
    ) -> Result<(), crate::HostError> {
        self.require_task_authorized_browser_target(target_id, tab_id)?;
        let Some(scope) = self
            .task_authorization
            .as_ref()
            .and_then(|lease| lease.browser_scope.as_ref())
        else {
            return Ok(());
        };
        if document_generation == scope.document_generation {
            return Ok(());
        }
        Err(crate::HostError::coded_protocol(
            crate::HostProtocolErrorCode::TaskAuthorizationDenied,
            "the browser document generation is outside the trusted task authorization",
        ))
    }

    pub(super) fn require_current_task_authorized_browser_document(
        &self,
    ) -> Result<(), crate::HostError> {
        let Some(scope) = self
            .task_authorization
            .as_ref()
            .and_then(|lease| lease.browser_scope.as_ref())
        else {
            return Ok(());
        };
        if self.browser.target_id() == Some(scope.host_target_id.as_str())
            && self.browser.latest_tab_id() == Some(scope.tab_id.as_str())
            && self.browser.latest_snapshot_id() == Some(scope.document_generation.as_str())
            && self.browser.latest_origin() == Some(scope.origin.as_str())
        {
            return Ok(());
        }
        Err(crate::HostError::coded_protocol(
            crate::HostProtocolErrorCode::TaskAuthorizationDenied,
            "fresh browser evidence does not match the authorized target, tab, document, and origin",
        ))
    }

    pub(super) fn require_current_allowed_browser_origin(&self) -> Result<(), crate::HostError> {
        if self.task_authorization.is_none() {
            return Ok(());
        }
        let origin = self.browser.latest_origin().ok_or_else(|| {
            crate::HostError::coded_protocol(
                crate::HostProtocolErrorCode::TaskAuthorizationRequired,
                "take a fresh browser snapshot before mutating an authorized browser origin",
            )
        })?;
        self.require_allowed_browser_origin(origin)
    }

    pub(super) fn is_idle_expired(&self) -> bool {
        self.last_activity.elapsed() >= self.idle_timeout
    }

    pub(super) fn mark_activity(&mut self) {
        self.last_activity = Instant::now();
    }

    pub(super) fn finish_observation_sensitive_attempt<T>(
        &mut self,
        result: ComputerUseResult<T>,
    ) -> ComputerUseResult<T> {
        self.synchronize_action_evidence_epoch();
        let Some(error) = result.as_ref().err() else {
            return result;
        };
        let input_unavailable = error.code == ComputerUseErrorCode::InteractiveDesktopUnavailable;
        let target_status = match error.code {
            ComputerUseErrorCode::TargetMinimized => Some(ComputerUseTargetStatus::Minimized),
            ComputerUseErrorCode::TargetUnavailable | ComputerUseErrorCode::MissingWindow => {
                Some(ComputerUseTargetStatus::Unavailable)
            }
            _ => None,
        };
        if input_unavailable
            || target_status.is_some()
            || error.code == ComputerUseErrorCode::InvalidTarget
            || error.code == ComputerUseErrorCode::SessionRefreshRequired
            || error.code == ComputerUseErrorCode::CompletionUnknown
        {
            self.invalidate_action_observations();
        }
        if input_unavailable {
            self.observe_input_readiness(
                ComputerUseInputReadiness {
                    status: ComputerUseInputStatus::Suspended,
                    code: "interactive_desktop_unavailable".into(),
                    reason: Some(error.message.clone()),
                },
                crate::session_events::observed_at_millis(),
            );
        }
        if let Some(status) = target_status {
            self.observe_target_availability(ComputerUseTargetAvailability {
                status,
                code: match status {
                    ComputerUseTargetStatus::Minimized => "target_minimized",
                    ComputerUseTargetStatus::Unavailable => "target_unavailable",
                    ComputerUseTargetStatus::Available => unreachable!(),
                }
                .into(),
                visible: false,
                minimized: status == ComputerUseTargetStatus::Minimized,
                foreground: false,
            });
        }
        result
    }

    pub(super) fn observe_input_readiness(
        &mut self,
        readiness: ComputerUseInputReadiness,
        observed_at: u64,
    ) -> bool {
        let transitioned = self.input_events.observe(readiness, observed_at).is_some();
        if transitioned {
            self.invalidate_action_observations();
        }
        transitioned
    }

    pub(super) fn refresh_input_readiness(&mut self) {
        let (readiness, observed_at) = crate::session_events::input_readiness_sample();
        self.observe_input_readiness(readiness, observed_at);
    }

    pub(super) fn observe_target_availability(
        &mut self,
        availability: ComputerUseTargetAvailability,
    ) -> bool {
        let transitioned = crate::session_events::refresh_target_availability(
            &mut self.input_events,
            availability,
        );
        if transitioned {
            self.invalidate_action_observations();
        }
        transitioned
    }

    pub(super) fn observe_target_state(&mut self, state: &Value) -> bool {
        #[cfg(windows)]
        if let Some(state_id) = state.get("window_state_id").and_then(Value::as_str) {
            let transitioned = crate::session_events::refresh_target_availability(
                &mut self.input_events,
                ComputerUseTargetAvailability::from_window_state(state),
            );
            if transitioned {
                self.session
                    .invalidate_action_observations_preserving_native_state(state_id);
                self.synchronize_action_evidence_epoch();
            }
            return transitioned;
        }
        self.observe_target_availability(ComputerUseTargetAvailability::from_window_state(state))
    }

    fn invalidate_action_observations(&mut self) {
        self.session.invalidate_action_observations();
        self.synchronize_action_evidence_epoch();
    }

    pub(super) fn abandon_wait_probe(&mut self) {
        // A cancelled Windows UIA future can leave its spawn_blocking worker
        // running. Drop the retained fallback and advance the evidence epoch so
        // the next semantic request never waits on that abandoned worker.
        self.invalidate_action_observations();
    }

    pub(super) fn synchronize_action_evidence_epoch(&mut self) -> bool {
        self.synchronize_action_evidence_epoch_with(HostEvidencePublication::None)
    }

    pub(super) fn synchronize_action_evidence_epoch_with(
        &mut self,
        publication: HostEvidencePublication,
    ) -> bool {
        let current = self.session.action_evidence_epoch();
        let plan = evidence_epoch_sync_plan(
            self.synchronized_action_evidence_epoch,
            current,
            publication,
        );
        if plan.epoch_changed {
            self.clear_native_observation_caches();
            if plan.invalidate_browser_snapshot {
                self.discard_browser_evidence();
            }
            self.synchronized_action_evidence_epoch = current;
        }
        if plan.bind_browser_snapshot {
            self.browser_evidence_epoch = Some(current);
        }
        plan.epoch_changed
    }

    pub(super) fn finish_browser_snapshot_attempt<T>(
        &mut self,
        result: ComputerUseResult<T>,
        publishes_snapshot_evidence: bool,
    ) -> Result<T, crate::HostError> {
        match result {
            Ok(value) if publishes_snapshot_evidence => {
                if let Err(error) = self.require_current_task_authorized_browser_document() {
                    self.discard_browser_evidence();
                    return Err(error);
                }
                self.synchronize_action_evidence_epoch_with(
                    HostEvidencePublication::BrowserSnapshot,
                );
                Ok(value)
            }
            Ok(value) => {
                self.discard_browser_evidence();
                self.synchronize_action_evidence_epoch();
                Ok(value)
            }
            Err(error) => {
                self.discard_browser_evidence();
                self.finish_observation_sensitive_attempt(Err(error))
                    .map_err(Into::into)
            }
        }
    }

    pub(super) fn require_current_browser_evidence_epoch(&mut self) -> ComputerUseResult<()> {
        self.synchronize_action_evidence_epoch();
        if self.browser_evidence_epoch == Some(self.synchronized_action_evidence_epoch) {
            return Ok(());
        }
        Err(dcc_cua_core::ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "take a fresh browser snapshot after action evidence changed",
        ))
    }

    pub(super) fn invalidate_observations(&mut self) {
        self.synchronized_action_evidence_epoch = self.session.action_evidence_epoch();
        self.clear_native_observation_caches();
        self.discard_browser_evidence();
    }

    fn clear_native_observation_caches(&mut self) {
        self.latest_observation_id = None;
        self.latest_accessibility_state_id = None;
        self.latest_accessibility_root = None;
    }

    pub(super) fn discard_browser_evidence(&mut self) {
        self.browser_evidence_epoch = None;
        self.browser.invalidate_snapshot();
    }
}

pub(super) struct HostDesktopSession {
    pub(super) runtime_session_id: String,
    pub(super) task_grant_id: String,
    pub(super) allow_raw_input: bool,
    pub(super) allow_trusted_confirmation: bool,
    pub(super) capability: String,
    pub(super) interrupt_generation: u64,
    pub(super) interrupted: bool,
    pub(super) session: ComputerUseDesktopSession,
    pub(super) latest_shared_image: Option<SharedImage>,
}

#[derive(Clone)]
pub(super) struct HostLaunchSession {
    pub(super) runtime_session_id: String,
    pub(super) task_grant_id: String,
    pub(super) application_label: String,
    pub(super) process_id: u32,
}

pub(super) struct ConnectionSessions {
    pub(super) connection_id: String,
    pub(super) agent_name: String,
    pub(super) windows: HashMap<String, HostSession>,
    pub(super) desktops: HashMap<String, HostDesktopSession>,
    pub(super) launches: HashMap<String, HostLaunchSession>,
}

impl Default for ConnectionSessions {
    fn default() -> Self {
        Self {
            connection_id: format!("host-connection-{}", uuid::Uuid::new_v4()),
            agent_name: String::new(),
            windows: HashMap::new(),
            desktops: HashMap::new(),
            launches: HashMap::new(),
        }
    }
}
