use super::*;
use rstest::rstest;

#[rstest]
fn temporary_backpressure_delivers_without_dropping_or_holding_the_revocation_fence() {
    let authorization = RecordingAuthorization::new(u64::MAX);
    let cancelled = AtomicBool::new(false);
    let (sender, receiver) = mpsc::sync_channel(0);
    let consumer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2));
        receiver.recv().unwrap()
    });
    assert_eq!(
        deliver_packet(
            &sender,
            42,
            &authorization,
            &cancelled,
            Duration::from_secs(1)
        ),
        Ok(true)
    );
    assert_eq!(consumer.join().unwrap(), 42);
}

#[rstest]
fn queue_stall_disconnect_and_revocation_remain_distinct_bounded_outcomes() {
    let authorization = RecordingAuthorization::new(u64::MAX);
    let cancelled = AtomicBool::new(false);
    let (sender, receiver) = mpsc::sync_channel(0);
    assert_eq!(
        deliver_packet(&sender, 42, &authorization, &cancelled, Duration::ZERO),
        Err(AudioLifecycleError::Source)
    );
    drop(receiver);
    assert_eq!(
        deliver_packet(&sender, 42, &authorization, &cancelled, Duration::ZERO),
        Err(AudioLifecycleError::Source)
    );
    authorization.revoke();
    assert_eq!(
        deliver_packet(&sender, 42, &authorization, &cancelled, Duration::ZERO),
        Ok(false)
    );
}

#[rstest]
#[case(false, false, true, true)]
#[case(true, false, true, false)]
#[case(true, true, true, true)]
#[case(false, false, false, false)]
fn absent_worker_is_not_unknown_but_spawned_worker_requires_reaping(
    #[case] spawned: bool,
    #[case] reaped: bool,
    #[case] reader_joined: bool,
    #[case] known: bool,
) {
    let evidence = OwnedAudioWorkerEvidence {
        spawned,
        reaped,
        reader_joined,
        ..Default::default()
    };
    assert_eq!(evidence.cleanup_known(), known);
}

#[rstest]
fn qpc_bracket_retries_only_scheduler_delay_with_a_fixed_ceiling() {
    let epoch = Instant::now();
    let mut count = 0;
    let calibrated = calibrate_epoch(epoch, || {
        count += 1;
        let before = epoch + Duration::from_millis(10);
        let after = before
            + if count == 1 {
                Duration::from_millis(3)
            } else {
                Duration::from_micros(200)
            };
        Ok((before, after, 1_000_000, 1_000_000))
    })
    .unwrap();
    assert_eq!(count, 2);
    assert_eq!(calibrated, 9_899_000);
    count = 0;
    assert_eq!(
        calibrate_epoch(epoch, || {
            count += 1;
            Ok((epoch, epoch + Duration::from_millis(3), 1, 1))
        }),
        Err(AudioLifecycleError::Source)
    );
    assert_eq!(count, 3);
}

#[rstest]
fn qpc_invalid_counter_and_api_failures_are_never_retried() {
    let epoch = Instant::now();
    let mut count = 0;
    assert_eq!(
        calibrate_epoch(epoch, || {
            count += 1;
            Ok((epoch, epoch, -1, 1))
        }),
        Err(AudioLifecycleError::Source)
    );
    assert_eq!(count, 1);
    count = 0;
    assert_eq!(
        calibrate_epoch(epoch, || {
            count += 1;
            Err(AudioLifecycleError::Source)
        }),
        Err(AudioLifecycleError::Source)
    );
    assert_eq!(count, 1);
}
