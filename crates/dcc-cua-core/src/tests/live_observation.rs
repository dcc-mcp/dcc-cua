use rstest::rstest;

use super::*;

#[rstest]
#[case(None, LiveObservationStartDisposition::StartedNew)]
#[case(Some(false), LiveObservationStartDisposition::StartedNew)]
#[case(Some(true), LiveObservationStartDisposition::ReuseExisting)]
fn live_observation_restart_ownership_follows_active_state(
    #[case] active: Option<bool>,
    #[case] expected: LiveObservationStartDisposition,
) {
    let state = active.map(|active| json!({"active": active}));
    assert_eq!(live_observation_start_disposition(state.as_ref()), expected);
}

#[rstest]
fn terminal_live_observation_stream_is_never_reused() {
    let state = json!({
        "active": true,
        "terminal_reason": {
            "code": "capture_failed",
            "message": "persistent WGC worker failed",
        },
    });

    assert_eq!(
        live_observation_start_disposition(Some(&state)),
        LiveObservationStartDisposition::StartedNew
    );
}

#[rstest]
#[tokio::test]
async fn live_observation_reuse_rechecks_the_desktop_and_exact_target() {
    let reusable = json!({"active": true, "terminal_reason": null});
    let active_unknown_desktop =
        windows_diagnostic_base(Ok(0), Err("OpenInputDesktop: access denied"), Ok(()), false);
    let target_checks = Cell::new(0_u32);

    let (disposition, target) = preflight_live_observation_start(
        Some(&reusable),
        require_exact_window_observation_from(&active_unknown_desktop),
        || {
            target_checks.set(target_checks.get() + 1);
            async { Ok::<_, ComputerUseError>(77_u64) }
        },
    )
    .await
    .expect("active WTS plus unreadable InputDesktop may reuse after exact target validation");
    assert_eq!(disposition, LiveObservationStartDisposition::ReuseExisting);
    assert_eq!(target, 77);
    assert_eq!(target_checks.get(), 1);

    for denied_diagnostic in [
        windows_diagnostic_base(Ok(4), Ok(Some("Default")), Ok(()), false),
        windows_diagnostic_base(Ok(0), Ok(Some("Winlogon")), Ok(()), false),
    ] {
        let denied_target_checks = Cell::new(0_u32);
        let error = preflight_live_observation_start(
            Some(&reusable),
            require_exact_window_observation_from(&denied_diagnostic),
            || {
                denied_target_checks.set(denied_target_checks.get() + 1);
                async { Ok::<_, ComputerUseError>(77_u64) }
            },
        )
        .await
        .expect_err("disconnected or secure desktops must reject reuse");
        assert_eq!(
            error.code,
            ComputerUseErrorCode::InteractiveDesktopUnavailable
        );
        assert_eq!(denied_target_checks.get(), 0);
    }

    let target_error = preflight_live_observation_start(
        Some(&reusable),
        require_exact_window_observation_from(&active_unknown_desktop),
        || async {
            Err::<u64, _>(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                "the exact target identity changed",
            ))
        },
    )
    .await
    .expect_err("reuse must propagate exact target revalidation failure");
    assert_eq!(target_error.code, ComputerUseErrorCode::InvalidTarget);
}

#[rstest]
fn live_observation_fps_is_bounded() {
    assert_eq!(
        ComputerUseLiveObservationStartRequest::default(),
        ComputerUseLiveObservationStartRequest {
            fps: 10,
            max_dimension: 1_568,
        }
    );
    for fps in [0, 31] {
        assert!(
            ComputerUseLiveObservationStartRequest {
                fps,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
    for max_dimension in [255, 4_097] {
        assert!(
            ComputerUseLiveObservationStartRequest {
                max_dimension,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
}

#[rstest]
fn live_observation_stops_on_terminal_capture_errors() {
    assert!(terminal_capture_error(&ComputerUseError::new(
        ComputerUseErrorCode::InvalidTarget,
        "window identity changed",
    )));
    assert!(!terminal_capture_error(&ComputerUseError::new(
        ComputerUseErrorCode::InteractiveDesktopUnavailable,
        "desktop disconnected",
    )));
    assert!(!terminal_capture_error(&ComputerUseError::new(
        ComputerUseErrorCode::CaptureFailed,
        "transient WGC failure",
    )));
}

#[rstest]
fn windows_capture_failure_policy_distinguishes_retry_from_terminal_fences() {
    let capture_failed =
        || ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, "transient WGC failure");
    let active_unknown_desktop =
        windows_diagnostic_base(Ok(0), Err("OpenInputDesktop: access denied"), Ok(()), false);
    let disconnected = windows_diagnostic_base(Ok(4), Ok(Some("Default")), Ok(()), false);
    let secure_desktop = windows_diagnostic_base(Ok(0), Ok(Some("Winlogon")), Ok(()), false);

    let cases = [
        (
            capture_failed(),
            require_exact_window_observation_from(&active_unknown_desktop),
            "retry",
            ComputerUseErrorCode::CaptureFailed,
        ),
        (
            capture_failed(),
            require_exact_window_observation_from(&disconnected),
            "pause",
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
        ),
        (
            capture_failed(),
            require_exact_window_observation_from(&secure_desktop),
            "pause",
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
        ),
        (
            ComputerUseError::new(ComputerUseErrorCode::MissingWindow, "window closed"),
            require_exact_window_observation_from(&active_unknown_desktop),
            "terminal",
            ComputerUseErrorCode::MissingWindow,
        ),
        (
            ComputerUseError::new(ComputerUseErrorCode::InvalidTarget, "owner changed"),
            require_exact_window_observation_from(&active_unknown_desktop),
            "terminal",
            ComputerUseErrorCode::InvalidTarget,
        ),
    ];

    for (capture_error, observation_gate, expected_disposition, expected_code) in cases {
        let decision = live_capture_failure_disposition(capture_error, observation_gate);
        match (decision, expected_disposition) {
            (CaptureFailureDisposition::Retry(error), "retry")
            | (CaptureFailureDisposition::Pause(error), "pause")
            | (CaptureFailureDisposition::Terminal(error), "terminal") => {
                assert_eq!(error.code, expected_code);
            }
            (unexpected, _) => panic!("unexpected failure disposition: {unexpected:?}"),
        }
    }
}

#[rstest]
fn live_observation_pause_preserves_the_last_frame_and_clears_on_new_capture() {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(7, vec![7], 1, 1, Instant::now()),
        Duration::ZERO,
        "test_capture",
    );
    status.record_paused_error(&ComputerUseError::new(
        ComputerUseErrorCode::InteractiveDesktopUnavailable,
        "Windows interactive session disconnected",
    ));

    let paused = status.as_json(true, 15);
    assert_eq!(paused["active"], true);
    assert_eq!(paused["paused"], true);
    assert_eq!(
        paused["pause_reason"]["code"],
        "interactive_desktop_unavailable"
    );
    assert_eq!(paused["pause_reason"]["last_sequence"], 7);
    assert_eq!(paused["latest_sequence"], 7);
    assert_eq!(paused["terminal_reason"], Value::Null);

    status.publish_frame(
        LiveObservationFrame::new(8, vec![8], 1, 1, Instant::now()),
        Duration::ZERO,
        "test_capture",
    );
    let resumed = status.as_json(true, 15);
    assert_eq!(resumed["paused"], false);
    assert_eq!(resumed["pause_reason"], Value::Null);
    assert_eq!(resumed["latest_sequence"], 8);
}

#[rstest]
#[tokio::test]
async fn live_observation_pause_keeps_the_stream_but_never_returns_the_cached_frame() {
    let (mut observation, publisher) = LiveObservation::from_test_stream(44, 7);
    let stream_id = observation.stream_id();
    let mut receiver = observation.subscribe();
    publisher.pause(ComputerUseError::new(
        ComputerUseErrorCode::InteractiveDesktopUnavailable,
        "Windows interactive session disconnected",
    ));

    let paused = wait_for_latest_frame(&mut receiver, None, Duration::from_millis(10))
        .await
        .expect_err("a paused producer must not return its cached frame");
    assert_eq!(
        paused.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert_eq!(observation.stream_id(), stream_id);
    assert_eq!(observation.state()["active"], true);
    assert_eq!(observation.state()["paused"], true);

    publisher.publish_frame(8, "test_resume");
    let resumed = observation
        .latest_after(Some(7))
        .await
        .expect("a newer frame should resume the same stream");
    assert_eq!(resumed.sequence(), 8);
    assert_eq!(observation.stream_id(), stream_id);
    assert_eq!(observation.state()["paused"], false);
}

#[rstest]
fn live_observation_png_converts_bgra_to_rgba() {
    let png = encode_bgra_to_png(&[3, 2, 1, 4, 7, 6, 5, 8], 1, 2).unwrap();
    let mut reader = png::Decoder::new(Cursor::new(&png)).read_info().unwrap();
    let mut bytes = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut bytes).unwrap();
    assert_eq!(&bytes[..info.buffer_size()], &[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(
        decode_png_to_bgra(&png).unwrap(),
        (vec![3, 2, 1, 4, 7, 6, 5, 8], 1, 2)
    );
}

#[rstest]
fn portable_live_frame_preserves_the_validated_source_png() {
    let png = encode_bgra_to_png(&[3, 2, 1, 4], 1, 1).unwrap();

    let frame = LiveObservationFrame::from_png(1, png.clone(), Instant::now()).unwrap();

    assert_eq!(frame.dimensions(), (1, 1));
    assert_eq!(frame.encoded_png(), Some(png.as_slice()));
    assert_eq!(frame.bgra(), &[3, 2, 1, 4]);
}

#[rstest]
fn live_observation_keeps_only_the_latest_frame() {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1], 1, 1, Instant::now()),
        Duration::ZERO,
        "test_capture",
    );
    status.publish_frame(
        LiveObservationFrame::new(2, vec![2], 1, 1, Instant::now()),
        Duration::ZERO,
        "test_capture",
    );

    assert_eq!(status.latest().expect("latest frame").sequence(), 2);
    assert_eq!(status.as_json(true, 10)["frames_captured"], 2);
    assert_eq!(status.as_json(true, 10)["frames_replaced"], 1);
}

#[rstest]
fn live_observation_state_reports_recent_rate_and_capture_cost() {
    let started = Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1], 1, 1, started),
        Duration::from_millis(6),
        "test_capture",
    );
    status.publish_frame(
        LiveObservationFrame::new(2, vec![2], 1, 1, started + Duration::from_millis(100)),
        Duration::from_millis(8),
        "test_capture",
    );

    let state = status.as_json(true, 10);
    assert_eq!(state["active"], true);
    assert_eq!(state["target_fps"], 10);
    assert_eq!(state["recent_effective_fps"], 10.0);
    assert_eq!(state["last_capture_duration_ms"], 8);
    assert_eq!(state["max_capture_duration_ms"], 8);
    assert_eq!(state["capture_mode"], "test_capture");
}

#[rstest]
#[tokio::test]
async fn live_observation_first_frame_wait_preserves_the_terminal_error() {
    let mut status = LiveObservationStatus::default();
    status.record_terminal_error(&ComputerUseError::new(
        ComputerUseErrorCode::InvalidTarget,
        "live observation target process identity changed",
    ));
    let (_sender, mut receiver) = tokio::sync::watch::channel(status);

    let error = wait_for_latest_frame(&mut receiver, None, Duration::from_millis(10))
        .await
        .expect_err("terminal target loss must end the first-frame wait");

    assert_eq!(error.code, ComputerUseErrorCode::InvalidTarget);
    assert_eq!(
        error.message,
        "live observation target process identity changed"
    );
}

#[rstest]
#[tokio::test]
async fn live_observation_fresh_frame_wait_preserves_terminal_error_after_an_old_frame() {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(9, vec![9], 1, 1, Instant::now()),
        Duration::ZERO,
        "test_capture",
    );
    status.record_terminal_error(&ComputerUseError::new(
        ComputerUseErrorCode::InteractiveDesktopUnavailable,
        "Windows session is no longer actively connected",
    ));
    let (_sender, mut receiver) = tokio::sync::watch::channel(status);

    let error = wait_for_latest_frame(&mut receiver, Some(9), Duration::from_millis(10))
        .await
        .expect_err("terminal disconnect must end a wait for a frame newer than the old one");

    assert_eq!(
        error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert_eq!(
        error.message,
        "Windows session is no longer actively connected"
    );
}

#[rstest]
#[tokio::test]
async fn live_observation_never_returns_a_cached_frame_from_a_terminal_stream() {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(9, vec![9], 1, 1, Instant::now()),
        Duration::ZERO,
        "test_capture",
    );
    status.record_terminal_error(&ComputerUseError::new(
        ComputerUseErrorCode::InvalidTarget,
        "live observation target process identity changed",
    ));
    let (_sender, mut receiver) = tokio::sync::watch::channel(status);

    let error = wait_for_latest_frame(&mut receiver, None, Duration::from_millis(10))
        .await
        .expect_err("a cached pre-terminal frame must not masquerade as a fresh screenshot");

    assert_eq!(error.code, ComputerUseErrorCode::InvalidTarget);
}

#[rstest]
#[tokio::test]
async fn live_observation_returns_a_frame_newer_than_the_decision_frame() {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1], 1, 1, Instant::now()),
        Duration::ZERO,
        "test_capture",
    );
    let (sender, mut receiver) = tokio::sync::watch::channel(status);
    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(2, vec![2], 1, 1, Instant::now()),
            Duration::from_millis(7),
            "test_capture",
        );
    });

    let frame = wait_for_latest_frame(&mut receiver, Some(1), Duration::from_millis(10))
        .await
        .expect("fresh latest frame");
    assert_eq!(frame.sequence(), 2);
}

#[rstest]
#[tokio::test]
async fn live_observation_post_action_capture_skips_frames_available_at_action_completion() {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1], 1, 1, Instant::now()),
        Duration::ZERO,
        "test_capture",
    );
    let (sender, mut receiver) = tokio::sync::watch::channel(status);
    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(2, vec![2], 1, 1, Instant::now()),
            Duration::ZERO,
            "test_capture",
        );
    });

    let after_sequence = observation_sequence_fence(
        7,
        Some(LiveObservationFence::new(7, 1)),
        Some(LiveObservationFence::new(7, 2)),
        None,
    );
    let publish_after_action = tokio::spawn(async move {
        tokio::task::yield_now().await;
        sender.send_modify(|status| {
            status.publish_frame(
                LiveObservationFrame::new(3, vec![3], 1, 1, Instant::now()),
                Duration::ZERO,
                "test_capture",
            );
        });
    });

    let frame = wait_for_latest_frame(&mut receiver, after_sequence, Duration::from_millis(100))
        .await
        .expect("capture_after frame strictly newer than action completion");
    publish_after_action.await.unwrap();
    assert_eq!(frame.sequence(), 3);
}

#[rstest]
#[tokio::test]
#[case("input_resumed")]
#[case("target_restored")]
async fn live_observation_transition_fence_skips_frames_cached_before_safe_resume(
    #[case] _transition: &str,
) {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(2, vec![2], 1, 1, Instant::now()),
        Duration::ZERO,
        "suspended_capture",
    );
    let (sender, mut receiver) = tokio::sync::watch::channel(status);
    let after_sequence =
        observation_sequence_fence(7, None, None, Some(LiveObservationFence::new(7, 2)));
    let publish_after_resume = tokio::spawn(async move {
        tokio::task::yield_now().await;
        sender.send_modify(|status| {
            status.publish_frame(
                LiveObservationFrame::new(3, vec![3], 1, 1, Instant::now()),
                Duration::ZERO,
                "resumed_capture",
            );
        });
    });

    let frame = wait_for_latest_frame(&mut receiver, after_sequence, Duration::from_millis(100))
        .await
        .expect("resume/restore must wait for a frame newer than its transition fence");
    publish_after_resume.await.unwrap();
    assert_eq!(frame.sequence(), 3);
}

#[rstest]
fn live_observation_restart_drops_fences_from_the_previous_stream() {
    let after_sequence = observation_sequence_fence(
        8,
        Some(LiveObservationFence::new(7, 18_591)),
        Some(LiveObservationFence::new(7, 18_590)),
        None,
    );

    assert_eq!(after_sequence, None);
}
