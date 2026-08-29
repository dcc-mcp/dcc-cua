use serde::Deserialize;

use dcc_cua_core::ComputerUseOwnedBrowserLaunchSpec;

use super::HostError;

pub const MAX_APPLICATION_LABEL_CHARS: usize = 80;
pub const MAX_TASK_GRANT_ID_CHARS: usize = 128;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TaskGrant {
    pub(super) task_grant_id: String,
    pub(super) application_label: String,
    #[serde(default)]
    pub(super) process_id: Option<u32>,
    #[serde(default)]
    pub(super) window_handle: Option<u64>,
    #[serde(default)]
    pub(super) window_title: Option<String>,
    #[serde(default)]
    pub(super) owned_browser_launch: Option<ComputerUseOwnedBrowserLaunchSpec>,
    #[serde(default)]
    pub(super) allowed_browser_origins: Vec<String>,
    #[serde(default)]
    pub(super) allow_raw_input: bool,
    #[serde(default)]
    pub(super) allow_app_launch: bool,
    #[serde(default)]
    pub(super) allow_app_terminate: bool,
    #[serde(default)]
    pub(super) allow_clipboard_read: bool,
    #[serde(default)]
    pub(super) allow_clipboard_write: bool,
    #[serde(default)]
    pub(super) allow_recording: bool,
    #[serde(default)]
    pub(super) showcase_output_dir: Option<String>,
    #[serde(default)]
    pub(super) allow_live_observation: bool,
    #[serde(default)]
    pub(super) allow_browser_input: bool,
    #[serde(default)]
    pub(super) allow_browser_prepare: bool,
    #[serde(default)]
    pub(super) allow_browser_download: bool,
    #[serde(default)]
    pub(super) allow_native_tool: bool,
    #[serde(default)]
    pub(super) allow_menu_invoke: bool,
    #[serde(default)]
    pub(super) allow_session_escalation: bool,
    #[serde(default)]
    pub(super) allow_trusted_confirmation: bool,
    #[serde(default)]
    pub(super) task_authorization_id: Option<String>,
    #[serde(default)]
    pub(super) task_authorization_window_capability: Option<String>,
}

impl TaskGrant {
    pub(super) fn validate_identity(&self) -> Result<(), HostError> {
        validate_identity_field(
            &self.task_grant_id,
            MAX_TASK_GRANT_ID_CHARS,
            "task_grant_id",
        )?;
        validate_identity_field(
            &self.application_label,
            MAX_APPLICATION_LABEL_CHARS,
            "application_label",
        )?;
        if let Some(authorization_id) = self.task_authorization_id.as_deref() {
            crate::task_authorization::validate_authorization_id(authorization_id)?;
        }
        match (
            self.task_authorization_id.as_deref(),
            self.task_authorization_window_capability.as_deref(),
        ) {
            (Some(_), Some(capability)) => {
                validate_identity_field(capability, 512, "task_authorization_window_capability")?;
            }
            (Some(_), None) => {
                return Err(HostError::Protocol(
                    "task_authorization_id requires task_authorization_window_capability".into(),
                ));
            }
            (None, Some(_)) => {
                return Err(HostError::Protocol(
                    "task_authorization_window_capability requires task_authorization_id".into(),
                ));
            }
            (None, None) => {}
        }
        if self.owned_browser_launch.is_some() {
            if self.process_id.is_some()
                || self.window_handle.is_some()
                || self.window_title.is_some()
                || self.allow_app_launch
                || self.allow_browser_prepare
            {
                return Err(HostError::Protocol(
                    "owned_browser_launch cannot nominate a target, launch an app, or grant browser_prepare".into(),
                ));
            }
            if self.task_authorization_id.is_none() {
                return Err(HostError::Protocol(
                    "owned_browser_launch requires trusted task authorization".into(),
                ));
            }
        }
        if self.showcase_output_dir.is_some() && !self.allow_recording {
            return Err(HostError::Protocol(
                "showcase_output_dir requires allow_recording".into(),
            ));
        }
        if self.allowed_browser_origins.len() > 32
            || self
                .allowed_browser_origins
                .iter()
                .any(|origin| !crate::task_authorization::valid_browser_origin(origin))
            || self
                .allowed_browser_origins
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.allowed_browser_origins.len()
        {
            return Err(HostError::Protocol(
                "allowed_browser_origins must contain at most 32 unique exact HTTP(S) origins"
                    .into(),
            ));
        }
        Ok(())
    }

    pub(super) fn reject_task_authorization(&self, route: &str) -> Result<(), HostError> {
        if self.task_authorization_id.is_some() {
            return Err(HostError::coded_protocol(
                crate::HostProtocolErrorCode::TaskAuthorizationDenied,
                format!("task authorization cannot authorize the global {route} route"),
            ));
        }
        Ok(())
    }

    pub(super) fn reject_task_authorization_activation(
        &self,
        activate_before: bool,
    ) -> Result<(), HostError> {
        if self.task_authorization_id.is_some() && activate_before {
            return Err(HostError::coded_protocol(
                crate::HostProtocolErrorCode::TaskAuthorizationDenied,
                "task-authorized sessions cannot activate a window during open_session",
            ));
        }
        Ok(())
    }

    pub(super) fn task_authorization_preflight_target(
        &self,
    ) -> Result<Option<crate::ConfirmationWindowIdentity>, HostError> {
        if self.task_authorization_id.is_none() || self.owned_browser_launch.is_some() {
            return Ok(None);
        }
        match (self.process_id, self.window_handle) {
            (Some(process_id), Some(window_handle)) if process_id != 0 && window_handle != 0 => {
                Ok(Some(crate::ConfirmationWindowIdentity {
                    process_id,
                    window_handle,
                }))
            }
            _ => Err(HostError::coded_protocol(
                crate::HostProtocolErrorCode::TaskAuthorizationRequired,
                "task authorization requires an exact nonzero process_id and window_handle before opening the session",
            )),
        }
    }
}

fn validate_identity_field(value: &str, max_chars: usize, field: &str) -> Result<(), HostError> {
    if value.is_empty()
        || value != value.trim()
        || value.chars().count() > max_chars
        || value.chars().any(char::is_control)
    {
        return Err(HostError::Protocol(format!(
            "{field} must contain 1..{max_chars} printable characters without surrounding whitespace"
        )));
    }
    Ok(())
}
