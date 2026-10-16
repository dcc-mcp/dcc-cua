//! Constructor-owned admission fence shared by capture, encoder and issuer.
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Serializes revocation with new frame admission. Finalizing an already
/// admitted frame/file remains cleanup and does not require a live lease.
pub struct RecordingAuthorization {
    revoked: Mutex<bool>,
    expires_at_unix_ms: u64,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    monotonic_deadline: Option<Instant>,
}

pub struct RecordingAdmission<'a> {
    _revoked: MutexGuard<'a, bool>,
}

impl RecordingAuthorization {
    #[must_use]
    pub fn new(expires_at_unix_ms: u64) -> Self {
        let mut authorization = Self::with_clock(
            expires_at_unix_ms,
            Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(u64::MAX, |elapsed| {
                        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
                    })
            }),
        );
        authorization.monotonic_deadline = Instant::now().checked_add(Duration::from_millis(
            expires_at_unix_ms.saturating_sub((authorization.clock)()),
        ));
        authorization
    }

    pub(crate) fn with_clock(
        expires_at_unix_ms: u64,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            revoked: Mutex::new(false),
            expires_at_unix_ms,
            clock,
            monotonic_deadline: None,
        }
    }

    /// On return no later admission can pass this fence. In-flight work that
    /// acquired the fence first finishes before revocation returns.
    pub fn revoke(&self) {
        if let Ok(mut revoked) = self.revoked.lock() {
            *revoked = true;
        }
    }

    pub fn admit(&self) -> Result<RecordingAdmission<'_>, &'static str> {
        let mut revoked = self
            .revoked
            .lock()
            .map_err(|_| "recording authorization poisoned")?;
        if *revoked
            || (self.clock)() >= self.expires_at_unix_ms
            || self
                .monotonic_deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            *revoked = true;
            return Err("recording task authorization expired or was revoked");
        }
        Ok(RecordingAdmission { _revoked: revoked })
    }

    pub fn with_active<T>(&self, work: impl FnOnce() -> T) -> Result<T, &'static str> {
        let _admission = self.admit()?;
        Ok(work())
    }
}

#[cfg(test)]
mod tests;
