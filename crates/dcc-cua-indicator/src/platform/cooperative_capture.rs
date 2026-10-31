//! Same-banner capture may borrow the exact input leases without changing peers.
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::{Duration, Instant};

use super::capture_exclusion::{self, CrossProcessGate};
use crate::{IndicatorError, input_capture};

pub(crate) enum ExclusionResources {
    // Cursor restoration and its global gate precede renderer restoration.
    Input {
        _cursor: super::PlatformInputCursorGuard,
        _presentation: super::PlatformInputSuppressionGuard,
    },
    Capture {
        _guard: capture_exclusion::Guard,
    },
}

pub(super) type Pool = input_capture::Pool<ExclusionResources>;
pub(crate) type OwnedEpoch = input_capture::Owner<ExclusionResources>;

pub(crate) enum CaptureGuard {
    Exclusive {
        _guard: OwnedEpoch,
    },
    Borrowed {
        _guard: input_capture::Borrow<ExclusionResources>,
    },
}

pub(super) fn begin_input(
    pool: &Pool,
    active: &AtomicBool,
    presentation: super::PlatformInputSuppressionGuard,
) -> Result<OwnedEpoch, IndicatorError> {
    let cursor = capture_exclusion::begin_input_cursor(active)?;
    pool.publish(ExclusionResources::Input {
        _cursor: cursor,
        _presentation: presentation,
    })
}

pub(super) fn begin_capture(
    pool: &Arc<Pool>,
    active: &AtomicBool,
    allow_input_borrow: bool,
) -> Result<CaptureGuard, IndicatorError> {
    let deadline = Instant::now() + capture_exclusion::CAPTURE_EXCLUSION_TIMEOUT;
    loop {
        if !active.load(std::sync::atomic::Ordering::Acquire) {
            return Err(IndicatorError::CaptureExclusion(
                "owning capture session stopped".into(),
            ));
        }
        if let Some(guard) = pool.borrow()? {
            // The leases keep this renderer/cursor hidden. Observe peers only;
            // no capture request, suppression, hide or restore is issued here.
            if allow_input_borrow || matches!(guard.resources(), ExclusionResources::Capture { .. })
            {
                capture_exclusion::confirm_overlays_hidden()?;
                return Ok(CaptureGuard::Borrowed { _guard: guard });
            }
        }
        match CrossProcessGate::try_acquire() {
            Ok(gate) => {
                let guard = capture_exclusion::begin_with_gate(active, gate)?;
                let owner = pool.publish(ExclusionResources::Capture { _guard: guard })?;
                return Ok(CaptureGuard::Exclusive { _guard: owner });
            }
            Err(error) if Instant::now() >= deadline => {
                return Err(IndicatorError::CaptureExclusion(format!(
                    "timed out while serializing cross-process DCC-CUA overlay capture exclusion ({:?}, os={:?})",
                    error.kind(),
                    error.raw_os_error(),
                )));
            }
            Err(_) => thread::sleep(Duration::from_millis(1)),
        }
    }
}
