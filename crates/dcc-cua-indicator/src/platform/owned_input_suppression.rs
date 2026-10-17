//! A private request channel to one renderer. No HWNDs or peer enumeration
//! cross this boundary: only its window-owning thread changes presentation.
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::IndicatorError;

const ACK_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Requests {
    stopped: bool,
    holders: usize,
    hidden: bool,
}

#[derive(Default)]
pub(super) struct RendererSuppression {
    requests: Mutex<Requests>,
    changed: Condvar,
}

impl RendererSuppression {
    pub(super) fn acquire(
        self: &Arc<Self>,
        valid: impl Fn() -> bool,
    ) -> Result<Guard, IndicatorError> {
        self.acquire_until(valid, Instant::now() + ACK_TIMEOUT)
    }

    fn acquire_until(
        self: &Arc<Self>,
        valid: impl Fn() -> bool,
        deadline: Instant,
    ) -> Result<Guard, IndicatorError> {
        let mut requests = self.requests.lock().map_err(|_| unavailable())?;
        if requests.stopped || !valid() {
            return Err(unavailable());
        }
        requests.holders = requests.holders.checked_add(1).ok_or_else(unavailable)?;
        let guard = Guard(Arc::clone(self));
        loop {
            if requests.stopped || !valid() {
                drop(requests);
                return Err(unavailable());
            }
            if requests.hidden {
                return Ok(guard);
            }
            if Instant::now() >= deadline {
                drop(requests);
                return Err(IndicatorError::Backend(
                    "owned indicator input suppression was not acknowledged".into(),
                ));
            }
            // Recheck the owning session/interrupt even when a presenter stops
            // without acknowledging. The renderer itself keeps servicing Win32.
            let remaining = deadline.saturating_duration_since(Instant::now());
            requests = self
                .changed
                .wait_timeout(requests, remaining.min(Duration::from_millis(10)))
                .map_err(|_| unavailable())?
                .0;
        }
    }

    /// Called only by the exact owner thread with its locally owned windows.
    /// Keep normal presentation disabled for as long as any worker owns a lease.
    pub(super) fn hide_if_requested(
        &self,
        hide_and_confirm: impl FnOnce() -> bool,
    ) -> Result<bool, IndicatorError> {
        let mut requests = self.requests.lock().map_err(|_| unavailable())?;
        if requests.stopped {
            return Err(unavailable());
        }
        if requests.holders == 0 {
            return Ok(false);
        }
        if !hide_and_confirm() {
            return Err(IndicatorError::Backend(
                "the owned indicator surfaces could not be physically hidden".into(),
            ));
        }
        requests.hidden = true;
        self.changed.notify_all();
        Ok(true)
    }

    pub(super) fn stop(&self) {
        if let Ok(mut requests) = self.requests.lock() {
            requests.stopped = true;
            requests.hidden = false;
        }
        self.changed.notify_all();
    }
}

fn unavailable() -> IndicatorError {
    IndicatorError::Backend("the owning indicator renderer is unavailable for input".into())
}

/// Releasing a lease never calls ShowWindow. A live owner may present its own
/// surfaces on its next frame; a stopped owner destroys them and cannot revive.
pub(crate) struct Guard(Arc<RendererSuppression>);

impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut requests) = self.0.requests.lock() {
            requests.holders -= 1;
            if requests.holders == 0 {
                requests.hidden = false;
            }
        }
        self.0.changed.notify_all();
    }
}

#[path = "../tests/owned_input_suppression.rs"]
#[cfg(test)]
mod tests;
