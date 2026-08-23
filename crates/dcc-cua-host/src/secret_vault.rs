use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use dcc_cua_browser::{BrowserTypeRequest, ResolvedBrowserTypeRequest};
use dcc_cua_core::{ComputerUseClipboardWriteRequest, ComputerUseError, ComputerUseErrorCode};
use dcc_cua_protocol::validate_secret_handle as validate_protocol_secret_handle;
use serde_json::{Value, json};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::action_confirmation::{
    ActionConfirmationOutcome, ConfirmationBinding, ConfirmationWindowIdentity,
    TrustedActionConfirmationRequest,
};
use crate::request_contract::action_confirmation_refusal;
use crate::task_authorization::authorize_window_confirmation;
use crate::{HostError, HostProtocolErrorCode, HostSession, TrustedActionConfirmationHost};

const MAX_SECRET_VALUE_CHARS: usize = 4096;

pub(crate) fn validate_secret_handle(handle: &str) -> Result<(), HostSecretVaultError> {
    validate_protocol_secret_handle(handle).map_err(|_| HostSecretVaultError::InvalidHandle)
}

/// One short-lived secret value. Debug output is always redacted and the
/// owned buffer is zeroized when this wrapper leaves scope.
#[must_use]
pub struct SecretValue(Zeroizing<String>);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Result<Self, HostSecretVaultError> {
        let value = value.into();
        if value.is_empty() || value.chars().count() > MAX_SECRET_VALUE_CHARS {
            return Err(HostSecretVaultError::InvalidValue);
        }
        Ok(Self(Zeroizing::new(value)))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum HostSecretVaultError {
    #[error("secret handle is invalid")]
    InvalidHandle,
    #[error("secret value is invalid")]
    InvalidValue,
    #[error("secret was not found")]
    NotFound,
    #[error("secret vault is unavailable")]
    Unavailable,
}

/// Constructor-owned secret boundary. Handles cross Host IPC, values do not.
#[async_trait]
pub trait HostSecretVault: Send + Sync {
    async fn resolve(&self, handle: &str) -> Result<SecretValue, HostSecretVaultError>;

    async fn store(&self, handle: &str, value: SecretValue) -> Result<(), HostSecretVaultError>;
}

/// Constructor-owned security services that cannot be supplied or replaced by
/// a Host IPC request.
#[derive(Clone, Default)]
pub struct HostSecurityServices {
    pub(crate) confirmation_host: Option<Arc<dyn TrustedActionConfirmationHost>>,
    pub(crate) task_authorization_host: Option<Arc<dyn crate::TrustedTaskAuthorizationHost>>,
    pub(crate) secret_vault: Option<Arc<dyn HostSecretVault>>,
}

impl HostSecurityServices {
    #[must_use]
    pub fn with_confirmation_host(mut self, host: Arc<dyn TrustedActionConfirmationHost>) -> Self {
        self.confirmation_host = Some(host);
        self
    }

    #[must_use]
    pub fn with_task_authorization_host(
        mut self,
        host: Arc<dyn crate::TrustedTaskAuthorizationHost>,
    ) -> Self {
        self.task_authorization_host = Some(host);
        self
    }

    #[must_use]
    pub fn with_secret_vault(mut self, vault: Arc<dyn HostSecretVault>) -> Self {
        self.secret_vault = Some(vault);
        self
    }
}

#[derive(Clone, Copy)]
pub(crate) struct BoundSecretRequest<'a> {
    session_id: &'a str,
    task_grant_id: &'a str,
    window_capability: &'a str,
}

impl<'a> BoundSecretRequest<'a> {
    pub(crate) fn new(
        session_id: &'a str,
        task_grant_id: &'a str,
        window_capability: &'a str,
    ) -> Self {
        Self {
            session_id,
            task_grant_id,
            window_capability,
        }
    }

    fn confirmation_binding<'b>(
        &'b self,
        host: &'b HostSession,
        observation_id: &'b str,
        accessibility_state_id: Option<&'b str>,
    ) -> ConfirmationBinding<'b> {
        ConfirmationBinding::window(
            self.session_id,
            self.task_grant_id,
            self.window_capability,
            ConfirmationWindowIdentity {
                process_id: host.target_process_id,
                window_handle: host.target_window_handle,
            },
            observation_id,
            accessibility_state_id,
        )
    }
}

pub(crate) enum BrowserTypeResolution {
    Resolved(ResolvedBrowserTypeRequest),
    Refused(Value),
}

pub(crate) async fn resolve_browser_type_request(
    host: &HostSession,
    security_services: &HostSecurityServices,
    binding: BoundSecretRequest<'_>,
    request: BrowserTypeRequest,
) -> Result<BrowserTypeResolution, HostError> {
    host.browser.validate_type_request(&request)?;
    let secret = if let Some(handle) = request.secret_handle() {
        let action = json!({
            "method": "browser_type",
            "request": &request,
            "browser_origin": host.browser.latest_origin(),
        });
        let confirmation = TrustedActionConfirmationRequest::for_bound_window_action_value(
            binding.confirmation_binding(host, request.snapshot_id(), Some(request.tab_id())),
            "credential_input",
            action,
        )?;
        let outcome = authorize_window_confirmation(security_services, host, confirmation).await;
        if outcome != ActionConfirmationOutcome::Allowed {
            return Ok(BrowserTypeResolution::Refused(
                action_confirmation_refusal(outcome).0,
            ));
        }
        Some(
            require_secret_vault(security_services)?
                .resolve(handle)
                .await
                .map_err(secret_vault_error)?,
        )
    } else {
        None
    };
    let request = request.resolve(secret.as_ref().map(SecretValue::expose))?;
    Ok(BrowserTypeResolution::Resolved(request))
}

pub(crate) async fn capture_clipboard_secret(
    host: &mut HostSession,
    security_services: &HostSecurityServices,
    binding: BoundSecretRequest<'_>,
    observation_id: &str,
    secret_handle: &str,
) -> Result<Value, HostError> {
    validate_secret_handle(secret_handle).map_err(secret_vault_error)?;
    if !host.allow_clipboard_read {
        return Err(HostError::coded_protocol(
            HostProtocolErrorCode::ClipboardReadNotGranted,
            "clipboard read is not granted",
        ));
    }
    if !host.allow_clipboard_write {
        return Err(HostError::coded_protocol(
            HostProtocolErrorCode::ClipboardWriteNotGranted,
            "clipboard write is not granted",
        ));
    }
    if host.latest_observation_id.as_deref() != Some(observation_id) {
        return Err(HostError::ComputerUse(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "clipboard secret capture requires the latest exact-window observation",
        )));
    }
    let action = json!({
        "action": "clipboard_capture_secret",
        "secret_handle": secret_handle,
        "clear_clipboard_after_store": true,
    });
    let confirmation = TrustedActionConfirmationRequest::for_bound_window_action_value(
        binding.confirmation_binding(
            host,
            observation_id,
            host.latest_accessibility_state_id.as_deref(),
        ),
        "credential_capture",
        action,
    )?;
    let outcome = authorize_window_confirmation(security_services, host, confirmation).await;
    if outcome != ActionConfirmationOutcome::Allowed {
        return Ok(action_confirmation_refusal(outcome).0);
    }

    let vault = require_secret_vault(security_services)?;
    let result = host.session.clipboard_read(true).await;
    let result = host.finish_observation_sensitive_attempt(result)?;
    let secret = extract_clipboard_secret(result).map_err(secret_vault_error)?;
    vault
        .store(secret_handle, secret)
        .await
        .map_err(secret_vault_error)?;

    let clear = host
        .session
        .clipboard_write(&ComputerUseClipboardWriteRequest {
            text: Some(String::new()),
            image_path: None,
            file_path: None,
        })
        .await;
    let clipboard_cleared = host.finish_observation_sensitive_attempt(clear).is_ok();
    Ok(json!({
        "type": "clipboard_secret_captured",
        "session_id": binding.session_id,
        "secret_handle": secret_handle,
        "clipboard_cleared": clipboard_cleared,
    }))
}

pub(crate) fn secret_vault_error(error: HostSecretVaultError) -> HostError {
    let (code, message) = match error {
        HostSecretVaultError::NotFound => (
            HostProtocolErrorCode::SecretNotFound,
            "the requested secret handle was not found",
        ),
        HostSecretVaultError::InvalidHandle | HostSecretVaultError::InvalidValue => (
            HostProtocolErrorCode::SecretCaptureFailed,
            "the secret input did not satisfy the bounded vault contract",
        ),
        HostSecretVaultError::Unavailable => (
            HostProtocolErrorCode::SecretVaultUnavailable,
            "the constructor-owned secret vault is unavailable",
        ),
    };
    HostError::coded_protocol(code, message)
}

pub(crate) fn require_secret_vault(
    security_services: &HostSecurityServices,
) -> Result<&dyn HostSecretVault, HostError> {
    security_services
        .secret_vault
        .as_deref()
        .ok_or_else(|| secret_vault_error(HostSecretVaultError::Unavailable))
}

pub(crate) fn extract_clipboard_secret(
    mut value: Value,
) -> Result<SecretValue, HostSecretVaultError> {
    let structured = value
        .get_mut("structuredContent")
        .and_then(Value::as_object_mut)
        .ok_or(HostSecretVaultError::InvalidValue)?;
    if structured.get("supported").and_then(Value::as_bool) != Some(true)
        || structured.get("privacy_sensitive").and_then(Value::as_bool) != Some(true)
        || structured
            .get("content_redacted_from_telemetry")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err(HostSecretVaultError::InvalidValue);
    }
    let text = match structured
        .get_mut("text")
        .map(std::mem::take)
        .ok_or(HostSecretVaultError::InvalidValue)?
    {
        Value::String(text) => text,
        _ => return Err(HostSecretVaultError::InvalidValue),
    };
    SecretValue::new(text)
}
