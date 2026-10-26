use super::*;
use rstest::rstest;
use std::cell::Cell;

#[rstest]
fn duration_timeout_is_a_controlled_stop_not_a_source_fault() {
    let clock = Cell::new(Duration::ZERO);
    let outcome = run_packet_loop::<()>(
        Duration::from_millis(10),
        || false,
        || {
            clock.set(Duration::from_millis(10));
            Err(mpsc::RecvTimeoutError::Timeout)
        },
        || true,
        || clock.get(),
        |_| panic!("timeout has no PCM"),
    );
    assert_eq!(outcome, PacketLoopEnd::Stopped);
}

#[rstest]
fn continuous_packets_cannot_avoid_the_duration_stop_or_be_admitted_after_it() {
    let calls = Cell::new(0);
    let ingested = Cell::new(0);
    let clock = Cell::new(Duration::ZERO);
    let outcome = run_packet_loop(
        Duration::from_millis(10),
        || calls.get() >= 3,
        || {
            calls.set(calls.get() + 1);
            clock.set(Duration::from_millis(10));
            Ok(())
        },
        || true,
        || clock.get(),
        |_| {
            ingested.set(ingested.get() + 1);
            Ok(())
        },
    );
    assert_eq!(outcome, PacketLoopEnd::Stopped);
    assert_eq!(ingested.get(), 0);
    assert_eq!(calls.get(), 1);
}

#[rstest]
#[case(true)]
#[case(false)]
fn authorization_loss_is_a_fault_even_at_the_duration_boundary(#[case] packet: bool) {
    let authorized = Cell::new(true);
    let clock = Cell::new(Duration::ZERO);
    let outcome = run_packet_loop(
        Duration::from_millis(10),
        || false,
        || {
            authorized.set(false);
            clock.set(Duration::from_millis(10));
            if packet {
                Ok(())
            } else {
                Err(mpsc::RecvTimeoutError::Timeout)
            }
        },
        || authorized.get(),
        || clock.get(),
        |_| panic!("revoked PCM cannot be admitted"),
    );
    assert_eq!(outcome, PacketLoopEnd::SourceFailed);
}

#[rstest]
fn source_disconnect_is_not_reclassified_as_a_duration_stop() {
    let clock = Cell::new(Duration::ZERO);
    let outcome = run_packet_loop::<()>(
        Duration::from_millis(10),
        || false,
        || {
            clock.set(Duration::from_millis(10));
            Err(mpsc::RecvTimeoutError::Disconnected)
        },
        || true,
        || clock.get(),
        |_| panic!("disconnected source has no PCM"),
    );
    assert_eq!(outcome, PacketLoopEnd::SourceFailed);
}

#[rstest]
#[case(AudioLifecycleError::Packet)]
#[case(AudioLifecycleError::Source)]
fn genuine_ingest_faults_remain_failures(#[case] error: AudioLifecycleError) {
    let outcome = run_packet_loop(
        Duration::from_millis(10),
        || false,
        || Ok(()),
        || true,
        || Duration::ZERO,
        |_| Err(error),
    );
    assert_eq!(outcome, PacketLoopEnd::SourceFailed);
}

#[rstest]
fn stopped_or_already_expired_owner_does_not_enter_receive() {
    for cancelled in [true, false] {
        let outcome = run_packet_loop::<()>(
            Duration::from_millis(10),
            || cancelled,
            || panic!("stopped owner must not read another packet"),
            || true,
            || Duration::from_millis(10),
            |_| panic!("stopped owner must not ingest"),
        );
        assert_eq!(outcome, PacketLoopEnd::Stopped);
    }
}
