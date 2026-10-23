use super::*;
use rstest::rstest;

#[path = "../../tests/fixtures/pixel_keypress.rs"]
mod fixture_keypress;

#[rstest]
fn native_fixture_f8_request_passes_host_and_pixel_policy_without_held_key_bypass() {
    let action = fixture_keypress::request("owned-native-observation".into());
    validate_action(&action)
        .expect("actual native fixture request must pass the Host action policy");
    validate_pixel_action(&action)
        .expect("actual native fixture request must pass the pixel route");
    assert_eq!(action.keys, ["F8"]);
    assert_eq!(action.duration_ms, None);
    let previous_held_f8 = ComputerUseAction {
        duration_ms: Some(80),
        ..action
    };
    assert_eq!(
        validate_action(&previous_held_f8).unwrap_err().code,
        ComputerUseErrorCode::InvalidAction
    );
}

#[rstest]
#[tokio::test]
async fn cancelled_async_waiter_keeps_worker_lease_until_release_and_postcheck() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    struct Lease(Arc<AtomicBool>);
    impl Drop for Lease {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let held = Arc::new(AtomicBool::new(false));
    let postcheck = Arc::new(AtomicBool::new(false));
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (finish_tx, finish_rx) = mpsc::channel();
    let (cleanup_tx, cleanup_rx) = tokio::sync::oneshot::channel();
    let (acquire_held, dispatch_held, finished) = (held.clone(), held.clone(), postcheck.clone());
    let waiter = tokio::spawn(run_owned_pixel_worker(
        move || {
            acquire_held.store(true, Ordering::Release);
            Ok(Lease(acquire_held))
        },
        move || {
            entered_tx.send(()).unwrap();
            finish_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            assert!(
                dispatch_held.load(Ordering::Acquire),
                "owned key release ran after restoration"
            );
            finished.store(true, Ordering::Release);
            assert!(
                dispatch_held.load(Ordering::Acquire),
                "native postcheck ran after restoration"
            );
            cleanup_tx.send(()).unwrap();
            Ok(())
        },
    ));
    entered_rx.await.unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    let retained = held.load(Ordering::Acquire);
    finish_tx.send(()).unwrap();
    cleanup_rx.await.unwrap();
    while held.load(Ordering::Acquire) {
        tokio::task::yield_now().await;
    }
    assert!(
        retained,
        "cancelling the await restored the presenter while native input was active"
    );
    assert!(postcheck.load(Ordering::Acquire));
}

#[rstest]
#[tokio::test]
async fn failed_owned_presentation_acquisition_never_enters_native_dispatch() {
    let result = run_owned_pixel_worker(
        || -> ComputerUseResult<()> {
            Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "renderer unavailable",
            ))
        },
        || -> ComputerUseResult<()> { panic!("native dispatch must not begin") },
    )
    .await;
    assert_eq!(
        result.unwrap_err().code,
        ComputerUseErrorCode::BackendUnavailable
    );
}

#[cfg(windows)]
#[rstest]
fn unavailable_owned_renderer_reports_a_known_zero_input_refusal() {
    let error = pixel_presentation_error(IndicatorError::Backend(
        "renderer stopped before ack".into(),
    ));
    assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
    let details = error.details.unwrap();
    assert_eq!(details.phase, Some(ComputerUseErrorPhase::PreDispatch));
    assert_eq!(details.action_attempted, Some(false));
    assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
    assert_eq!(details.effect_unknown, Some(false));
    assert_eq!(details.blind_retry, Some(false));
}

#[rstest]
#[case(7, 7, false, true)]
#[case(7, 8, false, false)]
#[case(8, 8, true, false)]
fn interruption_before_or_during_preflight_cannot_become_a_new_baseline(
    #[case] started: u64,
    #[case] current: u64,
    #[case] latched: bool,
    #[case] accepted: bool,
) {
    let result = validate_pixel_interrupt(started, current, latched);
    assert_eq!(result.is_ok(), accepted);
    if let Err(error) = result {
        let details = error.details.unwrap();
        assert_eq!(error.code, ComputerUseErrorCode::UserInterrupted);
        assert_eq!(details.action_attempted, Some(false));
        assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
    }
}

#[cfg(windows)]
#[rstest]
#[case(false, false, 0, ComputerUseInputState::NotSent)]
#[case(true, false, 0, ComputerUseInputState::Unknown)]
#[case(true, true, 0, ComputerUseInputState::Sent)]
#[case(false, false, 1, ComputerUseInputState::Sent)]
fn failed_input_preserves_partial_cursor_and_event_delivery(
    #[case] cursor_move_attempted: bool,
    #[case] cursor_move_succeeded: bool,
    #[case] inserted_events: u32,
    #[case] expected: ComputerUseInputState,
) {
    use dcc_cua_platform_windows::{
        WindowsGuardedInputError, WindowsGuardedInputOperation, WindowsGuardedInputOutcome,
        WindowsPhysicalInputFailureReason,
    };
    let attempted = cursor_move_attempted || inserted_events > 0;
    let error = input_error(WindowsGuardedInputError {
        reason: WindowsPhysicalInputFailureReason::Interrupted,
        native_input_fence: None,
        visibility_failure: None,
        outcome: WindowsGuardedInputOutcome {
            operation: WindowsGuardedInputOperation::Click,
            mutation_attempted: attempted,
            cursor_move_attempted,
            cursor_move_succeeded,
            requested_events: 2,
            inserted_events,
            cleanup_requested_events: 0,
            cleanup_inserted_events: 0,
            delivery_completed: false,
            post_dispatch_validated: false,
            fresh_observation_required: true,
        },
    });
    let details = error.details.unwrap();
    assert_eq!(details.input_sent, Some(expected));
    assert_eq!(details.action_attempted, Some(attempted));
    assert_eq!(details.effect_unknown, Some(attempted));
    assert_eq!(details.fresh_observation_required, Some(true));
    assert_eq!(details.blind_retry, Some(false));
}

#[cfg(windows)]
#[rstest]
fn rejected_fence_diagnostic_reaches_closed_error_details_with_original_refusal() {
    use dcc_cua_platform_windows::{
        WindowsGuardedInputError, WindowsGuardedInputOperation, WindowsGuardedInputOutcome,
        WindowsPhysicalInputFailureReason,
    };
    let sample = serde_json::from_value(serde_json::json!({
        "schema_version":1,"authorizes_input":false,"target_process_id":42,
        "target_window_handle":99,"point":[1461,1060],"unobscured":false,
        "point_proof_complete":true,"point_visible":false,"hit_window_handle":123,
        "hit_process_id":456,"hit_root_window_handle":123,"hit_is_target_or_child":false,
        "redacted_rectangles":[[1211,260,1778,1039]],"redaction_count":1,"redactions_complete":true
    }))
    .unwrap();
    let error = input_error(WindowsGuardedInputError {
        reason: WindowsPhysicalInputFailureReason::TargetOccluded,
        outcome: WindowsGuardedInputOutcome {
            operation: WindowsGuardedInputOperation::Click,
            mutation_attempted: false,
            cursor_move_attempted: false,
            cursor_move_succeeded: false,
            requested_events: 0,
            inserted_events: 0,
            cleanup_requested_events: 0,
            cleanup_inserted_events: 0,
            delivery_completed: false,
            post_dispatch_validated: false,
            fresh_observation_required: true,
        },
        native_input_fence: Some(sample),
        visibility_failure: Some(Box::new(
            dcc_cua_platform_windows::VisibleWindowCaptureDiagnostic {
                reason: dcc_cua_platform_windows::VisibleWindowCaptureReason::RootOverlap,
                target_bounds: Some([1211, 260, 1778, 1039]),
                blocker_process_id: Some(456),
                blocker_window_handle: Some(123),
                blocker_bounds: Some([1211, 260, 1778, 1039]),
                cloaked: Some(0),
                os_error: None,
                root_bounds_failure: None,
            },
        )),
    });
    assert_eq!(error.code, ComputerUseErrorCode::TargetUnavailable);
    let details = error.details.unwrap();
    assert_eq!(details.phase, Some(ComputerUseErrorPhase::PreDispatch));
    assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
    assert_eq!(
        details.capture.unwrap().reason,
        ComputerUseCaptureReason::RootOverlap
    );
    assert_eq!(
        details.native_input_fence.unwrap().hit_process_id,
        Some(456)
    );
}

#[rstest]
#[case("click", true)]
#[case("double_click", true)]
#[case("right_click", true)]
#[case("toggle", true)]
#[case("keypress", true)]
#[case("keyboard_shortcut", true)]
#[case("type", true)]
#[case("type_chars", true)]
#[case("drag", true)]
#[case("move", false)]
#[case("scroll", false)]
#[case("set_text", false)]
fn explicit_pixel_routes_are_closed(#[case] name: &str, #[case] accepted: bool) {
    let mut action = ComputerUseAction {
        action: name.into(),
        ..Default::default()
    };
    if matches!(name, "click" | "double_click" | "right_click" | "toggle") {
        action.x = Some(1.0);
        action.y = Some(2.0);
    }
    if name == "drag" {
        action.path = vec![
            ComputerUsePoint { x: 1.0, y: 2.0 },
            ComputerUsePoint { x: 3.0, y: 4.0 },
        ];
    }
    assert_eq!(validate_pixel_action(&action).is_ok(), accepted);
}

fn drag_action() -> ComputerUseAction {
    ComputerUseAction {
        action: "drag".into(),
        path: vec![
            ComputerUsePoint { x: 10.0, y: 20.0 },
            ComputerUsePoint { x: 110.0, y: 60.0 },
        ],
        ..Default::default()
    }
}

#[rstest]
#[case("empty")]
#[case("extra_point")]
#[case("coordinates")]
#[case("keys")]
#[case("modifiers")]
#[case("text")]
#[case("zero_duration")]
#[case("long_duration")]
#[case("zero_steps")]
#[case("many_steps")]
#[case("nonfinite")]
fn pixel_drag_rejects_unbounded_or_mixed_action_fields(#[case] variant: &str) {
    let mut action = drag_action();
    match variant {
        "empty" => action.path.clear(),
        "extra_point" => action.path.push(ComputerUsePoint { x: 2.0, y: 2.0 }),
        "coordinates" => action.x = Some(1.0),
        "keys" => action.keys.push("shift".into()),
        "modifiers" => action.modifiers.push("shift".into()),
        "text" => action.text = Some("NoOp".into()),
        "zero_duration" => action.duration_ms = Some(0),
        "long_duration" => action.duration_ms = Some(10_001),
        "zero_steps" => action.steps = Some(0),
        "many_steps" => action.steps = Some(201),
        "nonfinite" => action.path[1].x = f64::INFINITY,
        _ => unreachable!(),
    }
    assert_eq!(
        validate_pixel_action(&action).unwrap_err().code,
        ComputerUseErrorCode::InvalidAction
    );
}

#[cfg(windows)]
#[rstest]
#[case("valid", true)]
#[case("outside", false)]
#[case("same_pixel", false)]
#[case("redacted_sample", false)]
#[case("redacted_between_samples", false)]
#[case("redacted_elsewhere", true)]
#[case("redacted_excluded_corner", true)]
#[case("scaled", false)]
fn pixel_drag_checks_physical_origin_and_the_entire_redaction_path(
    #[case] variant: &str,
    #[case] accepted: bool,
) {
    let mut action = drag_action();
    action.steps = Some(1);
    let mut observation = observed();
    match variant {
        "outside" => action.path[1].x = 400.0,
        "same_pixel" => action.path[1] = ComputerUsePoint { x: 10.9, y: 20.1 },
        "redacted_sample" => {
            observation.capture_provenance["visibility"] =
                json!({"redacted_rectangles":[[-290,100,2,2]]})
        }
        "redacted_between_samples" => {
            observation.capture_provenance["visibility"] =
                json!({"redacted_rectangles":[[-241,119,3,3]]})
        }
        "redacted_elsewhere" => {
            observation.capture_provenance["visibility"] =
                json!({"redacted_rectangles":[[-241,140,3,3]]})
        }
        "redacted_excluded_corner" => {
            observation.capture_provenance["visibility"] =
                json!({"redacted_rectangles":[[-300,80,10,20]]})
        }
        "scaled" => observation.width /= 2,
        _ => {}
    }
    let result = physical_drag_points(&action, &observation);
    assert_eq!(result.is_ok(), accepted);
    if accepted {
        assert_eq!(result.unwrap(), vec![(-290, 100), (-190, 140)]);
    }
}

#[rstest]
#[case("selector")]
#[case("index")]
#[case("backend")]
#[case("background")]
#[case("duration")]
#[case("path")]
#[case("delay")]
fn pixel_input_never_falls_through_to_other_dispatchers(#[case] variant: &str) {
    let mut action = ComputerUseAction {
        action: "click".into(),
        x: Some(1.0),
        y: Some(2.0),
        ..Default::default()
    };
    match variant {
        "selector" => action.element_token = Some("other".into()),
        "index" => action.element_index = Some(1),
        "backend" => action.input_backend_id = Some("other".into()),
        "background" => action.delivery_mode = Some("background".into()),
        "duration" => action.duration_ms = Some(1),
        "path" => action.path.push(ComputerUsePoint { x: 1.0, y: 2.0 }),
        "delay" => action.delay_ms = Some(1),
        _ => unreachable!(),
    }
    assert_eq!(
        validate_pixel_action(&action).unwrap_err().code,
        ComputerUseErrorCode::InvalidAction
    );
}

#[cfg(windows)]
fn observed() -> ComputerUseObservation {
    ComputerUseObservation {
        observation_id: "pixel-1".into(),
        window_handle: 99,
        process_id: 42,
        window_title: String::new(),
        width: 400,
        height: 200,
        source_rect: [-300, 80, 400, 200],
        capture_backend: "dcc-cua-visible-exact-window".into(),
        session_id: "owned".into(),
        capture_provenance: json!({
            "process_id":42,"window_handle":99,"pixels_captured":true,"whole_desktop_capture":false,
            "scope":"window","capture_generation":1,"window_dpi":240,
            "native_window_bounds":[-310,70,420,220],"observation_mode":"pixels_only","accessibility_available":false,
            "native_visible_bounds":[-300,80,400,200],
            "native_instance":{"process_creation_time_100ns":7,"window_thread_id":8,
                "window_class_hash":9,"owner_window_handle":0}
        }),
    }
}

#[cfg(windows)]
#[rstest]
#[case("valid", true)]
#[case("missing_bounds", false)]
#[case("wrong_bounds", false)]
#[case("overflow_bounds", false)]
#[case("overflow_dpi", false)]
#[case("semantic", false)]
#[case("wrong_pid", false)]
#[case("wrong_hwnd", false)]
#[case("zero_source", false)]
#[case("scaled_source", false)]
#[case("missing_dwm", false)]
#[case("wrong_dwm", false)]
fn native_input_requires_the_actual_physical_capture_fence(
    #[case] variant: &str,
    #[case] accepted: bool,
) {
    let mut observation = observed();
    let mut target = WindowTarget {
        pid: 42,
        window_id: 99,
        title: String::new(),
        app_name: String::new(),
        bounds: [-310, 70, 420, 220],
        is_on_screen: true,
        is_minimized: false,
        z_index: None,
        is_foreground: true,
    };
    match variant {
        "missing_bounds" => observation.capture_provenance["native_window_bounds"] = Value::Null,
        "wrong_bounds" => target.bounds[0] += 1,
        "overflow_bounds" => {
            observation.capture_provenance["native_window_bounds"][0] = json!(2147483648_i64)
        }
        "overflow_dpi" => observation.capture_provenance["window_dpi"] = json!(4294967296_u64),
        "semantic" => observation.capture_provenance["observation_mode"] = json!("semantic"),
        "wrong_pid" => target.pid += 1,
        "wrong_hwnd" => target.window_id += 1,
        "zero_source" => observation.source_rect[2] = 0,
        "scaled_source" => observation.width /= 2,
        "missing_dwm" => observation.capture_provenance["native_visible_bounds"] = Value::Null,
        "wrong_dwm" => observation.capture_provenance["native_visible_bounds"][0] = json!(-299),
        _ => {}
    }
    let scope = ComputerUseTargetScope {
        process_id: Some(42),
        window_handle: Some(99),
        ..Default::default()
    };
    let action = ComputerUseAction {
        action: "click".into(),
        observation_id: Some("pixel-1".into()),
        ..Default::default()
    };
    assert_eq!(
        input_fence(&action, &observation, &scope, "owned", &target).is_ok(),
        accepted
    );
}

#[cfg(windows)]
#[rstest]
fn native_pixels_use_one_to_one_physical_origin_including_negative_monitors() {
    let observation = observed();
    let action = ComputerUseAction {
        action: "click".into(),
        x: Some(100.0),
        y: Some(50.0),
        ..Default::default()
    };
    assert_eq!(physical_point(&action, &observation).unwrap(), (-200, 130));
    let outside = ComputerUseAction {
        x: Some(400.0),
        ..action.clone()
    };
    assert_eq!(
        physical_point(&outside, &observation).unwrap_err().code,
        ComputerUseErrorCode::InvalidAction
    );
    let mut scaled = observation;
    scaled.width /= 2;
    assert_eq!(
        physical_point(&action, &scaled).unwrap_err().code,
        ComputerUseErrorCode::StaleObservation
    );
}

#[cfg(windows)]
#[rstest]
#[case("click")]
#[case("double_click")]
#[case("right_click")]
#[case("toggle")]
#[case("keypress")]
#[case("keyboard_shortcut")]
#[case("type")]
#[case("type_chars")]
fn every_pixel_action_requires_trusted_unscaled_capture_geometry(#[case] name: &str) {
    let scope = ComputerUseTargetScope {
        process_id: Some(42),
        window_handle: Some(99),
        ..Default::default()
    };
    let target = WindowTarget {
        pid: 42,
        window_id: 99,
        title: String::new(),
        app_name: String::new(),
        bounds: [-310, 70, 420, 220],
        is_on_screen: true,
        is_minimized: false,
        z_index: None,
        is_foreground: true,
    };
    let action = ComputerUseAction {
        action: name.into(),
        observation_id: Some("pixel-1".into()),
        ..Default::default()
    };
    assert!(input_fence(&action, &observed(), &scope, "owned", &target).is_ok());
    for variant in 0..2 {
        let mut observation = observed();
        if variant == 0 {
            observation.width /= 2;
        } else {
            observation.capture_provenance["native_visible_bounds"][0] = json!(-299);
        }
        assert_eq!(
            input_fence(&action, &observation, &scope, "owned", &target)
                .unwrap_err()
                .code,
            ComputerUseErrorCode::StaleObservation
        );
    }
}
