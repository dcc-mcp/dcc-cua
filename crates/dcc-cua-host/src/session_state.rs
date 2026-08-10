use std::collections::HashMap;

use dcc_cua_browser::BrowserSession;
use dcc_cua_core::{
    ComputerUseDesktopSession, ComputerUseErrorCode, ComputerUseInputReadiness,
    ComputerUseInputStatus, ComputerUseResult, ComputerUseSession, ComputerUseTargetAvailability,
    ComputerUseTargetStatus,
};
use dcc_cua_shm::SharedImage;
use serde_json::Value;

use crate::session_events::SessionInputEventQueue;

pub(super) struct HostSession {
    pub(super) runtime_session_id: String,
    pub(super) task_grant_id: String,
    pub(super) allow_raw_input: bool,
    pub(super) allow_app_terminate: bool,
    pub(super) allow_clipboard_read: bool,
    pub(super) allow_clipboard_write: bool,
    pub(super) allow_recording: bool,
    pub(super) allow_live_observation: bool,
    pub(super) allow_browser_input: bool,
    pub(super) allow_browser_prepare: bool,
    pub(super) allow_browser_download: bool,
    pub(super) allow_native_tool: bool,
    pub(super) allow_menu_invoke: bool,
    pub(super) allow_session_escalation: bool,
    pub(super) allow_trusted_confirmation: bool,
    pub(super) allow_restore_activate: bool,
    pub(super) capability: String,
    pub(super) interrupted: bool,
    pub(super) session: ComputerUseSession,
    pub(super) browser: BrowserSession,
    pub(super) latest_observation_id: Option<String>,
    pub(super) latest_accessibility_state_id: Option<String>,
    pub(super) latest_accessibility_root: Option<Value>,
    pub(super) latest_shared_image: Option<SharedImage>,
    pub(super) input_events: SessionInputEventQueue,
}

impl HostSession {
    pub(super) fn finish_observation_sensitive_attempt<T>(
        &mut self,
        result: ComputerUseResult<T>,
    ) -> ComputerUseResult<T> {
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
        self.observe_target_availability(ComputerUseTargetAvailability::from_window_state(state))
    }

    fn invalidate_action_observations(&mut self) {
        self.session.invalidate_action_observations();
        self.invalidate_observations();
    }

    pub(super) fn invalidate_observations(&mut self) {
        self.latest_observation_id = None;
        self.latest_accessibility_state_id = None;
        self.latest_accessibility_root = None;
        self.latest_shared_image = None;
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

#[derive(Default)]
pub(super) struct ConnectionSessions {
    pub(super) agent_name: String,
    pub(super) windows: HashMap<String, HostSession>,
    pub(super) desktops: HashMap<String, HostDesktopSession>,
    pub(super) launches: HashMap<String, HostLaunchSession>,
}
