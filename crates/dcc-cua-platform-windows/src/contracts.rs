use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiaTarget {
    pub process_id: u32,
    pub window_handle: u64,
}

/// One HWND/PID identity sampled from the interactive Windows desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsWindowIdentity {
    pub window_handle: u64,
    pub process_id: u32,
}

/// How the foreground window sampled after button-down relates to the grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsForegroundRelation {
    ExactTarget,
    SameProcess,
    ForeignProcess,
    NoForeground,
}

/// Mouse button whose system state is sampled after synthetic button-down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsPointerButton {
    Left,
    Right,
    Middle,
}

/// Typed, best-effort evidence gathered immediately after `SendInput` DOWN.
///
/// `async_button_down` and an exact foreground HWND are the only generic
/// prerequisites for continuing a scoped drag. Mouse capture is positive
/// consumer evidence when observed, but its absence is inconclusive because
/// applications are not required to call `SetCapture` for an in-window drag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsRawInputSnapshot {
    pub async_button_down: bool,
    pub target: WindowsWindowIdentity,
    pub foreground: Option<WindowsWindowIdentity>,
    pub foreground_relation: WindowsForegroundRelation,
    pub target_thread_capture: Option<WindowsWindowIdentity>,
    pub capture_query_succeeded: bool,
    pub capture_owned_by_target_process: bool,
}

impl WindowsRawInputSnapshot {
    #[must_use]
    pub fn allows_drag_path(&self) -> bool {
        self.async_button_down && self.foreground_relation == WindowsForegroundRelation::ExactTarget
    }
}

/// A finite phase in the existing exact-window activation sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsForegroundActivationPhase {
    RestoreWindow,
    InitialForegroundRequest,
    NonTopmostRaise,
    RaisedForegroundRequest,
    AttachForegroundInput,
    AttachTargetInput,
    BringTargetToTop,
    AttachedForegroundRequest,
    DetachTargetInput,
    DetachForegroundInput,
    RestoreTargetFrame,
}

/// The actual BOOL returned by one existing activation call, without UI content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsForegroundActivationAttempt {
    pub phase: WindowsForegroundActivationPhase,
    pub api_return: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_thread_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_error: Option<u32>,
}

#[cfg(any(windows, test))]
impl WindowsForegroundActivationAttempt {
    pub(crate) fn new(
        phase: WindowsForegroundActivationPhase,
        api_return: i32,
        related_thread_id: Option<u32>,
        documented_os_error: Option<u32>,
    ) -> Self {
        use WindowsForegroundActivationPhase as Phase;
        let supports_last_error = matches!(
            phase,
            Phase::NonTopmostRaise
                | Phase::RestoreTargetFrame
                | Phase::AttachForegroundInput
                | Phase::AttachTargetInput
                | Phase::DetachTargetInput
                | Phase::DetachForegroundInput
        );
        Self {
            phase,
            api_return,
            related_thread_id,
            os_error: (api_return == 0 && supports_last_error)
                .then_some(documented_os_error)
                .flatten(),
        }
    }
}

/// Content-free observations of activation attempts, not a root-cause claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsForegroundActivationDiagnostic {
    pub caller_process_id: u32,
    pub caller_thread_id: u32,
    pub target: WindowsWindowIdentity,
    pub target_thread_id: u32,
    pub target_root_window_handle: u64,
    pub target_owner_window_handle: u64,
    pub initial_foreground: Option<WindowsWindowIdentity>,
    pub final_foreground: Option<WindowsWindowIdentity>,
    #[serde(deserialize_with = "deserialize_activation_attempts")]
    pub attempts: Vec<WindowsForegroundActivationAttempt>,
    #[serde(deserialize_with = "deserialize_activation_poll_count")]
    pub foreground_poll_count: u8,
}

fn deserialize_activation_attempts<'de, D>(
    deserializer: D,
) -> Result<Vec<WindowsForegroundActivationAttempt>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let attempts = Vec::<WindowsForegroundActivationAttempt>::deserialize(deserializer)?;
    if attempts.len() > 16 {
        return Err(serde::de::Error::custom(
            "foreground activation has more than 16 API attempts",
        ));
    }
    Ok(attempts)
}

fn deserialize_activation_poll_count<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let count = u8::deserialize(deserializer)?;
    if count > 20 {
        return Err(serde::de::Error::custom(
            "foreground activation has more than 20 polls",
        ));
    }
    Ok(count)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiaAction {
    pub action: String,
    pub element_index: Option<u32>,
    pub element_token: Option<String>,
    pub text: Option<String>,
    pub checked: Option<bool>,
    pub delivery_mode: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum UiaError {
    #[error("Windows UI Automation fallback is unavailable on this platform")]
    Unsupported,
    #[error("Windows UI Automation target is invalid: {0}")]
    InvalidTarget(String),
    #[error("Windows UI Automation target is minimized: {0}")]
    TargetMinimized(String),
    #[error("Windows UI Automation snapshot is stale: {0}")]
    StaleSnapshot(String),
    #[error("Windows UI Automation denied the request: {0}")]
    PermissionDenied(String),
    #[error("Windows interactive desktop is unavailable during {stage}: {reason}")]
    InteractiveDesktopUnavailable { stage: String, reason: String },
    #[error("Windows UI Automation action is invalid: {0}")]
    InvalidAction(String),
    #[error("the exact window has no usable Windows UI Automation provider: {0}")]
    NoAccessibilityProvider(String),
    #[error("Windows UI Automation backend failed: {0}")]
    BackendUnavailable(String),
    #[error("Windows UI Automation operation failed: {0}")]
    OperationFailed(String),
    #[error(
        "Windows UI Automation worker protocol mismatch: expected {expected}, received {actual:?}"
    )]
    ProtocolMismatch { expected: u32, actual: Option<u64> },
    #[error("Windows refused exact-window foreground activation: {reason}")]
    ForegroundActivationRefused {
        reason: String,
        background_delivery_viable: bool,
        suggested_delivery_mode: Option<String>,
        diagnostic: Option<Box<WindowsForegroundActivationDiagnostic>>,
    },
}
