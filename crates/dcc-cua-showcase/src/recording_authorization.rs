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
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn expiry_rejects_queued_work_and_cannot_be_reopened_by_clock_rollback() {
        let clock = Arc::new(AtomicU64::new(9));
        let read_clock = Arc::clone(&clock);
        let authority = RecordingAuthorization::with_clock(
            10,
            Arc::new(move || read_clock.load(Ordering::SeqCst)),
        );
        let mut admitted = Vec::new();
        authority.with_active(|| admitted.push(1)).unwrap();
        clock.store(10, Ordering::SeqCst);
        assert!(authority.with_active(|| admitted.push(2)).is_err());
        clock.store(9, Ordering::SeqCst);
        assert!(authority.with_active(|| admitted.push(3)).is_err());
        assert_eq!(admitted, vec![1]);
    }

    #[test]
    fn expiry_during_readback_refuses_the_publication_boundary() {
        let clock = Arc::new(AtomicU64::new(9));
        let read_clock = Arc::clone(&clock);
        let authority = RecordingAuthorization::with_clock(
            10,
            Arc::new(move || read_clock.load(Ordering::SeqCst)),
        );
        authority.with_active(|| ()).unwrap();
        clock.store(10, Ordering::SeqCst);
        assert!(
            authority
                .with_active(|| panic!("late capture was published"))
                .is_err()
        );
    }

    #[test]
    fn monotonic_deadline_refuses_work_despite_wall_clock_rollback() {
        let mut authority = RecordingAuthorization::with_clock(10, Arc::new(|| 0));
        authority.monotonic_deadline = Some(Instant::now());
        assert!(
            authority
                .with_active(|| panic!("clock rollback extended lease"))
                .is_err()
        );
    }

    #[test]
    fn revocation_serializes_with_inflight_work_and_refuses_next_admission() {
        let authority = Arc::new(RecordingAuthorization::with_clock(u64::MAX, Arc::new(|| 0)));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let worker_authority = Arc::clone(&authority);
        let worker = std::thread::spawn(move || {
            worker_authority
                .with_active(|| {
                    entered_tx.send(()).unwrap();
                    finish_rx.recv().unwrap();
                })
                .unwrap()
        });
        entered_rx.recv().unwrap();
        let revoke_authority = Arc::clone(&authority);
        let revoker = std::thread::spawn(move || revoke_authority.revoke());
        finish_tx.send(()).unwrap();
        worker.join().unwrap();
        revoker.join().unwrap();
        assert!(
            authority
                .with_active(|| panic!("revoked frame admitted"))
                .is_err()
        );
    }
}
