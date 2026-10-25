use super::*;
use rstest::rstest;

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
