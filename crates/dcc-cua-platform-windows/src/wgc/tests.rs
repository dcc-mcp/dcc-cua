use rstest::rstest;

#[rstest]
#[case(Some(20_000), 1, 2, true)]
#[case(Some(20_000), 3, 3, false)]
#[case(None, 0, 1, false)]
fn calibration_retries_only_wide_brackets_and_uses_the_accepted_sample(
    #[case] qpc: Option<i64>,
    #[case] wide_samples: usize,
    #[case] expected_samples: usize,
    #[case] success: bool,
) {
    let now = Instant::now();
    let mut count = 0;
    let measurement = WgcFrameMeasurement::new(
        Duration::ZERO,
        Duration::ZERO,
        Duration::ZERO,
        Duration::ZERO,
        Some(10_000),
    );
    let result = measurement.at_publish_with_sample(|| {
        count += 1;
        let before = now + Duration::from_millis(count as u64 * 10);
        (
            before,
            qpc,
            before + Duration::from_millis(if count <= wide_samples { 3 } else { 1 }),
        )
    });
    assert_eq!(count, expected_samples);
    assert_eq!(result.capture_clock.is_ok(), success);
    if let Ok(clock) = result.capture_clock {
        assert_eq!(clock.published_at, now + Duration::from_millis(21));
        assert_eq!(clock.captured_at, now + Duration::from_micros(19_500));
        assert_eq!(clock.calibration_bracket, Duration::from_millis(1));
    }
}

use super::*;

#[rstest]
fn compositor_clock_retains_source_age_through_delayed_publication() {
    let before = Instant::now();
    let after = before + Duration::from_micros(100);
    let timing = compositor_timing_from_100ns(Some(10_000_000), Some(17_500_000));
    let clock = map_compositor_clock(timing, Some(17_500_000), before, after).unwrap();
    assert_eq!(
        clock.captured_at,
        before + Duration::from_micros(50) - Duration::from_millis(750)
    );
    assert_eq!(clock.published_at, after);
    assert_eq!(clock.calibration_bracket, Duration::from_micros(100));
    assert_eq!(clock.published_qpc_100ns, 17_500_000);
}

#[rstest]
#[case(
    None,
    Some(20_000),
    WgcCompositorTimingUnavailable::FrameTimestampUnavailable
)]
#[case(
    Some(10_000),
    None,
    WgcCompositorTimingUnavailable::PerformanceCounterUnavailable
)]
#[case(
    Some(-1),
    Some(20_000),
    WgcCompositorTimingUnavailable::FrameTimestampUnavailable
)]
#[case(
    Some(30_000),
    Some(20_000),
    WgcCompositorTimingUnavailable::TimestampAfterPublish
)]
fn invalid_compositor_clock_never_substitutes_publication_time(
    #[case] source: Option<i64>,
    #[case] published: Option<i64>,
    #[case] reason: WgcCompositorTimingUnavailable,
) {
    let now = Instant::now();
    assert_eq!(
        map_compositor_clock(
            compositor_timing_from_100ns(source, published),
            published,
            now,
            now
        ),
        Err(reason)
    );
}

#[rstest]
#[case(2, true)]
#[case(3, false)]
fn compositor_clock_calibration_has_a_bounded_uncertainty(
    #[case] bracket_ms: u64,
    #[case] admitted: bool,
) {
    let now = Instant::now();
    let timing = compositor_timing_from_100ns(Some(10_000), Some(20_000));
    let result = map_compositor_clock(
        timing,
        Some(20_000),
        now,
        now + Duration::from_millis(bracket_ms),
    );
    assert_eq!(result.is_ok(), admitted);
    if !admitted {
        assert_eq!(
            result.unwrap_err(),
            WgcCompositorTimingUnavailable::CalibrationBracketTooWide
        );
    }
}

#[rstest]
fn native_wgc_geometry_failure_is_distinct_from_backend_unavailability() {
    for reason in [
        WgcGeometryError::FrameMetadataUnavailable,
        WgcGeometryError::InvalidFrameSize,
        WgcGeometryError::FrameSizeChanged,
        WgcGeometryError::RawShapeMismatch,
    ] {
        assert_eq!(geometry_error(reason).geometry_failure(), Some(reason));
    }
    assert_eq!(
        capture_error("backend", "unavailable").geometry_failure(),
        None
    );
    assert_eq!(
        invalid_target("backend frame unavailable").geometry_failure(),
        None
    );
}

#[rstest]
fn actual_item_size_is_bounded_before_pool_allocation_or_recreation() {
    assert_eq!(
        actual_size(SizeInt32 {
            Width: 646,
            Height: 495
        })
        .unwrap(),
        [646, 495]
    );
    for (width, height) in [(0, 1), (-1, 1), (1, 0), (i32::MAX, i32::MAX), (8193, 8192)] {
        assert_eq!(
            actual_size(SizeInt32 {
                Width: width,
                Height: height
            })
            .unwrap_err()
            .geometry_failure(),
            Some(WgcGeometryError::InvalidFrameSize)
        );
    }
}
