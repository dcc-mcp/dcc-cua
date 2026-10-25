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
fn held_admission_rechecks_expiry_before_starting_prepared_source() {
    let clock = Arc::new(AtomicU64::new(1));
    let read = clock.clone();
    let authority =
        RecordingAuthorization::with_clock(2, Arc::new(move || read.load(Ordering::SeqCst)));
    let guard = authority.admit().unwrap();
    clock.store(2, Ordering::SeqCst);
    assert!(guard.validate_current().is_err());
}
