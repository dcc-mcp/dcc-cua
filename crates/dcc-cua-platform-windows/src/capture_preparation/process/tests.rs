use rstest::rstest;

use super::*;

#[rstest]
fn a_blocked_reply_does_not_block_the_supervisor_or_grow_an_unbounded_queue() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (entered, started) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let first = AtomicBool::new(true);
    let (sender, returned) = start_reply_writer(move |_| {
        if first.swap(false, Ordering::AcqRel) {
            entered.send(()).unwrap();
            blocked.recv().unwrap();
        }
        Ok(())
    })
    .unwrap();
    let pending = super::super::state::PreparationState::new([1; 16], 100, vec![]).status;
    let mut settled = pending.clone();
    settled.phase = PreparationPhase::Refused;
    settled.pending_sequence = None;
    settled.capture_revoked = true;
    settled.cleanup_verified = true;
    let reply = |index, status| Reply {
        index,
        nonce: [1; 32],
        status: Ok(status),
    };
    assert!(sender.try_send(reply(1, pending.clone())).is_ok());
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(sender.try_send(reply(2, settled)).is_ok());
    assert!(matches!(
        sender.try_send(reply(3, pending)),
        Err(mpsc::TrySendError::Full(_))
    ));
    assert!(returned.try_recv().is_err());
    release.send(()).unwrap();
    let first = returned.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(first.result.is_ok());
    assert!(!first.terminal);
    let second = returned.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(second.result.is_ok());
    assert!(second.terminal);
}

#[rstest]
#[case(false)]
#[case(true)]
fn reply_writer_failure_and_panic_have_actual_failure_acknowledgements(#[case] panic: bool) {
    let (sender, returned) = start_reply_writer(move |_| {
        assert!(!panic, "pure injected writer panic");
        Err(PreparationError::new(PreparationFailure::Disconnected))
    })
    .unwrap();
    sender
        .try_send(Reply {
            index: 1,
            nonce: [1; 32],
            status: Ok(super::super::state::PreparationState::new([1; 16], 100, vec![]).status),
        })
        .unwrap_or_else(|_| panic!("fresh pure writer queue must accept one reply"));
    let actual = returned.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!actual.terminal);
    assert_eq!(
        actual.result.unwrap_err().reason,
        if panic {
            PreparationFailure::WorkerLost
        } else {
            PreparationFailure::Disconnected
        }
    );
    assert!(matches!(
        returned.recv_timeout(Duration::from_secs(5)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
}
