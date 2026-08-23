use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::action_confirmation::{ConfirmationWindowIdentity, TrustedActionConfirmationRequest};
use crate::{
    ActionConfirmationOutcome, HostError, HostProtocolErrorCode, HostSecurityServices, HostSession,
    authorize_action_confirmation,
};

pub const TRUSTED_TASK_AUTHORIZATION_SCHEMA: &str = "dcc-cua-trusted-task-authorization-v1";
const TRUSTED_TASK_AUTHORIZATION_VALIDATION_SCHEMA: &str =
    "dcc-cua-trusted-task-authorization-validation-v1";
const MAX_TASK_AUTHORIZATION_ID_CHARS: usize = 128;
const MAX_TASK_AUTHORIZATION_ACTIONS: usize = 32;
const MAX_TASK_AUTHORIZATION_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct TrustedTaskActionScope {
    pub action: String,
    pub input_kind: String,
    pub secret_input: bool,
    pub authorization_category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_origin: Option<String>,
}

impl TrustedTaskActionScope {
    fn validate(&self) -> bool {
        let fields_are_closed = matches!(
            self.action.as_str(),
            "click"
                | "double_click"
                | "right_click"
                | "toggle"
                | "drag"
                | "type"
                | "type_chars"
                | "set_text"
                | "set_value"
                | "set_checked"
                | "keypress"
                | "press"
                | "press_key"
                | "keyboard_shortcut"
                | "hotkey"
                | "browser_type"
                | "clipboard_capture_secret"
        ) && matches!(
            self.input_kind.as_str(),
            "raw_input" | "semantic" | "browser" | "clipboard"
        ) && matches!(
            self.authorization_category.as_str(),
            "account_access"
                | "account_security"
                | "content_change"
                | "credential"
                | "destructive"
                | "destructive_write"
                | "external_effect"
                | "payment"
                | "publishing"
                | "raw_input"
        );
        let binding_is_coherent = match self.input_kind.as_str() {
            "browser" => {
                self.action == "browser_type"
                    && self.secret_input
                    && self.authorization_category == "credential"
                    && self
                        .browser_origin
                        .as_deref()
                        .is_some_and(valid_browser_origin)
            }
            "clipboard" => {
                self.action == "clipboard_capture_secret"
                    && self.secret_input
                    && self.authorization_category == "credential"
                    && self.browser_origin.is_none()
            }
            "raw_input" => {
                matches!(
                    self.authorization_category.as_str(),
                    "raw_input" | "credential"
                ) && self.browser_origin.is_none()
            }
            "semantic" => {
                self.authorization_category != "raw_input" && self.browser_origin.is_none()
            }
            _ => false,
        };
        fields_are_closed && binding_is_coherent
    }

    fn from_confirmation(request: &TrustedActionConfirmationRequest) -> Option<Self> {
        if request
            .action
            .get("method")
            .and_then(|value| value.as_str())
            == Some("browser_type")
        {
            let browser_origin = request
                .action
                .get("browser_origin")
                .and_then(|value| value.as_str())?
                .to_owned();
            return Some(Self {
                action: "browser_type".into(),
                input_kind: "browser".into(),
                secret_input: true,
                authorization_category: "credential".into(),
                browser_origin: Some(browser_origin),
            });
        }
        let action = request.action.get("action")?.as_str()?;
        if action == "clipboard_capture_secret" {
            return Some(Self {
                action: action.to_owned(),
                input_kind: "clipboard".into(),
                secret_input: true,
                authorization_category: "credential".into(),
                browser_origin: None,
            });
        }
        Some(Self {
            action: action.to_owned(),
            input_kind: request.action.get("input_kind")?.as_str()?.to_owned(),
            secret_input: request.action.get("secret_handle").is_some(),
            authorization_category: request
                .action
                .get("authorization_category")?
                .as_str()?
                .to_owned(),
            browser_origin: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrustedTaskAuthorizationRequest {
    pub schema: String,
    pub request_id: String,
    pub authorization_id: String,
    pub session_id: String,
    pub task_grant_id: String,
    pub application_label: String,
    pub window_capability: String,
    pub target_process_id: u32,
    pub target_window_handle: u64,
    pub request_digest: String,
}

#[derive(Serialize)]
struct UnsignedTaskAuthorizationRequest<'a> {
    schema: &'a str,
    request_id: &'a str,
    authorization_id: &'a str,
    session_id: &'a str,
    task_grant_id: &'a str,
    application_label: &'a str,
    window_capability: &'a str,
    target_process_id: u32,
    target_window_handle: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedTaskAuthorizationLease {
    pub authorization_id: String,
    pub session_id: String,
    pub task_grant_id: String,
    pub application_label: String,
    pub window_capability: String,
    pub target_process_id: u32,
    pub target_window_handle: u64,
    pub allowed_actions: Vec<TrustedTaskActionScope>,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub request_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrustedTaskAuthorizationValidationRequest {
    pub schema: String,
    pub request_id: String,
    pub authorization_id: String,
    pub session_id: String,
    pub task_grant_id: String,
    pub application_label: String,
    pub window_capability: String,
    pub target_process_id: u32,
    pub target_window_handle: u64,
    pub action_scope: TrustedTaskActionScope,
    pub action_request_digest: String,
    pub lease_request_digest: String,
    pub request_digest: String,
}

#[derive(Serialize)]
struct UnsignedTaskAuthorizationValidationRequest<'a> {
    schema: &'a str,
    request_id: &'a str,
    authorization_id: &'a str,
    session_id: &'a str,
    task_grant_id: &'a str,
    application_label: &'a str,
    window_capability: &'a str,
    target_process_id: u32,
    target_window_handle: u64,
    action_scope: &'a TrustedTaskActionScope,
    action_request_digest: &'a str,
    lease_request_digest: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustedTaskAuthorizationStatus {
    Active,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedTaskAuthorizationValidationDecision {
    pub status: TrustedTaskAuthorizationStatus,
    pub request_digest: String,
}

#[derive(Debug, Error)]
pub enum TrustedTaskAuthorizationHostError {
    #[error("trusted task authorization was denied")]
    Denied,
    #[error("trusted task authorization host failed: {reason}")]
    Unavailable { reason: String },
}

#[async_trait]
pub trait TrustedTaskAuthorizationHost: Send + Sync {
    async fn authorize(
        &self,
        request: TrustedTaskAuthorizationRequest,
    ) -> Result<TrustedTaskAuthorizationLease, TrustedTaskAuthorizationHostError>;

    async fn validate(
        &self,
        request: TrustedTaskAuthorizationValidationRequest,
    ) -> Result<TrustedTaskAuthorizationValidationDecision, TrustedTaskAuthorizationHostError>;
}

#[derive(Clone, Copy)]
pub(crate) struct TaskAuthorizationBinding<'a> {
    authorization_id: &'a str,
    session_id: &'a str,
    task_grant_id: &'a str,
    application_label: &'a str,
    window_capability: &'a str,
    target: ConfirmationWindowIdentity,
}

impl<'a> TaskAuthorizationBinding<'a> {
    pub(crate) fn window(
        authorization_id: &'a str,
        session_id: &'a str,
        task_grant_id: &'a str,
        application_label: &'a str,
        window_capability: &'a str,
        target: ConfirmationWindowIdentity,
    ) -> Self {
        Self {
            authorization_id,
            session_id,
            task_grant_id,
            application_label,
            window_capability,
            target,
        }
    }

    fn request(self) -> Result<TrustedTaskAuthorizationRequest, HostError> {
        validate_authorization_id(self.authorization_id)?;
        let request_id = Uuid::new_v4().to_string();
        let unsigned = UnsignedTaskAuthorizationRequest {
            schema: TRUSTED_TASK_AUTHORIZATION_SCHEMA,
            request_id: &request_id,
            authorization_id: self.authorization_id,
            session_id: self.session_id,
            task_grant_id: self.task_grant_id,
            application_label: self.application_label,
            window_capability: self.window_capability,
            target_process_id: self.target.process_id,
            target_window_handle: self.target.window_handle,
        };
        let request_digest = digest(&unsigned, "task authorization")?;
        Ok(TrustedTaskAuthorizationRequest {
            schema: TRUSTED_TASK_AUTHORIZATION_SCHEMA.to_owned(),
            request_id,
            authorization_id: self.authorization_id.to_owned(),
            session_id: self.session_id.to_owned(),
            task_grant_id: self.task_grant_id.to_owned(),
            application_label: self.application_label.to_owned(),
            window_capability: self.window_capability.to_owned(),
            target_process_id: self.target.process_id,
            target_window_handle: self.target.window_handle,
            request_digest,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskAuthorizationOutcome {
    Allowed,
    NotConfigured,
    OutOfScope,
    Expired,
    Revoked,
    Required,
}

pub(crate) async fn issue_task_authorization(
    host: Option<&dyn TrustedTaskAuthorizationHost>,
    binding: TaskAuthorizationBinding<'_>,
) -> Result<TrustedTaskAuthorizationLease, HostError> {
    let Some(host) = host else {
        return Err(task_authorization_required(
            "task authorization requires a constructor-owned host",
        ));
    };
    let request = binding.request()?;
    let expected = request.clone();
    let lease = host.authorize(request).await.map_err(|error| match error {
        TrustedTaskAuthorizationHostError::Denied => HostError::coded_protocol(
            HostProtocolErrorCode::TaskAuthorizationDenied,
            "the trusted task authorization was denied",
        ),
        TrustedTaskAuthorizationHostError::Unavailable { .. } => {
            task_authorization_required("the trusted task authorization was not available")
        }
    })?;
    validate_lease(&expected, &lease)?;
    Ok(lease)
}

pub(crate) async fn authorize_task_scoped_action(
    host: Option<&dyn TrustedTaskAuthorizationHost>,
    lease: Option<&TrustedTaskAuthorizationLease>,
    confirmation: &TrustedActionConfirmationRequest,
) -> TaskAuthorizationOutcome {
    let Some(lease) = lease else {
        return TaskAuthorizationOutcome::NotConfigured;
    };
    let now = unix_time_millis();
    if now > lease.expires_at_unix_ms {
        return TaskAuthorizationOutcome::Expired;
    }
    if confirmation.session_id != lease.session_id
        || confirmation.task_grant_id != lease.task_grant_id
        || confirmation.window_capability != lease.window_capability
        || confirmation.target_process_id != Some(lease.target_process_id)
        || confirmation.target_window_handle != Some(lease.target_window_handle)
    {
        return TaskAuthorizationOutcome::OutOfScope;
    }
    let Some(action_scope) = TrustedTaskActionScope::from_confirmation(confirmation) else {
        return TaskAuthorizationOutcome::OutOfScope;
    };
    if !lease.allowed_actions.contains(&action_scope) {
        return TaskAuthorizationOutcome::OutOfScope;
    }
    let Some(host) = host else {
        return TaskAuthorizationOutcome::Required;
    };
    let request = match validation_request(lease, confirmation, action_scope) {
        Ok(request) => request,
        Err(_) => return TaskAuthorizationOutcome::Required,
    };
    let expected_digest = request.request_digest.clone();
    let Ok(decision) = host.validate(request).await else {
        return TaskAuthorizationOutcome::Required;
    };
    if decision.request_digest != expected_digest {
        return TaskAuthorizationOutcome::Required;
    }
    match decision.status {
        TrustedTaskAuthorizationStatus::Active => TaskAuthorizationOutcome::Allowed,
        TrustedTaskAuthorizationStatus::Revoked => TaskAuthorizationOutcome::Revoked,
    }
}

pub(crate) async fn authorize_window_confirmation(
    security_services: &HostSecurityServices,
    host: &HostSession,
    confirmation: TrustedActionConfirmationRequest,
) -> ActionConfirmationOutcome {
    match authorize_task_scoped_action(
        security_services.task_authorization_host.as_deref(),
        host.task_authorization.as_ref(),
        &confirmation,
    )
    .await
    {
        TaskAuthorizationOutcome::Allowed => ActionConfirmationOutcome::Allowed,
        TaskAuthorizationOutcome::NotConfigured => {
            authorize_action_confirmation(
                security_services.confirmation_host.as_deref(),
                host.allow_trusted_confirmation,
                confirmation,
            )
            .await
        }
        TaskAuthorizationOutcome::OutOfScope => {
            ActionConfirmationOutcome::TaskAuthorizationOutOfScope
        }
        TaskAuthorizationOutcome::Expired => ActionConfirmationOutcome::TaskAuthorizationExpired,
        TaskAuthorizationOutcome::Revoked => ActionConfirmationOutcome::TaskAuthorizationRevoked,
        TaskAuthorizationOutcome::Required => ActionConfirmationOutcome::TaskAuthorizationRequired,
    }
}

fn validation_request(
    lease: &TrustedTaskAuthorizationLease,
    confirmation: &TrustedActionConfirmationRequest,
    action_scope: TrustedTaskActionScope,
) -> Result<TrustedTaskAuthorizationValidationRequest, HostError> {
    let request_id = Uuid::new_v4().to_string();
    let unsigned = UnsignedTaskAuthorizationValidationRequest {
        schema: TRUSTED_TASK_AUTHORIZATION_VALIDATION_SCHEMA,
        request_id: &request_id,
        authorization_id: &lease.authorization_id,
        session_id: &lease.session_id,
        task_grant_id: &lease.task_grant_id,
        application_label: &lease.application_label,
        window_capability: &lease.window_capability,
        target_process_id: lease.target_process_id,
        target_window_handle: lease.target_window_handle,
        action_scope: &action_scope,
        action_request_digest: &confirmation.request_digest,
        lease_request_digest: &lease.request_digest,
    };
    let request_digest = digest(&unsigned, "task authorization validation")?;
    Ok(TrustedTaskAuthorizationValidationRequest {
        schema: TRUSTED_TASK_AUTHORIZATION_VALIDATION_SCHEMA.to_owned(),
        request_id,
        authorization_id: lease.authorization_id.clone(),
        session_id: lease.session_id.clone(),
        task_grant_id: lease.task_grant_id.clone(),
        application_label: lease.application_label.clone(),
        window_capability: lease.window_capability.clone(),
        target_process_id: lease.target_process_id,
        target_window_handle: lease.target_window_handle,
        action_scope,
        action_request_digest: confirmation.request_digest.clone(),
        lease_request_digest: lease.request_digest.clone(),
        request_digest,
    })
}

fn validate_lease(
    request: &TrustedTaskAuthorizationRequest,
    lease: &TrustedTaskAuthorizationLease,
) -> Result<(), HostError> {
    let fields_match = lease.authorization_id == request.authorization_id
        && lease.session_id == request.session_id
        && lease.task_grant_id == request.task_grant_id
        && lease.application_label == request.application_label
        && lease.window_capability == request.window_capability
        && lease.target_process_id == request.target_process_id
        && lease.target_window_handle == request.target_window_handle
        && lease.request_digest == request.request_digest;
    let now = unix_time_millis();
    let valid_time = lease.issued_at_unix_ms <= now
        && lease.expires_at_unix_ms > now
        && lease
            .expires_at_unix_ms
            .saturating_sub(lease.issued_at_unix_ms)
            <= MAX_TASK_AUTHORIZATION_TTL_MS;
    let actions = lease
        .allowed_actions
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let valid_actions = !actions.is_empty()
        && actions.len() == lease.allowed_actions.len()
        && actions.len() <= MAX_TASK_AUTHORIZATION_ACTIONS
        && actions.iter().all(TrustedTaskActionScope::validate);
    if !fields_match || !valid_time || !valid_actions {
        return Err(task_authorization_required(
            "the trusted task authorization lease is invalid or out of scope",
        ));
    }
    Ok(())
}

pub(crate) fn validate_authorization_id(value: &str) -> Result<(), HostError> {
    if value.is_empty()
        || value != value.trim()
        || value.chars().count() > MAX_TASK_AUTHORIZATION_ID_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(HostError::Protocol(format!(
            "task_authorization_id must contain 1..{MAX_TASK_AUTHORIZATION_ID_CHARS} printable characters without surrounding whitespace"
        )));
    }
    Ok(())
}

fn valid_browser_origin(value: &str) -> bool {
    let authority = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"));
    authority.is_some_and(|authority| {
        !authority.is_empty()
            && value.len() <= 2_048
            && !authority.contains('/')
            && !authority.contains('?')
            && !authority.contains('#')
            && !authority.contains('@')
    })
}

fn task_authorization_required(message: &str) -> HostError {
    HostError::coded_protocol(HostProtocolErrorCode::TaskAuthorizationRequired, message)
}

fn unix_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn digest(value: &impl Serialize, context: &str) -> Result<String, HostError> {
    let encoded = serde_json::to_vec(value)
        .map_err(|error| HostError::Protocol(format!("could not digest {context}: {error}")))?;
    Ok(format!("sha256:{:x}", Sha256::digest(encoded)))
}
