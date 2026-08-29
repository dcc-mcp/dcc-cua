use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use thiserror::Error;
use uuid::Uuid;

use dcc_cua_core::ComputerUseOwnedBrowserLaunchSpec;

use crate::task_authorization::{
    MAX_TASK_AUTHORIZATION_ACTIONS, MAX_TASK_AUTHORIZATION_TTL_MS,
    TRUSTED_TASK_AUTHORIZATION_LEASE_VALIDATION_SCHEMA, TRUSTED_TASK_AUTHORIZATION_SCHEMA,
    TRUSTED_TASK_AUTHORIZATION_VALIDATION_SCHEMA, TrustedTaskActionScope,
    TrustedTaskAuthorizationBrowserScope, TrustedTaskAuthorizationHost,
    TrustedTaskAuthorizationHostError, TrustedTaskAuthorizationLease,
    TrustedTaskAuthorizationLeaseValidationRequest, TrustedTaskAuthorizationRequest,
    TrustedTaskAuthorizationStatus, TrustedTaskAuthorizationValidationDecision,
    TrustedTaskAuthorizationValidationRequest,
};
use crate::{MAX_APPLICATION_LABEL_CHARS, MAX_TASK_GRANT_ID_CHARS};

const MAX_BROKER_AUTHORIZATIONS: usize = 256;

/// Exact scope registered only by an authenticated embedding after explicit
/// user input. This value never crosses Host IPC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedTaskAuthorizationRegistration {
    pub connection_id: Option<String>,
    pub task_id: Option<String>,
    pub task_grant_id: String,
    pub application_label: String,
    pub target: TrustedTaskAuthorizationTarget,
    pub allowed_host_methods: Vec<String>,
    pub allowed_actions: Vec<TrustedTaskActionScope>,
    pub allowed_browser_origins: Vec<String>,
    pub browser_scope: Option<TrustedTaskAuthorizationBrowserScope>,
    pub expires_at_unix_ms: u64,
}

/// User-visible target scope registered before a task starts.
///
/// Exact windows preserve the existing attach contract. Owned browsers expose
/// only a closed launch specification; their PID and HWND are accepted from
/// the trusted Host exactly once after DCC-CUA launches and observes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedTaskAuthorizationTarget {
    ExactWindow { process_id: u32, window_handle: u64 },
    OwnedBrowser(ComputerUseOwnedBrowserLaunchSpec),
}

/// Opaque reference that an embedding may place in a task grant. It carries no
/// authority without the constructor-owned broker state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedTaskAuthorizationReceipt {
    pub authorization_id: String,
    pub window_capability: String,
    pub expires_at_unix_ms: u64,
}

impl TrustedTaskAuthorizationRegistration {
    /// Validate a proposed exact task scope before presenting it to a user.
    pub fn validate(&self) -> Result<(), TrustedTaskAuthorizationBrokerError> {
        validate_registration(self)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TrustedTaskAuthorizationBrokerError {
    #[error("task authorization registration is invalid: {reason}")]
    InvalidRegistration { reason: String },
    #[error("task authorization was not found")]
    NotFound,
    #[error("task authorization broker is unavailable")]
    Unavailable,
}

/// Move-only issuer capability held by the embedding's authenticated user-input
/// surface. It is intentionally neither serializable nor reachable from Host
/// IPC.
pub struct TrustedTaskAuthorizationIssuer {
    state: Arc<Mutex<BrokerState>>,
}

impl TrustedTaskAuthorizationIssuer {
    pub fn register(
        &self,
        registration: TrustedTaskAuthorizationRegistration,
    ) -> Result<TrustedTaskAuthorizationReceipt, TrustedTaskAuthorizationBrokerError> {
        registration.validate()?;
        let now = crate::task_authorization::unix_time_millis();
        let mut state = self
            .state
            .lock()
            .map_err(|_| TrustedTaskAuthorizationBrokerError::Unavailable)?;
        state
            .authorizations
            .retain(|_, record| record.registration.expires_at_unix_ms > now);
        if state.authorizations.len() >= MAX_BROKER_AUTHORIZATIONS {
            return Err(TrustedTaskAuthorizationBrokerError::Unavailable);
        }
        let authorization_id = loop {
            let candidate = format!("task-auth-{}", Uuid::new_v4());
            if !state.authorizations.contains_key(&candidate) {
                break candidate;
            }
        };
        let expires_at_unix_ms = registration.expires_at_unix_ms;
        let window_capability = format!("cua-window-{}", Uuid::new_v4());
        state.authorizations.insert(
            authorization_id.clone(),
            BrokerRecord {
                registration,
                window_capability: window_capability.clone(),
                binding: None,
                revoked: false,
            },
        );
        Ok(TrustedTaskAuthorizationReceipt {
            authorization_id,
            window_capability,
            expires_at_unix_ms,
        })
    }

    pub fn revoke(
        &self,
        authorization_id: &str,
    ) -> Result<(), TrustedTaskAuthorizationBrokerError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| TrustedTaskAuthorizationBrokerError::Unavailable)?;
        let record = state
            .authorizations
            .get_mut(authorization_id)
            .ok_or(TrustedTaskAuthorizationBrokerError::NotFound)?;
        record.revoked = true;
        Ok(())
    }
}

/// Create a split-capability task-authorization broker.
///
/// The issuer must remain inside the authenticated embedding. Only the returned
/// Host trait object is installed in `HostSecurityServices`.
pub fn trusted_task_authorization_broker() -> (
    TrustedTaskAuthorizationIssuer,
    Arc<dyn TrustedTaskAuthorizationHost>,
) {
    let state = Arc::new(Mutex::new(BrokerState::default()));
    (
        TrustedTaskAuthorizationIssuer {
            state: Arc::clone(&state),
        },
        Arc::new(BrokerHost { state }),
    )
}

#[derive(Default)]
struct BrokerState {
    authorizations: BTreeMap<String, BrokerRecord>,
}

struct BrokerRecord {
    registration: TrustedTaskAuthorizationRegistration,
    window_capability: String,
    binding: Option<BrokerBinding>,
    revoked: bool,
}

struct BrokerBinding {
    connection_id: String,
    session_id: String,
    lease_request_digest: String,
    target_process_id: u32,
    target_window_handle: u64,
}

struct BrokerHost {
    state: Arc<Mutex<BrokerState>>,
}

#[async_trait]
impl TrustedTaskAuthorizationHost for BrokerHost {
    async fn authorize(
        &self,
        request: TrustedTaskAuthorizationRequest,
    ) -> Result<TrustedTaskAuthorizationLease, TrustedTaskAuthorizationHostError> {
        if request.schema != TRUSTED_TASK_AUTHORIZATION_SCHEMA {
            return Err(TrustedTaskAuthorizationHostError::Denied);
        }
        let now = crate::task_authorization::unix_time_millis();
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        let record = state
            .authorizations
            .get_mut(&request.authorization_id)
            .ok_or(TrustedTaskAuthorizationHostError::Denied)?;
        let registration = &record.registration;
        let target_matches = match registration.target {
            TrustedTaskAuthorizationTarget::ExactWindow {
                process_id,
                window_handle,
            } => {
                request.target_process_id == process_id
                    && request.target_window_handle == window_handle
            }
            TrustedTaskAuthorizationTarget::OwnedBrowser(_) => {
                request.target_process_id != 0 && request.target_window_handle != 0
            }
        };
        let exact_match = request.task_grant_id == registration.task_grant_id
            && request.application_label == registration.application_label
            && request.window_capability == record.window_capability
            && registration
                .task_id
                .as_deref()
                .is_none_or(|expected| request.session_id == expected)
            && registration
                .connection_id
                .as_deref()
                .is_none_or(|expected| request.connection_id == expected)
            && target_matches;
        if record.revoked
            || record.binding.is_some()
            || registration.expires_at_unix_ms <= now
            || !exact_match
        {
            return Err(TrustedTaskAuthorizationHostError::Denied);
        }
        record.binding = Some(BrokerBinding {
            connection_id: request.connection_id.clone(),
            session_id: request.session_id.clone(),
            lease_request_digest: request.request_digest.clone(),
            target_process_id: request.target_process_id,
            target_window_handle: request.target_window_handle,
        });
        Ok(TrustedTaskAuthorizationLease {
            connection_id: request.connection_id,
            authorization_id: request.authorization_id,
            session_id: request.session_id,
            task_grant_id: request.task_grant_id,
            application_label: request.application_label,
            window_capability: request.window_capability,
            target_process_id: request.target_process_id,
            target_window_handle: request.target_window_handle,
            allowed_actions: registration.allowed_actions.clone(),
            allowed_host_methods: registration.allowed_host_methods.clone(),
            allowed_browser_origins: registration.allowed_browser_origins.clone(),
            browser_scope: registration.browser_scope.clone(),
            issued_at_unix_ms: now,
            expires_at_unix_ms: registration.expires_at_unix_ms,
            request_digest: request.request_digest,
        })
    }

    async fn validate(
        &self,
        request: TrustedTaskAuthorizationValidationRequest,
    ) -> Result<TrustedTaskAuthorizationValidationDecision, TrustedTaskAuthorizationHostError> {
        if request.schema != TRUSTED_TASK_AUTHORIZATION_VALIDATION_SCHEMA {
            return Err(unavailable());
        }
        let state = self.state.lock().map_err(|_| unavailable())?;
        let record = state
            .authorizations
            .get(&request.authorization_id)
            .ok_or_else(unavailable)?;
        let binding = record.binding.as_ref().ok_or_else(unavailable)?;
        let registration = &record.registration;
        let exact_match = request.session_id == binding.session_id
            && request.connection_id == binding.connection_id
            && request.lease_request_digest == binding.lease_request_digest
            && request.task_grant_id == registration.task_grant_id
            && request.application_label == registration.application_label
            && request.window_capability == record.window_capability
            && request.target_process_id == binding.target_process_id
            && request.target_window_handle == binding.target_window_handle
            && registration.allowed_actions.contains(&request.action_scope);
        if !exact_match {
            return Err(unavailable());
        }
        Ok(TrustedTaskAuthorizationValidationDecision {
            status: if record.revoked {
                TrustedTaskAuthorizationStatus::Revoked
            } else {
                TrustedTaskAuthorizationStatus::Active
            },
            request_digest: request.request_digest,
        })
    }

    async fn validate_lease(
        &self,
        request: TrustedTaskAuthorizationLeaseValidationRequest,
    ) -> Result<TrustedTaskAuthorizationValidationDecision, TrustedTaskAuthorizationHostError> {
        if request.schema != TRUSTED_TASK_AUTHORIZATION_LEASE_VALIDATION_SCHEMA {
            return Err(unavailable());
        }
        let state = self.state.lock().map_err(|_| unavailable())?;
        let record = state
            .authorizations
            .get(&request.authorization_id)
            .ok_or_else(unavailable)?;
        let binding = record.binding.as_ref().ok_or_else(unavailable)?;
        let registration = &record.registration;
        let exact_match = request.session_id == binding.session_id
            && request.connection_id == binding.connection_id
            && request.lease_request_digest == binding.lease_request_digest
            && request.task_grant_id == registration.task_grant_id
            && request.application_label == registration.application_label
            && request.window_capability == record.window_capability
            && request.target_process_id == binding.target_process_id
            && request.target_window_handle == binding.target_window_handle;
        if !exact_match {
            return Err(unavailable());
        }
        Ok(TrustedTaskAuthorizationValidationDecision {
            status: if record.revoked {
                TrustedTaskAuthorizationStatus::Revoked
            } else {
                TrustedTaskAuthorizationStatus::Active
            },
            request_digest: request.request_digest,
        })
    }
}

fn validate_registration(
    registration: &TrustedTaskAuthorizationRegistration,
) -> Result<(), TrustedTaskAuthorizationBrokerError> {
    if let Some(connection_id) = registration.connection_id.as_deref() {
        validate_identity(connection_id, 256, "connection_id")?;
    }
    if let Some(task_id) = registration.task_id.as_deref() {
        validate_identity(task_id, 256, "task_id")?;
    }
    validate_identity(
        &registration.task_grant_id,
        MAX_TASK_GRANT_ID_CHARS,
        "task_grant_id",
    )?;
    validate_identity(
        &registration.application_label,
        MAX_APPLICATION_LABEL_CHARS,
        "application_label",
    )?;
    if matches!(
        registration.target,
        TrustedTaskAuthorizationTarget::ExactWindow { process_id: 0, .. }
            | TrustedTaskAuthorizationTarget::ExactWindow {
                window_handle: 0,
                ..
            }
    ) {
        return invalid("the exact target PID and window handle must be non-zero");
    }
    let now = crate::task_authorization::unix_time_millis();
    if registration.expires_at_unix_ms <= now
        || registration.expires_at_unix_ms.saturating_sub(now) > MAX_TASK_AUTHORIZATION_TTL_MS
    {
        return invalid("expiry must be in the future and no more than 24 hours away");
    }
    let actions = registration
        .allowed_actions
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if actions.is_empty()
        || actions.len() != registration.allowed_actions.len()
        || actions.len() > MAX_TASK_AUTHORIZATION_ACTIONS
        || !actions.iter().all(TrustedTaskActionScope::validate)
    {
        return invalid("allowed actions must be unique, closed, and non-empty");
    }
    let methods = registration
        .allowed_host_methods
        .iter()
        .collect::<BTreeSet<_>>();
    if methods.is_empty()
        || methods.len() != registration.allowed_host_methods.len()
        || methods.len() > 64
        || methods.iter().any(|method| {
            !crate::task_authorization_scope::is_task_authorizable_host_method(method)
        })
    {
        return invalid("allowed Host methods must be unique, closed, and non-empty");
    }
    let origins = registration
        .allowed_browser_origins
        .iter()
        .collect::<BTreeSet<_>>();
    if origins.len() != registration.allowed_browser_origins.len()
        || origins.len() > MAX_TASK_AUTHORIZATION_ACTIONS
        || origins
            .iter()
            .any(|origin| !crate::task_authorization::valid_browser_origin(origin))
    {
        return invalid("allowed browser origins must be unique exact HTTP(S) origins");
    }
    if let Some(scope) = registration.browser_scope.as_ref()
        && (!scope.validate()
            || matches!(
                registration.target,
                TrustedTaskAuthorizationTarget::OwnedBrowser(_)
            )
            || registration.allowed_browser_origins.as_slice() != [scope.origin.as_str()]
            || !registration
                .allowed_host_methods
                .iter()
                .any(|method| method == "browser_snapshot"))
    {
        return invalid(
            "an exact browser scope requires an exact window, its sole origin, and browser_snapshot",
        );
    }
    Ok(())
}

fn validate_identity(
    value: &str,
    max_chars: usize,
    field: &str,
) -> Result<(), TrustedTaskAuthorizationBrokerError> {
    if value.is_empty()
        || value != value.trim()
        || value.chars().count() > max_chars
        || value.chars().any(char::is_control)
    {
        return invalid(format!(
            "{field} must contain 1..{max_chars} printable characters without surrounding whitespace"
        ));
    }
    Ok(())
}

fn invalid<T>(reason: impl Into<String>) -> Result<T, TrustedTaskAuthorizationBrokerError> {
    Err(TrustedTaskAuthorizationBrokerError::InvalidRegistration {
        reason: reason.into(),
    })
}

fn unavailable() -> TrustedTaskAuthorizationHostError {
    TrustedTaskAuthorizationHostError::Unavailable {
        reason: "the constructor-owned task authorization broker rejected the request".into(),
    }
}
