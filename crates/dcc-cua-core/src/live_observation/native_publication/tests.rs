use super::*;
use rstest::rstest;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Barrier, Weak};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnerState {
    Idle,
    Queued,
    InFlight,
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Snapshot {
    owner_state: OwnerState,
    metadata: Option<Metadata>,
}

impl<P, R> PublicationController<P, R> {
    fn has_pending(&self) -> bool {
        self.shared.has_pending()
    }

    fn snapshot(&self) -> Snapshot {
        let slot = lock(&self.shared.slot);
        let owner_state = if slot.stopped {
            OwnerState::Stopped
        } else if let Some(owner) = &slot.owner {
            match lock(&owner.state).location {
                Location::Queued => OwnerState::Queued,
                Location::InFlight => OwnerState::InFlight,
                Location::Released => OwnerState::Idle,
            }
        } else {
            OwnerState::Idle
        };
        Snapshot {
            owner_state,
            metadata: slot.owner.as_ref().map(|owner| owner.metadata),
        }
    }
}

fn metadata(phase: NativePublicationPhase, sequence: u64) -> Metadata {
    Metadata {
        phase,
        stream_id: 81,
        sequence,
        started_generation: 43,
    }
}

fn future_deadline() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

fn quiet_channel<P, R>() -> (PublicationController<P, R>, PublicationInbox<P, R>) {
    channel(Arc::new(|| {}))
}

#[rstest]
fn queued_publication_is_visible_for_priority_service() {
    let (controller, inbox) = quiet_channel::<&str, ()>();
    assert!(!inbox.has_pending());
    assert!(!controller.has_pending());
    let pending = controller
        .submit(
            "publication",
            metadata(NativePublicationPhase::Prepare, 1),
            future_deadline(),
        )
        .unwrap();
    assert!(controller.has_pending());
    assert!(inbox.has_pending());
    let mut ordinary_capture_calls = 0;
    let work = if let Some(work) = inbox.take() {
        Some(work)
    } else {
        ordinary_capture_calls += 1;
        None
    };
    assert_eq!(ordinary_capture_calls, 0);
    let mut work = work.expect("queued publication must be available before ordinary capture");
    assert!(!inbox.has_pending());
    assert_eq!(work.take_payload(), "publication");
    assert_eq!(controller.snapshot().owner_state, OwnerState::InFlight);
    work.complete((), Instant::now()).unwrap();
    drop(pending);
}

#[rstest]
fn one_slot_covers_both_queue_and_in_flight_owner_and_all_controller_clones() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let clone = controller.clone();
    let mut pending = controller
        .submit(
            10,
            metadata(NativePublicationPhase::Prepare, 1),
            future_deadline(),
        )
        .unwrap();
    assert_eq!(controller.snapshot().owner_state, OwnerState::Queued);
    assert!(matches!(
        clone.submit(
            11,
            metadata(NativePublicationPhase::Prepare, 2),
            future_deadline()
        ),
        Err(Failure::Full)
    ));
    let mut work = inbox.take().unwrap();
    assert!(inbox.take().is_none());
    assert!(matches!(
        clone.submit(
            12,
            metadata(NativePublicationPhase::Encoded, 3),
            future_deadline()
        ),
        Err(Failure::Full)
    ));
    assert_eq!(work.take_payload(), 10);
    work.complete(20, Instant::now()).unwrap();
    assert_eq!(controller.snapshot().owner_state, OwnerState::Idle);
    let second = clone
        .submit(
            13,
            metadata(NativePublicationPhase::Encoded, 4),
            future_deadline(),
        )
        .unwrap();
    assert_eq!(pending.try_result(Instant::now()), Ok(Some(20)));
    assert!(inbox.has_pending());
    drop(second);
}

#[rstest]
fn canceled_receiver_before_processing_drains_queue_and_notifies_owner() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let wake_count = Arc::clone(&wakes);
    let (controller, inbox) = channel::<u64, ()>(Arc::new(move || {
        wake_count.fetch_add(1, Ordering::SeqCst);
    }));
    let pending = controller
        .submit(
            10,
            metadata(NativePublicationPhase::Prepare, 1),
            future_deadline(),
        )
        .unwrap();
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    drop(pending);
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
    assert!(!inbox.has_pending());
    assert!(inbox.take().is_none());
    assert_eq!(controller.snapshot().owner_state, OwnerState::Idle);
    assert_eq!(controller.snapshot().metadata, None);
    let replacement = controller
        .submit(
            11,
            metadata(NativePublicationPhase::Encoded, 2),
            future_deadline(),
        )
        .unwrap();
    drop(replacement);
}

#[rstest]
fn cancellation_during_service_blocks_ack_and_retains_capacity_until_owner_finishes() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let pending = controller
        .submit(
            10,
            metadata(NativePublicationPhase::Prepare, 1),
            future_deadline(),
        )
        .unwrap();
    let mut work = inbox.take().unwrap();
    let payload = work.take_payload();
    work.ensure_live(Instant::now()).unwrap();
    drop(pending);
    assert_eq!(payload, 10);
    assert_eq!(work.ensure_live(Instant::now()), Err(Failure::Canceled));
    assert_eq!(controller.snapshot().owner_state, OwnerState::InFlight);
    assert!(matches!(
        controller.submit(
            11,
            metadata(NativePublicationPhase::Encoded, 2),
            future_deadline()
        ),
        Err(Failure::Full)
    ));
    assert_eq!(work.complete(20, Instant::now()), Err(Failure::Canceled));
    assert_eq!(controller.snapshot().owner_state, OwnerState::Idle);
    assert_eq!(controller.snapshot().metadata, None);
}

#[rstest]
fn cancellation_and_validation_on_distinct_threads_cannot_ack() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let pending = controller
        .submit(
            10,
            metadata(NativePublicationPhase::Encoded, 1),
            future_deadline(),
        )
        .unwrap();
    let mut work = inbox.take().unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let owner_barrier = Arc::clone(&barrier);
    let owner = std::thread::spawn(move || {
        let payload = work.take_payload();
        work.ensure_live(Instant::now()).unwrap();
        owner_barrier.wait();
        owner_barrier.wait();
        work.complete(payload + 1, Instant::now())
    });
    barrier.wait();
    drop(pending);
    assert!(matches!(
        controller.submit(
            11,
            metadata(NativePublicationPhase::Prepare, 2),
            future_deadline()
        ),
        Err(Failure::Full)
    ));
    barrier.wait();
    assert_eq!(owner.join().unwrap(), Err(Failure::Canceled));
    assert_eq!(controller.snapshot().owner_state, OwnerState::Idle);
}

#[rstest]
fn cancellation_after_take_cannot_revoke_the_owners_unconsumed_payload() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let pending = controller
        .submit(
            10,
            metadata(NativePublicationPhase::Prepare, 1),
            future_deadline(),
        )
        .unwrap();
    let mut work = inbox.take().unwrap();
    work.ensure_live(Instant::now()).unwrap();
    drop(pending);
    assert_eq!(work.take_payload(), 10);
    assert_eq!(work.ensure_live(Instant::now()), Err(Failure::Canceled));
    assert_eq!(work.complete(20, Instant::now()), Err(Failure::Canceled));
}

#[rstest]
fn stop_after_take_cannot_revoke_the_owners_unconsumed_payload() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let mut pending = controller
        .submit(
            10,
            metadata(NativePublicationPhase::Encoded, 1),
            future_deadline(),
        )
        .unwrap();
    let mut work = inbox.take().unwrap();
    inbox.stop();
    assert_eq!(work.take_payload(), 10);
    assert_eq!(work.ensure_live(Instant::now()), Err(Failure::Stopped));
    assert_eq!(work.complete(20, Instant::now()), Err(Failure::Stopped));
    assert_eq!(pending.try_result(Instant::now()), Err(Failure::Stopped));
}

#[rstest]
fn simultaneous_controller_clones_admit_exactly_one_request() {
    let (controller, inbox) = quiet_channel::<u64, ()>();
    let barrier = Arc::new(Barrier::new(3));
    let mut owners = Vec::new();
    for sequence in [1, 2] {
        let clone = controller.clone();
        let start = Arc::clone(&barrier);
        owners.push(std::thread::spawn(move || {
            start.wait();
            clone.submit(
                sequence,
                metadata(NativePublicationPhase::Prepare, sequence),
                future_deadline(),
            )
        }));
    }
    barrier.wait();
    let results: Vec<_> = owners
        .into_iter()
        .map(|owner| owner.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(Failure::Full)))
            .count(),
        1
    );
    assert!(inbox.has_pending());
    drop(results);
    assert!(inbox.take().is_none());
}

#[rstest]
fn original_deadline_is_not_extended_and_expired_admission_is_rejected() {
    let (controller, inbox) = quiet_channel::<u64, ()>();
    let deadline = future_deadline();
    let pending = controller
        .submit(10, metadata(NativePublicationPhase::Prepare, 1), deadline)
        .unwrap();
    let work = inbox.take().unwrap();
    assert_eq!(work.deadline(), deadline);
    drop(work);
    drop(pending);
    assert!(matches!(
        controller.submit(
            11,
            metadata(NativePublicationPhase::Prepare, 2),
            Instant::now()
        ),
        Err(Failure::Deadline)
    ));
    assert_eq!(controller.snapshot().owner_state, OwnerState::Idle);
}

#[rstest]
fn deadline_before_processing_discards_queued_payload() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let deadline = future_deadline();
    let mut pending = controller
        .submit(10, metadata(NativePublicationPhase::Prepare, 1), deadline)
        .unwrap();
    assert_eq!(pending.try_result(deadline), Err(Failure::Deadline));
    assert!(inbox.take().is_none());
    assert_eq!(controller.snapshot().owner_state, OwnerState::Idle);
    assert_eq!(controller.snapshot().metadata, None);
}

#[rstest]
fn owner_take_refuses_a_request_that_expired_in_the_queue() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let deadline = Instant::now() + Duration::from_millis(25);
    let mut pending = controller
        .submit(10, metadata(NativePublicationPhase::Prepare, 1), deadline)
        .unwrap();
    std::thread::sleep(Duration::from_millis(30));
    assert!(inbox.take().is_none());
    assert_eq!(pending.try_result(Instant::now()), Err(Failure::Deadline));
    assert_eq!(controller.snapshot().owner_state, OwnerState::Idle);
}

#[rstest]
fn deadline_before_validation_prevents_owner_work_and_ack() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let deadline = future_deadline();
    let mut pending = controller
        .submit(10, metadata(NativePublicationPhase::Prepare, 1), deadline)
        .unwrap();
    let work = inbox.take().unwrap();
    assert_eq!(work.ensure_live(deadline), Err(Failure::Deadline));
    assert_eq!(controller.snapshot().owner_state, OwnerState::InFlight);
    assert_eq!(work.complete(20, deadline), Err(Failure::Deadline));
    assert_eq!(pending.try_result(deadline), Err(Failure::Deadline));
    assert_eq!(controller.snapshot().owner_state, OwnerState::Idle);
}

#[rstest]
fn deadline_after_validation_rejects_late_ack() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let deadline = future_deadline();
    let mut pending = controller
        .submit(10, metadata(NativePublicationPhase::Encoded, 1), deadline)
        .unwrap();
    let mut work = inbox.take().unwrap();
    work.ensure_live(Instant::now()).unwrap();
    assert_eq!(work.take_payload(), 10);
    assert_eq!(work.complete(20, deadline), Err(Failure::Deadline));
    assert_eq!(pending.try_result(deadline), Err(Failure::Deadline));
    assert_eq!(controller.snapshot().owner_state, OwnerState::Idle);
}

#[rstest]
fn reply_polled_after_deadline_is_rejected_even_if_owner_completed_earlier() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let deadline = future_deadline();
    let mut pending = controller
        .submit(10, metadata(NativePublicationPhase::Encoded, 1), deadline)
        .unwrap();
    inbox.take().unwrap().complete(20, Instant::now()).unwrap();
    let next = controller
        .submit(
            11,
            metadata(NativePublicationPhase::Prepare, 2),
            future_deadline(),
        )
        .unwrap();
    assert_eq!(pending.try_result(deadline), Err(Failure::Deadline));
    assert_eq!(pending.try_result(deadline), Err(Failure::Deadline));
    assert_eq!(controller.snapshot().owner_state, OwnerState::Queued);
    assert_eq!(controller.snapshot().metadata.unwrap().sequence, 2);
    drop(next);
}

#[rstest]
fn stop_drains_queued_work_and_permanently_rejects_submissions() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let mut pending = controller
        .submit(
            10,
            metadata(NativePublicationPhase::Prepare, 1),
            future_deadline(),
        )
        .unwrap();
    inbox.stop();
    inbox.stop();
    assert!(!inbox.has_pending());
    assert!(inbox.take().is_none());
    assert_eq!(pending.try_result(Instant::now()), Err(Failure::Stopped));
    assert!(matches!(
        controller.clone().submit(
            11,
            metadata(NativePublicationPhase::Encoded, 2),
            future_deadline()
        ),
        Err(Failure::Stopped)
    ));
    assert_eq!(controller.snapshot().owner_state, OwnerState::Stopped);
}

#[rstest]
fn stop_during_owner_validation_prevents_ack_on_another_thread() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let mut pending = controller
        .submit(
            10,
            metadata(NativePublicationPhase::Encoded, 1),
            future_deadline(),
        )
        .unwrap();
    let mut work = inbox.take().unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let owner_barrier = Arc::clone(&barrier);
    let owner = std::thread::spawn(move || {
        let payload = work.take_payload();
        work.ensure_live(Instant::now()).unwrap();
        owner_barrier.wait();
        owner_barrier.wait();
        assert_eq!(work.ensure_live(Instant::now()), Err(Failure::Stopped));
        work.complete(payload + 1, Instant::now())
    });
    barrier.wait();
    inbox.stop();
    barrier.wait();
    assert_eq!(owner.join().unwrap(), Err(Failure::Stopped));
    assert_eq!(pending.try_result(Instant::now()), Err(Failure::Stopped));
    assert_eq!(controller.snapshot().owner_state, OwnerState::Stopped);
}

#[rstest]
fn inbox_disconnect_stops_queued_and_in_flight_requests() {
    for in_flight in [false, true] {
        let (controller, inbox) = quiet_channel::<u64, u64>();
        let mut pending = controller
            .submit(
                10,
                metadata(NativePublicationPhase::Prepare, 1),
                future_deadline(),
            )
            .unwrap();
        let work = in_flight.then(|| inbox.take().unwrap());
        drop(inbox);
        assert_eq!(pending.try_result(Instant::now()), Err(Failure::Stopped));
        if let Some(work) = work {
            assert_eq!(work.complete(20, Instant::now()), Err(Failure::Stopped));
        }
        assert_eq!(controller.snapshot().owner_state, OwnerState::Stopped);
    }
}

#[rstest]
fn incomplete_owner_drop_closes_reply_and_releases_slot() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let mut pending = controller
        .submit(
            10,
            metadata(NativePublicationPhase::Prepare, 1),
            future_deadline(),
        )
        .unwrap();
    drop(inbox.take().unwrap());
    assert_eq!(
        pending.try_result(Instant::now()),
        Err(Failure::ReplyUnavailable)
    );
    assert_eq!(controller.snapshot().owner_state, OwnerState::Idle);
    assert_eq!(controller.snapshot().metadata, None);
    let next = controller
        .submit(
            11,
            metadata(NativePublicationPhase::Encoded, 2),
            future_deadline(),
        )
        .unwrap();
    drop(next);
}

#[rstest]
fn canceling_an_old_completed_receiver_cannot_drain_a_new_request() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let first = controller
        .submit(
            10,
            metadata(NativePublicationPhase::Prepare, 1),
            future_deadline(),
        )
        .unwrap();
    inbox.take().unwrap().complete(20, Instant::now()).unwrap();
    let mut second = controller
        .submit(
            11,
            metadata(NativePublicationPhase::Encoded, 2),
            future_deadline(),
        )
        .unwrap();
    drop(first);
    assert!(inbox.has_pending());
    let mut work = inbox.take().unwrap();
    assert_eq!(work.take_payload(), 11);
    work.complete(21, Instant::now()).unwrap();
    assert_eq!(second.try_result(Instant::now()), Ok(Some(21)));
}

#[rstest]
fn prepare_and_encoded_preserve_exact_typed_proof_payload_metadata_and_deadline() {
    struct TypedProof {
        target_pid: u32,
        target_hwnd: isize,
        opaque_proof: Arc<Vec<u8>>,
        generation: u64,
    }
    let (controller, inbox) = quiet_channel::<TypedProof, Vec<u8>>();
    for phase in [
        NativePublicationPhase::Prepare,
        NativePublicationPhase::Encoded,
    ] {
        let proof = Arc::new(vec![0, 41, 255, 17]);
        let expected_metadata = metadata(phase, 913);
        let deadline = future_deadline();
        let payload = TypedProof {
            target_pid: 7301,
            target_hwnd: 0x12345,
            opaque_proof: Arc::clone(&proof),
            generation: 43,
        };
        let mut pending = controller
            .submit(payload, expected_metadata, deadline)
            .unwrap();
        assert_eq!(controller.snapshot().metadata, Some(expected_metadata));
        let mut work = inbox.take().unwrap();
        assert_eq!(work.metadata(), expected_metadata);
        assert_eq!(work.deadline(), deadline);
        let payload = work.take_payload();
        assert_eq!(
            (payload.target_pid, payload.target_hwnd, payload.generation),
            (7301, 0x12345, 43)
        );
        assert!(Arc::ptr_eq(&payload.opaque_proof, &proof));
        assert_eq!(*payload.opaque_proof, [0, 41, 255, 17]);
        work.complete(vec![5, 8, 13], Instant::now()).unwrap();
        assert_eq!(pending.try_result(Instant::now()), Ok(Some(vec![5, 8, 13])));
    }
}

#[rstest]
fn payload_and_result_are_consumed_once() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let mut pending = controller
        .submit(
            10,
            metadata(NativePublicationPhase::Prepare, 1),
            future_deadline(),
        )
        .unwrap();
    assert_eq!(pending.try_result(Instant::now()), Ok(None));
    let mut work = inbox.take().unwrap();
    assert_eq!(work.take_payload(), 10);
    let second_take =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work.take_payload()));
    assert!(second_take.is_err());
    work.ensure_live(Instant::now()).unwrap();
    work.complete(20, Instant::now()).unwrap();
    assert_eq!(pending.try_result(Instant::now()), Ok(Some(20)));
    assert_eq!(
        pending.try_result(Instant::now()),
        Err(Failure::ReplyUnavailable)
    );
}

struct DropProbe {
    on_drop: Arc<dyn Fn() + Send + Sync>,
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        (self.on_drop)();
    }
}

type ProbeShared = Shared<DropProbe, DropProbe>;
type ProbeBinding = Arc<Mutex<Option<Weak<ProbeShared>>>>;

fn assert_unlocked(binding: &ProbeBinding) {
    let shared = { lock(binding).as_ref().and_then(Weak::upgrade) };
    if let Some(shared) = shared {
        let slot = shared
            .slot
            .try_lock()
            .expect("slot lock held across external code");
        if let Some(owner) = &slot.owner {
            let _request = owner
                .state
                .try_lock()
                .expect("request lock held across external code");
        }
    }
}

fn probe(binding: &ProbeBinding, drops: &Arc<AtomicUsize>) -> DropProbe {
    let binding = Arc::clone(binding);
    let drops = Arc::clone(drops);
    DropProbe {
        on_drop: Arc::new(move || {
            assert_unlocked(&binding);
            drops.fetch_add(1, Ordering::SeqCst);
        }),
    }
}

fn probing_channel() -> (
    PublicationController<DropProbe, DropProbe>,
    PublicationInbox<DropProbe, DropProbe>,
    ProbeBinding,
    Arc<AtomicUsize>,
) {
    let binding: ProbeBinding = Arc::new(Mutex::new(None));
    let wake_binding = Arc::clone(&binding);
    let (controller, inbox) = channel(Arc::new(move || {
        assert_unlocked(&wake_binding);
    }));
    *lock(&binding) = Some(Arc::downgrade(&controller.shared));
    (controller, inbox, binding, Arc::new(AtomicUsize::new(0)))
}

#[rstest]
fn wake_and_payload_destructors_never_run_under_slot_or_request_locks() {
    let (controller, inbox, binding, drops) = probing_channel();
    let pending = controller
        .submit(
            probe(&binding, &drops),
            metadata(NativePublicationPhase::Prepare, 1),
            future_deadline(),
        )
        .unwrap();
    assert!(matches!(
        controller.submit(
            probe(&binding, &drops),
            metadata(NativePublicationPhase::Prepare, 2),
            future_deadline()
        ),
        Err(Failure::Full)
    ));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    drop(pending);
    assert_eq!(drops.load(Ordering::SeqCst), 2);

    let pending = controller
        .submit(
            probe(&binding, &drops),
            metadata(NativePublicationPhase::Prepare, 3),
            future_deadline(),
        )
        .unwrap();
    let mut work = inbox.take().unwrap();
    let payload = work.take_payload();
    drop(pending);
    assert_eq!(
        work.complete(probe(&binding, &drops), Instant::now()),
        Err(Failure::Canceled)
    );
    drop(payload);
    assert_eq!(drops.load(Ordering::SeqCst), 4);

    let pending = controller
        .submit(
            probe(&binding, &drops),
            metadata(NativePublicationPhase::Prepare, 4),
            future_deadline(),
        )
        .unwrap();
    drop(inbox.take().unwrap());
    drop(pending);
    assert_eq!(drops.load(Ordering::SeqCst), 5);

    let pending = controller
        .submit(
            probe(&binding, &drops),
            metadata(NativePublicationPhase::Encoded, 5),
            future_deadline(),
        )
        .unwrap();
    inbox.stop();
    drop(pending);
    assert_eq!(drops.load(Ordering::SeqCst), 6);
    assert!(matches!(
        controller.submit(
            probe(&binding, &drops),
            metadata(NativePublicationPhase::Encoded, 6),
            future_deadline()
        ),
        Err(Failure::Stopped)
    ));
    assert_eq!(drops.load(Ordering::SeqCst), 7);
}

#[rstest]
fn discarded_late_result_is_dropped_outside_all_locks() {
    let (controller, inbox, binding, drops) = probing_channel();
    let deadline = future_deadline();
    let mut pending = controller
        .submit(
            probe(&binding, &drops),
            metadata(NativePublicationPhase::Encoded, 1),
            deadline,
        )
        .unwrap();
    inbox
        .take()
        .unwrap()
        .complete(probe(&binding, &drops), Instant::now())
        .unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(matches!(
        pending.try_result(deadline),
        Err(Failure::Deadline)
    ));
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

#[rstest]
fn stop_after_completion_rejects_and_drops_an_unconsumed_reply() {
    let (controller, inbox, binding, drops) = probing_channel();
    let mut pending = controller
        .submit(
            probe(&binding, &drops),
            metadata(NativePublicationPhase::Encoded, 1),
            future_deadline(),
        )
        .unwrap();
    inbox
        .take()
        .unwrap()
        .complete(probe(&binding, &drops), Instant::now())
        .unwrap();
    inbox.stop();
    assert!(matches!(
        pending.try_result(Instant::now()),
        Err(Failure::Stopped)
    ));
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

fn wait_until_actual_deadline(deadline: Instant) {
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
    assert!(Instant::now() >= deadline);
}

#[rstest]
fn completion_rejects_a_captured_clock_after_actual_deadline() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let deadline = Instant::now() + Duration::from_millis(250);
    let mut pending = controller
        .submit(10, metadata(NativePublicationPhase::Encoded, 1), deadline)
        .unwrap();
    let mut work = inbox.take().unwrap();
    let payload = work.take_payload();
    let captured = Instant::now();
    assert!(captured < deadline);
    wait_until_actual_deadline(deadline);
    assert_eq!(work.complete(payload + 1, captured), Err(Failure::Deadline));
    assert_eq!(pending.try_result(captured), Err(Failure::Deadline));
    assert_eq!(controller.snapshot().metadata, None);
    assert_eq!(controller.snapshot().owner_state, OwnerState::Idle);
}

#[rstest]
fn reply_rejects_a_captured_clock_after_actual_deadline() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let deadline = Instant::now() + Duration::from_millis(250);
    let mut pending = controller
        .submit(10, metadata(NativePublicationPhase::Prepare, 1), deadline)
        .unwrap();
    let captured = Instant::now();
    assert!(captured < deadline);
    inbox.take().unwrap().complete(11, captured).unwrap();
    wait_until_actual_deadline(deadline);
    assert_eq!(pending.try_result(captured), Err(Failure::Deadline));
    assert_eq!(pending.try_result(captured), Err(Failure::Deadline));
    assert_eq!(controller.snapshot().metadata, None);
}

#[rstest]
fn validation_rejects_a_captured_clock_after_actual_deadline() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let deadline = Instant::now() + Duration::from_millis(250);
    let mut pending = controller
        .submit(10, metadata(NativePublicationPhase::Prepare, 1), deadline)
        .unwrap();
    let work = inbox.take().unwrap();
    let captured = Instant::now();
    assert!(captured < deadline);
    wait_until_actual_deadline(deadline);
    assert_eq!(work.ensure_live(captured), Err(Failure::Deadline));
    assert_eq!(controller.snapshot().owner_state, OwnerState::InFlight);
    assert!(matches!(
        controller.submit(
            12,
            metadata(NativePublicationPhase::Encoded, 2),
            future_deadline()
        ),
        Err(Failure::Full)
    ));
    assert_eq!(work.complete(11, captured), Err(Failure::Deadline));
    assert_eq!(pending.try_result(captured), Err(Failure::Deadline));
    assert_eq!(controller.snapshot().metadata, None);
}

#[rstest]
fn mutex_wait_cannot_use_a_pre_deadline_completion_clock() {
    let (controller, inbox) = quiet_channel::<u64, u64>();
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut pending = controller
        .submit(10, metadata(NativePublicationPhase::Encoded, 1), deadline)
        .unwrap();
    let mut work = inbox.take().unwrap();
    let payload = work.take_payload();
    let captured = Instant::now();
    assert!(captured < deadline);
    let slot_guard = lock(&controller.shared.slot);
    let (entered, waiting) = std::sync::mpsc::channel();
    let owner = std::thread::spawn(move || {
        entered.send(()).unwrap();
        work.complete(payload + 1, captured)
    });
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!owner.is_finished());
    wait_until_actual_deadline(deadline);
    assert!(!owner.is_finished());
    drop(slot_guard);
    assert_eq!(owner.join().unwrap(), Err(Failure::Deadline));
    assert_eq!(pending.try_result(captured), Err(Failure::Deadline));
    assert_eq!(controller.snapshot().owner_state, OwnerState::Idle);
    assert_eq!(controller.snapshot().metadata, None);
}
