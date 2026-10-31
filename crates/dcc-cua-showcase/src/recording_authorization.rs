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
    parent: Option<Arc<RecordingAuthorization>>,
}

pub struct RecordingAdmission<'a> {
    _revoked: MutexGuard<'a, bool>,
    authorization: &'a RecordingAuthorization,
    _parent: Option<Box<RecordingAdmission<'a>>>,
}

impl RecordingAdmission<'_> {
    /// Recheck expiry after a bounded native preparation while retaining the
    /// revocation fence; reacquiring the same mutex here would deadlock.
    pub fn validate_current(&self) -> Result<(), &'static str> {
        if *self._revoked || self.authorization.remaining_lifetime().is_zero() {
            Err("recording task authorization expired or was revoked")
        } else {
            Ok(())
        }
    }
}

impl RecordingAuthorization {
    /// A recorder can revoke its own stream without revoking the whole task.
    /// Admission always holds both fences, so task revocation still wins.
    pub fn recording_local(parent: Arc<Self>) -> Arc<Self> {
        Arc::new(Self {
            revoked: Mutex::new(false),
            expires_at_unix_ms: parent.expires_at_unix_ms,
            clock: parent.clock.clone(),
            monotonic_deadline: parent.monotonic_deadline,
            parent: Some(parent),
        })
    }
    pub fn remaining_lifetime(&self) -> Duration {
        let wall = Duration::from_millis(self.expires_at_unix_ms.saturating_sub((self.clock)()));
        self.monotonic_deadline.map_or(wall, |deadline| {
            wall.min(deadline.saturating_duration_since(Instant::now()))
        })
    }
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
            parent: None,
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
        let parent = self
            .parent
            .as_ref()
            .map(|parent| parent.admit())
            .transpose()?;
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
        Ok(RecordingAdmission {
            _revoked: revoked,
            authorization: self,
            _parent: parent.map(Box::new),
        })
    }

    pub fn with_active<T>(&self, work: impl FnOnce() -> T) -> Result<T, &'static str> {
        let _admission = self.admit()?;
        Ok(work())
    }
}

#[cfg(test)]
mod tests;
