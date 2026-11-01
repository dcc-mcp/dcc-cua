use super::*;
use rstest::rstest;
use std::sync::atomic::{AtomicU64, Ordering};

#[rstest]
fn expiry_rejects_queued_work_and_cannot_be_reopened_by_clock_rollback() {
    let clock = Arc::new(AtomicU64::new(9));
    let read_clock = Arc::clone(&clock);
    let authority =
        RecordingAuthorization::with_clock(10, Arc::new(move || read_clock.load(Ordering::SeqCst)));
    let mut admitted = Vec::new();
    authority.with_active(|| admitted.push(1)).unwrap();
    clock.store(10, Ordering::SeqCst);
    assert!(authority.with_active(|| admitted.push(2)).is_err());
    clock.store(9, Ordering::SeqCst);
    assert!(authority.with_active(|| admitted.push(3)).is_err());
    assert_eq!(admitted, vec![1]);
}

#[rstest]
fn expiry_during_readback_refuses_the_publication_boundary() {
    let clock = Arc::new(AtomicU64::new(9));
    let read_clock = Arc::clone(&clock);
    let authority =
        RecordingAuthorization::with_clock(10, Arc::new(move || read_clock.load(Ordering::SeqCst)));
    authority.with_active(|| ()).unwrap();
    clock.store(10, Ordering::SeqCst);
    assert!(
        authority
            .with_active(|| panic!("late capture was published"))
            .is_err()
    );
}

#[rstest]
fn monotonic_deadline_refuses_work_despite_wall_clock_rollback() {
    let mut authority = RecordingAuthorization::with_clock(10, Arc::new(|| 0));
    authority.monotonic_deadline = Some(Instant::now());
    assert!(
        authority
            .with_active(|| panic!("clock rollback extended lease"))
            .is_err()
    );
}

#[rstest]
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

#[rstest]
fn retained_parent_deadline_survives_late_issuance_and_clock_rollback() {
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 60_000;
    let original = Instant::now() + Duration::from_millis(60);
    let mut authority = RecordingAuthorization::with_monotonic_deadline(expires, original);
    // Simulates a wall clock rolled backwards after the original policy admission.
    authority.clock = Arc::new(|| 0);
    let mut frames = 0;
    authority.with_active(|| frames += 1).unwrap();
    std::thread::sleep(
        original.saturating_duration_since(Instant::now()) + Duration::from_millis(5),
    );
    assert!(authority.with_active(|| frames += 1).is_err());
    assert_eq!(frames, 1);
    let mut rebound = RecordingAuthorization::with_monotonic_deadline(expires, original);
    rebound.clock = Arc::new(|| 0);
    assert!(
        rebound
            .with_active(|| panic!("rebind extended original deadline"))
            .is_err()
    );
}

#[rstest]
fn retained_deadline_cannot_extend_a_shorter_epoch_lease() {
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 50;
    let mut authority = RecordingAuthorization::with_monotonic_deadline(
        expires,
        Instant::now() + Duration::from_secs(60),
    );
    authority.clock = Arc::new(|| 0);
    std::thread::sleep(Duration::from_millis(70));
    assert!(authority.with_active(|| ()).is_err());
}
