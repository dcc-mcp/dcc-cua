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
    #[error("Windows UI Automation snapshot is stale: {0}")]
    StaleSnapshot(String),
    #[error("Windows UI Automation denied the request: {0}")]
    PermissionDenied(String),
    #[error("Windows UI Automation action is invalid: {0}")]
    InvalidAction(String),
    #[error("Windows UI Automation backend failed: {0}")]
    BackendUnavailable(String),
    #[error("Windows refused exact-window foreground activation: {reason}")]
    ForegroundActivationRefused {
        reason: String,
        background_delivery_viable: bool,
        suggested_delivery_mode: Option<String>,
    },
}
