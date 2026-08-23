use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{HostAction, HostError};

pub const TRUSTED_ACTION_CONFIRMATION_SCHEMA: &str =
    "dcc-cua-trusted-action-confirmation-request-v2";

/// Exact, content-bounded request delivered only to a trusted embedding host.
///
/// This type is never accepted from Host IPC. A new request is created for one
/// action attempt and its digest binds the decision to the current evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrustedActionConfirmationRequest {
    pub schema: String,
    pub request_id: String,
    pub session_id: String,
    pub task_grant_id: String,
    pub window_capability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_process_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_window_handle: Option<u64>,
    pub observation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accessibility_state_id: Option<String>,
    pub intent: String,
    pub action: Value,
    pub request_digest: String,
}

#[derive(Serialize)]
struct UnsignedConfirmationRequest<'a> {
    schema: &'a str,
    request_id: &'a str,
    session_id: &'a str,
    task_grant_id: &'a str,
    window_capability: &'a str,
    target_process_id: Option<u32>,
    target_window_handle: Option<u64>,
    observation_id: &'a str,
    accessibility_state_id: Option<&'a str>,
    intent: &'a str,
    action: &'a Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConfirmationWindowIdentity {
    pub process_id: u32,
    pub window_handle: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ConfirmationBinding<'a> {
    session_id: &'a str,
    task_grant_id: &'a str,
    window_capability: &'a str,
    target: Option<ConfirmationWindowIdentity>,
    observation_id: &'a str,
    accessibility_state_id: Option<&'a str>,
}

impl<'a> ConfirmationBinding<'a> {
    pub(crate) fn window(
        session_id: &'a str,
        task_grant_id: &'a str,
        window_capability: &'a str,
        target: ConfirmationWindowIdentity,
        observation_id: &'a str,
        accessibility_state_id: Option<&'a str>,
    ) -> Self {
        Self {
            session_id,
            task_grant_id,
            window_capability,
            target: Some(target),
            observation_id,
            accessibility_state_id,
        }
    }

    fn desktop(
        session_id: &'a str,
        task_grant_id: &'a str,
        desktop_capability: &'a str,
        observation_id: &'a str,
    ) -> Self {
        Self {
            session_id,
            task_grant_id,
            window_capability: desktop_capability,
            target: None,
            observation_id,
            accessibility_state_id: None,
        }
    }
}

impl TrustedActionConfirmationRequest {
    pub(crate) fn for_bound_window_action_value(
        binding: ConfirmationBinding<'_>,
        intent: &str,
        action: Value,
    ) -> Result<Self, HostError> {
        Self::new_value(binding, intent, action)
    }

    pub(crate) fn for_desktop_action(
        session_id: &str,
        task_grant_id: &str,
        desktop_capability: &str,
        observation_id: &str,
        action: &HostAction,
    ) -> Result<Self, HostError> {
        Self::new(
            ConfirmationBinding::desktop(
                session_id,
                task_grant_id,
                desktop_capability,
                observation_id,
            ),
            action,
        )
    }

    fn new(binding: ConfirmationBinding<'_>, action: &HostAction) -> Result<Self, HostError> {
        let action_value = serde_json::to_value(action).map_err(|error| {
            HostError::Protocol(format!("could not bind action confirmation: {error}"))
        })?;
        Self::new_value(binding, &action.intent, action_value)
    }

    fn new_value(
        binding: ConfirmationBinding<'_>,
        intent: &str,
        action_value: Value,
    ) -> Result<Self, HostError> {
        let ConfirmationBinding {
            session_id,
            task_grant_id,
            window_capability,
            target,
            observation_id,
            accessibility_state_id,
        } = binding;
        let request_id = Uuid::new_v4().to_string();
        let target_process_id = target.map(|identity| identity.process_id);
        let target_window_handle = target.map(|identity| identity.window_handle);
        let unsigned = UnsignedConfirmationRequest {
            schema: TRUSTED_ACTION_CONFIRMATION_SCHEMA,
            request_id: &request_id,
            session_id,
            task_grant_id,
            window_capability,
            target_process_id,
            target_window_handle,
            observation_id,
            accessibility_state_id,
            intent,
            action: &action_value,
        };
        let encoded = serde_json::to_vec(&unsigned).map_err(|error| {
            HostError::Protocol(format!("could not digest action confirmation: {error}"))
        })?;
        let request_digest = format!("sha256:{:x}", Sha256::digest(encoded));
        Ok(Self {
            schema: TRUSTED_ACTION_CONFIRMATION_SCHEMA.to_owned(),
            request_id,
            session_id: session_id.to_owned(),
            task_grant_id: task_grant_id.to_owned(),
            window_capability: window_capability.to_owned(),
            target_process_id,
            target_window_handle,
            observation_id: observation_id.to_owned(),
            accessibility_state_id: accessibility_state_id.map(str::to_owned),
            intent: intent.to_owned(),
            action: action_value,
            request_digest,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustedActionConfirmationAction {
    Allow,
    Deny,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TrustedActionConfirmationDecision {
    pub action: TrustedActionConfirmationAction,
    pub request_digest: String,
}

#[derive(Debug, Error)]
#[error("trusted action confirmation host failed: {reason}")]
pub struct TrustedActionConfirmationHostError {
    pub reason: String,
}

/// Constructor-owned confirmation boundary. Implementations must obtain an
/// explicit user decision and must not expose this callback through Host IPC.
#[async_trait]
pub trait TrustedActionConfirmationHost: Send + Sync {
    async fn confirm(
        &self,
        request: TrustedActionConfirmationRequest,
    ) -> Result<TrustedActionConfirmationDecision, TrustedActionConfirmationHostError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionConfirmationOutcome {
    Allowed,
    Required,
    Denied,
    Cancelled,
    TaskAuthorizationRequired,
    TaskAuthorizationOutOfScope,
    TaskAuthorizationExpired,
    TaskAuthorizationRevoked,
}

pub(crate) async fn authorize_action_confirmation(
    host: Option<&dyn TrustedActionConfirmationHost>,
    grant_allows_confirmation: bool,
    request: TrustedActionConfirmationRequest,
) -> ActionConfirmationOutcome {
    if !grant_allows_confirmation {
        return ActionConfirmationOutcome::Required;
    }
    let Some(host) = host else {
        return ActionConfirmationOutcome::Required;
    };
    let expected_digest = request.request_digest.clone();
    let Ok(decision) = host.confirm(request).await else {
        return ActionConfirmationOutcome::Required;
    };
    if decision.request_digest != expected_digest {
        return ActionConfirmationOutcome::Required;
    }
    match decision.action {
        TrustedActionConfirmationAction::Allow => ActionConfirmationOutcome::Allowed,
        TrustedActionConfirmationAction::Deny => ActionConfirmationOutcome::Denied,
        TrustedActionConfirmationAction::Cancel => ActionConfirmationOutcome::Cancelled,
    }
}
