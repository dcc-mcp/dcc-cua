//! Explicit temporary, non-activating preparation for passive pixel evidence.
//!
//! Preparation never grants content input or creates an actionable observation.
//! A real outstanding window call remains pending until the mutation worker
//! reports its return; revocation schedules rollback rather than cancelling it.

#[cfg(any(windows, test))]
mod journal;
#[cfg(windows)]
mod native;
#[cfg(windows)]
mod process;
#[cfg(windows)]
mod runtime;
#[cfg(any(windows, test))]
mod state;
#[cfg(test)]
mod tests;

#[cfg(windows)]
pub use native::read_capture_preparation_identity;
#[cfg(windows)]
pub use process::{
    CapturePreparationHandle, PreparedEvidenceFrameSource, PreparedEvidenceGuard,
    dispatch_capture_preparation,
};

pub use dcc_cua_protocol::capture_preparation::*;

/// Monotonic Windows uptime milliseconds from GetTickCount64, never UTC.
/// Trusted callers sample this before target lookup to bind absolute expiry.
#[cfg(windows)]
pub fn read_capture_preparation_clock_ms() -> u64 {
    runtime::ticks()
}

/// Pixels are passive evidence; this type has no input-token conversion.
#[cfg(windows)]
pub struct PassivePreparedFrame {
    pub capture: crate::VisibleWindowCapture,
    pub evidence_before: crate::ExactWindowPixelEvidence,
    pub evidence_after: crate::ExactWindowPixelEvidence,
    pub actual_foreground: bool,
    pub preparation_id: [u8; 16],
    /// Monotonic Windows uptime milliseconds from GetTickCount64, never UTC.
    pub captured_at_ms: u64,
}
