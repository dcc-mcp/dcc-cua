use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerUseInputStatus {
    Ready,
    Suspended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseInputReadiness {
    pub status: ComputerUseInputStatus,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ComputerUseInputReadiness {
    #[must_use]
    pub fn from_diagnostic(diagnostic: &Value) -> Self {
        let ready = diagnostic["success"] == true && diagnostic["input_ready"] == true;
        let code = if ready {
            diagnostic["code"].as_str()
        } else {
            diagnostic["input_code"]
                .as_str()
                .or_else(|| diagnostic["code"].as_str())
        }
        .unwrap_or(if ready {
            "interactive_desktop_ready"
        } else {
            "interactive_desktop_unavailable"
        });
        let reason = (!ready).then(|| {
            diagnostic["input_message"]
                .as_str()
                .or_else(|| diagnostic["message"].as_str())
                .unwrap_or("Windows input surface is unavailable")
                .to_owned()
        });
        Self {
            status: if ready {
                ComputerUseInputStatus::Ready
            } else {
                ComputerUseInputStatus::Suspended
            },
            code: code.to_owned(),
            reason,
        }
    }
}

/// Probe input readiness without activating a window or sending input.
#[must_use]
pub fn probe_input_readiness() -> ComputerUseInputReadiness {
    ComputerUseInputReadiness::from_diagnostic(&crate::interactive_desktop::diagnostic())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseInputTarget {
    pub session_id: String,
    pub process_id: u32,
    pub window_handle: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseSessionInputState {
    pub status: ComputerUseInputStatus,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Unix epoch time in milliseconds for the latest readiness sample.
    pub observed_at: u64,
    /// Per-session cursor of the last input transition. The page-level
    /// `latest_sequence` is the authoritative cursor across all event types.
    pub sequence: u64,
    pub target: ComputerUseInputTarget,
}

/// Availability of the exact PID/HWND target, independent from whether the
/// interactive desktop can currently accept input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerUseTargetStatus {
    Available,
    Minimized,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseTargetAvailability {
    pub status: ComputerUseTargetStatus,
    pub code: String,
    pub visible: bool,
    pub minimized: bool,
    pub foreground: bool,
}

impl ComputerUseTargetAvailability {
    #[must_use]
    pub fn from_window_state(state: &Value) -> Self {
        let minimized = state["minimized"]
            .as_bool()
            .or_else(|| state["is_minimized"].as_bool())
            .unwrap_or(false);
        let visible = state["visible"]
            .as_bool()
            .or_else(|| state["is_on_screen"].as_bool())
            .unwrap_or(false);
        let foreground = state["foreground"]
            .as_bool()
            .or_else(|| state["is_foreground"].as_bool())
            .unwrap_or(false);
        Self {
            status: if minimized {
                ComputerUseTargetStatus::Minimized
            } else if visible {
                ComputerUseTargetStatus::Available
            } else {
                ComputerUseTargetStatus::Unavailable
            },
            code: match (minimized, visible) {
                (true, _) => "target_minimized",
                (false, true) => "target_available",
                (false, false) => "target_unavailable",
            }
            .into(),
            visible,
            minimized,
            foreground,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseSessionTargetState {
    pub status: ComputerUseTargetStatus,
    pub code: String,
    pub visible: bool,
    pub minimized: bool,
    pub foreground: bool,
    /// Unix epoch time in milliseconds for the latest read-only target sample.
    pub observed_at: u64,
    /// Per-session cursor of the last target transition. The page-level
    /// `latest_sequence` is the authoritative cursor across all event types.
    pub sequence: u64,
    pub target: ComputerUseInputTarget,
}

impl ComputerUseSessionTargetState {
    #[must_use]
    pub fn initial(
        target: ComputerUseInputTarget,
        availability: ComputerUseTargetAvailability,
        observed_at: u64,
    ) -> Self {
        Self {
            status: availability.status,
            code: availability.code,
            visible: availability.visible,
            minimized: availability.minimized,
            foreground: availability.foreground,
            observed_at,
            sequence: 1,
            target,
        }
    }
}

impl ComputerUseSessionInputState {
    #[must_use]
    pub fn initial(
        target: ComputerUseInputTarget,
        readiness: ComputerUseInputReadiness,
        observed_at: u64,
    ) -> Self {
        Self {
            status: readiness.status,
            code: readiness.code,
            reason: readiness.reason,
            observed_at,
            sequence: 1,
            target,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerUseSessionEventKind {
    InputSuspended,
    InputResumed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseInputResumeRequirements {
    pub automatic_input: bool,
    pub exact_target_revalidation: bool,
    pub fresh_observation: bool,
    pub foreground_or_explicit_activation: bool,
    pub upstream_session_refresh_may_be_required: bool,
}

impl ComputerUseInputResumeRequirements {
    #[must_use]
    pub const fn safe_continuation() -> Self {
        Self {
            automatic_input: false,
            exact_target_revalidation: true,
            fresh_observation: true,
            foreground_or_explicit_activation: true,
            upstream_session_refresh_may_be_required: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseSessionEvent {
    #[serde(rename = "type")]
    pub kind: ComputerUseSessionEventKind,
    pub sequence: u64,
    pub previous: ComputerUseSessionInputState,
    pub current: ComputerUseSessionInputState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_requirements: Option<ComputerUseInputResumeRequirements>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerUseSessionTargetEventKind {
    TargetMinimized,
    TargetRestored,
    TargetUnavailable,
    TargetAvailable,
}

/// Machine-readable continuation contract for a minimized exact target.
///
/// This advertises an explicit operation; it never authorizes an automatic
/// focus change or input retry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseTargetRecoveryRequirements {
    pub automatic_input: bool,
    pub explicit_request_required: bool,
    pub operation: String,
    pub exact_target_revalidation: bool,
    pub fresh_observation: bool,
    pub foreground_validation: bool,
    pub blind_retry: bool,
}

impl ComputerUseTargetRecoveryRequirements {
    #[must_use]
    pub fn explicit_restore_activate() -> Self {
        Self {
            automatic_input: false,
            explicit_request_required: true,
            operation: "restore_activate".into(),
            exact_target_revalidation: true,
            fresh_observation: true,
            foreground_validation: true,
            blind_retry: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseSessionTargetEvent {
    #[serde(rename = "type")]
    pub kind: ComputerUseSessionTargetEventKind,
    pub sequence: u64,
    pub previous: ComputerUseSessionTargetState,
    pub current: ComputerUseSessionTargetState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_requirements: Option<ComputerUseTargetRecoveryRequirements>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_requirements: Option<ComputerUseInputResumeRequirements>,
}

/// One item in the single bounded per-session event stream. `untagged` keeps
/// the existing input event JSON stable while adding typed target events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ComputerUseSessionEventEnvelope {
    Input(ComputerUseSessionEvent),
    Target(ComputerUseSessionTargetEvent),
}

impl ComputerUseSessionEventEnvelope {
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        match self {
            Self::Input(event) => event.sequence,
            Self::Target(event) => event.sequence,
        }
    }
}

impl From<ComputerUseSessionEvent> for ComputerUseSessionEventEnvelope {
    fn from(event: ComputerUseSessionEvent) -> Self {
        Self::Input(event)
    }
}

impl From<ComputerUseSessionTargetEvent> for ComputerUseSessionEventEnvelope {
    fn from(event: ComputerUseSessionTargetEvent) -> Self {
        Self::Target(event)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseSessionEventsPage {
    pub after_sequence: u64,
    pub latest_sequence: u64,
    pub oldest_available_sequence: Option<u64>,
    pub resync_required: bool,
    pub timed_out: bool,
    pub current_state: ComputerUseSessionInputState,
    pub current_target_state: ComputerUseSessionTargetState,
    pub events: Vec<ComputerUseSessionEventEnvelope>,
}

#[cfg(test)]
mod tests;
