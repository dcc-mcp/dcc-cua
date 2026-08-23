use serde::Deserialize;

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
        Ok(())
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
