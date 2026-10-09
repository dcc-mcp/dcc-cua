//! Exact-instance temporary passive capture preparation contract.
use serde::{Deserialize, Serialize};

/// Constructor-owned filesystem and duration permission. Host requests cannot
/// nominate the journal directory or widen the affected window scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePreparationAuthorization {
    pub journal_directory: String,
    pub max_lifetime_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePreparationBeginRequest {
    pub window_state_id: String,
    pub lifetime_ms: u64,
}

impl CapturePreparationBeginRequest {
    pub fn validate(&self) -> Result<(), PreparationError> {
        if self.window_state_id.is_empty()
            || self.window_state_id.len() > 128
            || self.lifetime_ms == 0
            || self.lifetime_ms > MAX_PREPARATION_LIFETIME_MS
        {
            return Err(PreparationError::new(PreparationFailure::InvalidBinding));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

pub const MAX_PREPARATION_LIFETIME_MS: u64 = 30_000;
pub const MAX_AFFECTED_WINDOWS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationNativeInstance {
    pub process_creation_time_100ns: u64,
    pub window_thread_id: u32,
    pub window_class_hash: u64,
    pub owner_window_handle: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationExecutableIdentity {
    pub canonical_image_path: String,
    pub volume_serial_number: u64,
    pub file_id: [u8; 16],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationWindowIdentity {
    pub process_id: u32,
    pub window_handle: u64,
    pub native_instance: PreparationNativeInstance,
    pub executable: PreparationExecutableIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePreparationSpec {
    pub preparation_id: [u8; 16],
    pub target: PreparationWindowIdentity,
    /// Exact additional owner/owned identities, never a PID-wide permission.
    /// Empty permits only an independently verified ownerless/ownedless root.
    pub allowed_affected_scope: Vec<PreparationWindowIdentity>,
    pub lifetime_ms: u64,
    /// Fixed trusted authorization expiry in Windows GetTickCount64 milliseconds.
    /// This internal binding is sampled before target lookup; callers cannot extend it.
    pub authorization_deadline_ms: u64,
    /// Durable journal directory explicitly selected by the trusted owner.
    pub journal_directory: std::path::PathBuf,
}

impl CapturePreparationSpec {
    pub fn validate(&self) -> Result<(), PreparationError> {
        if self.preparation_id == [0; 16]
            || self.lifetime_ms == 0
            || self.lifetime_ms > MAX_PREPARATION_LIFETIME_MS
            || self.authorization_deadline_ms == 0
            || self.allowed_affected_scope.len() >= MAX_AFFECTED_WINDOWS
            || !self.journal_directory.is_absolute()
        {
            return Err(PreparationError::new(PreparationFailure::InvalidBinding));
        }
        let mut handles = std::collections::BTreeSet::new();
        for identity in std::iter::once(&self.target).chain(&self.allowed_affected_scope) {
            if identity.process_id == 0
                || identity.window_handle == 0
                || identity.native_instance.process_creation_time_100ns == 0
                || identity.native_instance.window_thread_id == 0
                || identity.native_instance.window_class_hash == 0
                || identity.executable.canonical_image_path.is_empty()
                || identity.executable.file_id == [0; 16]
                || !handles.insert(identity.window_handle)
            {
                return Err(PreparationError::new(PreparationFailure::InvalidBinding));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparationPhase {
    PendingPromotion,
    Active,
    RestorePending,
    CleanupUnknown,
    Restored,
    Refused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparationFailure {
    InvalidBinding,
    AuthorizationDenied,
    ParentUnavailable,
    SupervisorNotIndependent,
    TargetUnavailable,
    TargetOutsideDesktop,
    IdentityChanged,
    AffectedScopeChanged,
    EnumerationIncomplete,
    DesktopUnavailable,
    GeometryChanged,
    AnchorChanged,
    MutationFailed,
    ReadbackFailed,
    JournalFailed,
    GateBusy,
    Expired,
    Stopped,
    ParentDied,
    Disconnected,
    WorkerLost,
    ProtocolMismatch,
    NotActive,
    CaptureFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("capture preparation refused: {reason:?} (OS error {os_error:?})")]
#[serde(deny_unknown_fields)]
pub struct PreparationError {
    pub reason: PreparationFailure,
    pub os_error: Option<i32>,
}

impl PreparationError {
    pub const fn new(reason: PreparationFailure) -> Self {
        Self {
            reason,
            os_error: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationAnchors {
    pub above: Option<PreparationWindowIdentity>,
    pub below: Option<PreparationWindowIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedWindowState {
    pub identity: PreparationWindowIdentity,
    pub topmost: bool,
    pub bounds: [i32; 4],
    pub visible_bounds: [i32; 4],
    pub dpi: u32,
    pub visible: bool,
    pub minimized: bool,
    /// Actual readback only; preparation never promises keyboard focus.
    pub foreground: bool,
    pub anchors: PreparationAnchors,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparationMutationKind {
    Promote,
    Restore,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationMutationReceipt {
    pub sequence: u64,
    pub kind: PreparationMutationKind,
    /// Monotonic Windows GetTickCount64 uptime milliseconds, not UTC.
    pub returned_at_ms: u64,
    pub api_success: bool,
    pub os_error: Option<i32>,
    pub readback: Vec<PreparedWindowState>,
    pub failure: Option<PreparationError>,
    pub native_calls: Vec<PreparationNativeCallReceipt>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationNativeCallReceipt {
    pub window_handle: u64,
    pub insert_after: i64,
    pub api_success: bool,
    pub os_error: Option<i32>,
    /// Monotonic Windows GetTickCount64 uptime milliseconds at actual return.
    pub returned_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePreparationStatus {
    pub preparation_id: [u8; 16],
    pub phase: PreparationPhase,
    /// Fixed deadline in monotonic Windows GetTickCount64 uptime milliseconds.
    pub deadline_ms: u64,
    pub pending_sequence: Option<u64>,
    pub capture_revoked: bool,
    pub cleanup_verified: bool,
    pub original: Vec<PreparedWindowState>,
    pub last_mutation: Option<PreparationMutationReceipt>,
    pub failure: Option<PreparationError>,
    pub journal_path: std::path::PathBuf,
    pub affected_readback: Vec<PreparedWindowState>,
    pub last_completed_sequence: Option<u64>,
}
