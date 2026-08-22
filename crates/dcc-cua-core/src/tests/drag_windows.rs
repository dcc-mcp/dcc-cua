use rstest::rstest;

use super::*;

#[rstest]
fn action_input_backend_id_is_a_bounded_wire_identifier() {
    let action: ComputerUseAction = serde_json::from_value(json!({
        "action": "drag",
        "input_backend_id": "windows.synthetic_touch.v1",
        "delivery_mode": "foreground",
        "path": [{"x": 1.0, "y": 2.0}, {"x": 3.0, "y": 4.0}]
    }))
    .unwrap();
    assert_eq!(
        action.input_backend_id.as_deref(),
        Some("windows.synthetic_touch.v1")
    );
    validate_action(&action).unwrap();

    for invalid in [
        "",
        ".windows.send_input.v1",
        "windows/send_input/v1",
        "WINDOWS.send_input.v1",
    ] {
        assert!(
            validate_action(&ComputerUseAction {
                action: "drag".into(),
                input_backend_id: Some(invalid.into()),
                delivery_mode: Some("foreground".into()),
                path: vec![
                    ComputerUsePoint { x: 1.0, y: 2.0 },
                    ComputerUsePoint { x: 3.0, y: 4.0 },
                ],
                ..Default::default()
            })
            .is_err(),
            "accepted invalid input backend id {invalid:?}"
        );
    }
    assert!(
        validate_action(&ComputerUseAction {
            action: "drag".into(),
            input_backend_id: Some("x".repeat(65)),
            delivery_mode: Some("foreground".into()),
            path: vec![
                ComputerUsePoint { x: 1.0, y: 2.0 },
                ComputerUsePoint { x: 3.0, y: 4.0 },
            ],
            ..Default::default()
        })
        .is_err()
    );
}

#[cfg(windows)]
#[rstest]
fn scoped_foreground_drag_without_an_explicit_backend_uses_the_combined_down_route() {
    let action: ComputerUseAction = serde_json::from_value(json!({
        "action": "drag",
        "delivery_mode": "foreground",
        "button": "left",
        "path": [{"x": 1.0, "y": 2.0}, {"x": 3.0, "y": 4.0}]
    }))
    .unwrap();
    validate_action(&action).unwrap();

    assert_eq!(
        select_windows_foreground_drag_backend(&action).unwrap(),
        WindowsForegroundDragBackend::CombinedDownDrag
    );
}

#[cfg(windows)]
#[rstest]
fn implicit_combined_down_drag_rejects_unsupported_shapes_without_legacy_fallback() {
    let action = || ComputerUseAction {
        action: "drag".into(),
        delivery_mode: Some("foreground".into()),
        path: vec![
            ComputerUsePoint { x: 1.0, y: 2.0 },
            ComputerUsePoint { x: 3.0, y: 4.0 },
        ],
        ..Default::default()
    };

    let mut right_button = action();
    right_button.button = Some("right".into());
    assert!(
        select_windows_foreground_drag_backend(&right_button)
            .unwrap_err()
            .contains("left-button")
    );

    let mut modified = action();
    modified.modifiers = vec!["shift".into()];
    assert!(
        select_windows_foreground_drag_backend(&modified)
            .unwrap_err()
            .contains("modifiers")
    );

    let mut multi_segment = action();
    multi_segment.path.push(ComputerUsePoint { x: 5.0, y: 6.0 });
    assert!(
        select_windows_foreground_drag_backend(&multi_segment)
            .unwrap_err()
            .contains("exactly two path points")
    );
}

#[cfg(windows)]
#[rstest]
fn explicit_windows_drag_backend_selection_never_falls_back() {
    let action = |backend: Option<&str>, button: Option<&str>, delivery: &str| ComputerUseAction {
        action: "drag".into(),
        delivery_mode: Some(delivery.into()),
        input_backend_id: backend.map(str::to_owned),
        button: button.map(str::to_owned),
        path: vec![
            ComputerUsePoint { x: 1.0, y: 2.0 },
            ComputerUsePoint { x: 3.0, y: 4.0 },
        ],
        ..Default::default()
    };

    assert_eq!(
        select_windows_foreground_drag_backend(&action(
            Some(WINDOWS_SEND_INPUT_BACKEND_ID),
            None,
            "foreground"
        ))
        .unwrap(),
        WindowsForegroundDragBackend::SendInput
    );
    assert_eq!(
        select_windows_foreground_drag_backend(&action(
            Some("windows.synthetic_touch.v1"),
            Some("left"),
            "foreground"
        ))
        .unwrap(),
        WindowsForegroundDragBackend::SyntheticTouch
    );
    assert_eq!(
        select_windows_foreground_drag_backend(&action(
            Some(WINDOWS_RELATIVE_SEND_INPUT_BACKEND_ID),
            Some("left"),
            "foreground"
        ))
        .unwrap(),
        WindowsForegroundDragBackend::RelativeSendInput
    );
    assert!(
        select_windows_foreground_drag_backend(&action(
            Some("windows.synthetic_touch.v1"),
            Some("right"),
            "foreground"
        ))
        .unwrap_err()
        .contains("left-button")
    );
    assert!(
        select_windows_foreground_drag_backend(&action(
            Some("windows.synthetic_touch.v1"),
            None,
            "background"
        ))
        .is_err()
    );
    assert!(
        select_windows_foreground_drag_backend(&action(
            Some("windows.unknown.v1"),
            None,
            "foreground"
        ))
        .unwrap_err()
        .contains("unsupported")
    );
    let mut semantic_drag = action(Some("windows.synthetic_touch.v1"), None, "foreground");
    semantic_drag.element_token = Some("element-token".into());
    assert!(
        select_windows_foreground_drag_backend(&semantic_drag)
            .unwrap_err()
            .contains("screenshot coordinates")
    );
}

#[cfg(windows)]
#[rstest]
fn combined_down_drag_route_is_left_foreground_and_two_point_only() {
    let action: ComputerUseAction = serde_json::from_value(json!({
        "action": "drag",
        "delivery_mode": "foreground",
        "input_backend_id": "windows.send_input.combined_down_drag.v1",
        "button": "left",
        "path": [{"x": 1.0, "y": 2.0}, {"x": 3.0, "y": 4.0}]
    }))
    .unwrap();
    validate_action(&action).unwrap();

    assert_eq!(
        format!(
            "{:?}",
            select_windows_foreground_drag_backend(&action).unwrap()
        ),
        "CombinedDownDrag"
    );

    let mut legacy_action = action.clone();
    legacy_action.input_backend_id = Some(WINDOWS_SEND_INPUT_BACKEND_ID.into());
    assert_eq!(
        select_windows_foreground_drag_backend(&legacy_action).unwrap(),
        WindowsForegroundDragBackend::SendInput
    );

    for (button, delivery) in [("right", "foreground"), ("left", "background")] {
        let mut rejected = action.clone();
        rejected.button = Some(button.into());
        rejected.delivery_mode = Some(delivery.into());
        assert!(select_windows_foreground_drag_backend(&rejected).is_err());
    }

    let mut modified = action.clone();
    modified.modifiers = vec!["shift".into()];
    assert!(
        select_windows_foreground_drag_backend(&modified)
            .unwrap_err()
            .contains("modifiers")
    );

    let mut multi_segment = action.clone();
    multi_segment.path.push(ComputerUsePoint { x: 5.0, y: 6.0 });
    assert!(
        select_windows_foreground_drag_backend(&multi_segment)
            .unwrap_err()
            .contains("exactly two path points")
    );
}

#[cfg(windows)]
#[rstest]
fn relative_drag_calibrates_accelerated_cursor_motion_to_the_exact_waypoint() {
    let positions = RefCell::new(VecDeque::from([(100, 100), (120, 100), (110, 100)]));
    let requested_deltas = RefCell::new(Vec::new());
    let settled_waypoints = RefCell::new(0_u32);

    let trace = run_windows_calibrated_relative_path(
        &[(110, 100)],
        4,
        0,
        || Ok(()),
        || {
            positions
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| "missing cursor sample".to_owned())
        },
        |dx, dy| {
            requested_deltas.borrow_mut().push((dx, dy));
            RelativeMoveInjection::accepted()
        },
        || *settled_waypoints.borrow_mut() += 1,
    );

    assert_eq!(requested_deltas.into_inner(), [(10, 0), (-5, 0)]);
    assert_eq!(trace["endpoint_reached"], true);
    assert_eq!(trace["endpoint_exact"], true);
    assert_eq!(trace["waypoints_reached"], 1);
    assert_eq!(trace["moves"].as_array().map(Vec::len), Some(2));
    assert_eq!(trace["moves"][0]["requested"], 1);
    assert_eq!(trace["moves"][0]["inserted"], 1);
    assert_eq!(trace["moves"][1]["residual"], json!([-10, 0]));
    assert_eq!(trace["moves"][1]["damping_applied"], json!([true, false]));
    assert_eq!(trace["waypoint_completions"][0]["rule"], "exact");
    assert_eq!(settled_waypoints.into_inner(), 1);
}

#[cfg(windows)]
#[rstest]
fn relative_drag_replays_r10_sign_flip_with_half_gain_damping() {
    // r10 oscillated around this exact waypoint because Windows applied roughly 2x motion:
    // [1434,831] + [8,-5] -> [1450,821], then [-8,5] -> [1434,831].
    let cursor = RefCell::new((1434_i32, 831_i32));
    let requested_deltas = RefCell::new(Vec::new());

    let trace = run_windows_calibrated_relative_path(
        &[(1442, 826)],
        4,
        0,
        || Ok(()),
        || Ok(*cursor.borrow()),
        |dx, dy| {
            requested_deltas.borrow_mut().push((dx, dy));
            let accelerated = |delta: i32| {
                if delta.unsigned_abs() > 1 {
                    delta * 2
                } else {
                    delta
                }
            };
            let mut position = cursor.borrow_mut();
            position.0 += accelerated(dx);
            position.1 += accelerated(dy);
            RelativeMoveInjection::accepted()
        },
        || {},
    );

    assert_eq!(requested_deltas.into_inner(), [(8, -5), (-4, 2), (0, 1)]);
    assert_eq!(*cursor.borrow(), (1442, 826));
    assert_eq!(trace["endpoint_reached"], true);
    assert_eq!(trace["endpoint_exact"], true);
    assert_eq!(trace["failure"], Value::Null);
    assert_eq!(trace["moves"][1]["residual"], json!([-8, 5]));
    assert_eq!(trace["moves"][1]["damping_applied"], json!([true, true]));
}

#[cfg(windows)]
#[rstest]
fn relative_drag_replays_r11_unit_stall_as_bounded_quantized_completion() {
    let cursor = RefCell::new((1462_i32, 814_i32));
    let requested_deltas = RefCell::new(Vec::new());

    let trace = run_windows_calibrated_relative_path(
        &[(1452, 820)],
        4,
        0,
        || Ok(()),
        || Ok(*cursor.borrow()),
        |dx, dy| {
            requested_deltas.borrow_mut().push((dx, dy));
            let next = match (dx, dy) {
                (-10, 6) => (1441, 826),
                (5, -3) => (1449, 822),
                (3, -2) => (1453, 819),
                (-1, 1) => (1453, 819),
                command => panic!("unexpected r11 replay command {command:?}"),
            };
            *cursor.borrow_mut() = next;
            RelativeMoveInjection::accepted()
        },
        || {},
    );

    assert_eq!(
        requested_deltas.into_inner(),
        [(-10, 6), (5, -3), (3, -2), (-1, 1)]
    );
    assert_eq!(*cursor.borrow(), (1453, 819));
    assert_eq!(trace["endpoint_reached"], true);
    assert_eq!(trace["endpoint_exact"], false);
    assert_eq!(trace["schema_version"], 4);
    assert_eq!(trace["all_waypoints_exact"], false);
    assert_eq!(trace["move_attempt_budget"], 4);
    assert_eq!(trace["move_attempts_used"], 4);
    assert_eq!(
        trace["completion_policy"],
        "configured_tolerance_or_intermediate_physical_pixel_or_observed_unit_stall"
    );
    assert_eq!(trace["quantized_stall_tolerance_px"], 1);
    assert_eq!(
        trace["waypoint_completions"][0],
        json!({
            "waypoint_index": 0,
            "target": [1452, 820],
            "actual": [1453, 819],
            "rule": "quantized_unit_stall",
            "max_error_px": 1,
            "attempt_budget": 4,
            "attempts_used": 4,
            "remaining_attempts": 0,
        })
    );
    assert_eq!(trace["moves"][3]["cursor_moved"], false);
}

#[cfg(windows)]
#[rstest]
fn relative_drag_replays_r12_with_a_bounded_fifth_attempt() {
    let cursor = RefCell::new((1810_i32, 836_i32));
    let requested_deltas = RefCell::new(Vec::new());

    let trace = run_windows_calibrated_relative_path(
        &[(1786, 845)],
        RELATIVE_DRAG_MAX_ATTEMPTS_PER_WAYPOINT,
        0,
        || Ok(()),
        || Ok(*cursor.borrow()),
        |dx, dy| {
            requested_deltas.borrow_mut().push((dx, dy));
            let next = match (dx, dy) {
                (-24, 9) => (1762, 854),
                (12, -4) => (1790, 846),
                (-2, -1) => (1788, 845),
                (-2, 0) => (1785, 845),
                (1, 0) => (1786, 845),
                command => panic!("unexpected r12 replay command {command:?}"),
            };
            *cursor.borrow_mut() = next;
            RelativeMoveInjection::accepted()
        },
        || {},
    );

    assert_eq!(
        requested_deltas.into_inner(),
        [(-24, 9), (12, -4), (-2, -1), (-2, 0), (1, 0)]
    );
    assert_eq!(*cursor.borrow(), (1786, 845));
    assert_eq!(trace["endpoint_reached"], true);
    assert_eq!(trace["endpoint_exact"], true);
    assert_eq!(trace["schema_version"], 4);
    assert_eq!(trace["all_waypoints_exact"], true);
    assert_eq!(trace["max_attempts_per_waypoint"], 6);
    assert_eq!(trace["move_attempt_budget"], 6);
    assert_eq!(trace["move_attempts_used"], 5);
    assert_eq!(
        trace["waypoint_completions"][0],
        json!({
            "waypoint_index": 0,
            "target": [1786, 845],
            "actual": [1786, 845],
            "rule": "exact",
            "max_error_px": 0,
            "attempt_budget": 6,
            "attempts_used": 5,
            "remaining_attempts": 1,
        })
    );
}

#[cfg(windows)]
#[rstest]
fn relative_drag_replays_r13_without_damping_residual_two_into_the_unit_deadzone() {
    let cursor = RefCell::new((1050_i32, 1258_i32));
    let requested_deltas = RefCell::new(Vec::new());

    let trace = run_windows_calibrated_relative_path(
        &[(1025, 1258), (1024, 1258)],
        RELATIVE_DRAG_MAX_ATTEMPTS_PER_WAYPOINT,
        0,
        || Ok(()),
        || Ok(*cursor.borrow()),
        |dx, dy| {
            requested_deltas.borrow_mut().push((dx, dy));
            let next = match (dx, dy) {
                (-25, 0) => (983, 1258),
                (21, 0) => (1053, 1258),
                (-14, 0) => (1021, 1258),
                (2, 0) => (1022, 1258),
                (3, 0) => (1027, 1258),
                (-1, 0) => (1027, 1258),
                (-2, 0) => (1024, 1258),
                command => panic!("unexpected r13 replay command {command:?}"),
            };
            *cursor.borrow_mut() = next;
            RelativeMoveInjection::accepted()
        },
        || {},
    );

    assert_eq!(
        requested_deltas.into_inner(),
        [(-25, 0), (21, 0), (-14, 0), (2, 0), (3, 0), (-2, 0)]
    );
    assert_eq!(*cursor.borrow(), (1024, 1258));
    assert_eq!(trace["endpoint_reached"], true);
    assert_eq!(trace["endpoint_exact"], true);
    assert_eq!(trace["all_waypoints_exact"], false);
    assert_eq!(trace["schema_version"], 4);
    assert_eq!(trace["intermediate_tolerance_px"], 1);
    assert_eq!(trace["damping_min_effective_command_px"], 2);
    assert_eq!(trace["stagnation_escape_max_residual_px"], 3);
    assert_eq!(trace["stagnation_escape_max_command_px"], 4);
    assert_eq!(
        trace["waypoint_completions"][0]["rule"],
        "intermediate_physical_pixel"
    );
    assert_eq!(trace["waypoint_completions"][0]["max_error_px"], 1);
    assert_eq!(trace["waypoint_completions"][0]["attempts_used"], 6);
    assert_eq!(trace["waypoint_completions"][1]["rule"], "exact");
    assert_eq!(trace["moves"][5]["delta"], json!([-2, 0]));
    assert_eq!(trace["moves"][5]["damping_applied"], json!([true, false]));
    assert_eq!(
        trace["moves"][5]["stagnation_escape_applied"],
        json!([false, false])
    );
}

#[cfg(windows)]
#[rstest]
fn relative_drag_replays_r15_nonlinear_gain_without_command_divergence() {
    // r15's 4K RDP session amplified the same waypoint correction more on every move:
    // 1337 -29-> 1245 +31-> 1399 -45-> 1151 +78-> 1621 -156-> 627 +340-> 2857.
    // Scale arbitrary replacement commands by those measured per-attempt gains so the test
    // remains about convergence instead of prescribing the controller's implementation.
    let measured_gain_trace = [
        (29_i32, 92_i32),
        (31, 154),
        (45, 248),
        (78, 470),
        (156, 994),
        (340, 2_230),
    ];
    let cursor = RefCell::new((1_337_i32, 630_i32));
    let cursor_reads = RefCell::new(0_usize);
    let requested_deltas = RefCell::new(Vec::new());

    let trace = run_windows_calibrated_relative_path(
        &[(1_308, 630)],
        RELATIVE_DRAG_MAX_ATTEMPTS_PER_WAYPOINT,
        0,
        || Ok(()),
        || {
            *cursor_reads.borrow_mut() += 1;
            Ok(*cursor.borrow())
        },
        |dx, dy| {
            assert_eq!(dy, 0);
            let attempt = requested_deltas.borrow().len();
            requested_deltas.borrow_mut().push((dx, dy));
            let displacement = if dx.unsigned_abs() <= 2 {
                dx
            } else {
                let (measured_command, measured_displacement) = measured_gain_trace[attempt];
                let magnitude = (i64::from(dx).abs() * i64::from(measured_displacement)
                    + i64::from(measured_command) / 2)
                    / i64::from(measured_command);
                i64::from(dx.signum())
                    .saturating_mul(magnitude)
                    .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
            };
            cursor.borrow_mut().0 += displacement;
            RelativeMoveInjection::accepted()
        },
        || {},
    );

    let requested_deltas = requested_deltas.into_inner();
    assert_eq!(requested_deltas.first(), Some(&(-29, 0)));
    assert!(
        requested_deltas
            .iter()
            .all(|(dx, dy)| dx.unsigned_abs() <= 29 && *dy == 0),
        "adaptive commands escaped the initial 29px trust region: {requested_deltas:?}"
    );
    assert_eq!(*cursor.borrow(), (1_308, 630));
    assert_eq!(trace["endpoint_reached"], true);
    assert_eq!(trace["endpoint_exact"], true);
    assert_eq!(trace["failure"], Value::Null);
    assert_eq!(
        trace["fence_checks"].as_u64(),
        trace["move_attempts_used"].as_u64()
    );
    assert_eq!(
        *cursor_reads.borrow(),
        trace["move_attempts_used"].as_u64().unwrap() as usize + 1
    );
    assert!(trace["moves"].as_array().unwrap().iter().all(|movement| {
        movement["requested"] == 1 && movement["inserted"] == 1 && movement["after"].is_array()
    }));
}

#[cfg(windows)]
#[rstest]
fn relative_drag_escapes_a_small_observed_deadzone_without_extra_attempts() {
    let cursor = RefCell::new((102_i32, 100_i32));
    let requested_deltas = RefCell::new(Vec::new());

    let trace = run_windows_calibrated_relative_path(
        &[(100, 100), (99, 100)],
        2,
        0,
        || Ok(()),
        || Ok(*cursor.borrow()),
        |dx, dy| {
            requested_deltas.borrow_mut().push((dx, dy));
            match (dx, dy) {
                (-2, 0) => {}
                (-3, 0) => *cursor.borrow_mut() = (99, 100),
                command => panic!("unexpected deadzone escape command {command:?}"),
            }
            RelativeMoveInjection::accepted()
        },
        || {},
    );

    assert_eq!(requested_deltas.into_inner(), [(-2, 0), (-3, 0)]);
    assert_eq!(trace["endpoint_reached"], true);
    assert_eq!(trace["endpoint_exact"], true);
    assert_eq!(trace["all_waypoints_exact"], false);
    assert_eq!(
        trace["moves"][1]["stagnation_escape_applied"],
        json!([true, false])
    );
    assert_eq!(
        trace["waypoint_completions"][0]["rule"],
        "intermediate_physical_pixel"
    );
}

#[cfg(windows)]
#[rstest]
fn relative_drag_does_not_treat_a_moving_cursor_with_one_pixel_error_as_quantized() {
    let positions = RefCell::new(VecDeque::from([(101, 100), (99, 100)]));

    let trace = run_windows_calibrated_relative_path(
        &[(100, 100)],
        1,
        0,
        || Ok(()),
        || {
            positions
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| "missing cursor sample".to_owned())
        },
        |dx, dy| {
            assert_eq!((dx, dy), (-1, 0));
            RelativeMoveInjection::accepted()
        },
        || panic!("a moving one-pixel miss must not settle"),
    );

    assert_eq!(trace["endpoint_reached"], false);
    assert_eq!(trace["endpoint_exact"], false);
    assert_eq!(trace["failure"], "endpoint_not_reached");
    assert_eq!(trace["moves"][0]["cursor_moved"], true);
    assert_eq!(trace["waypoint_completions"], json!([]));
}

#[cfg(windows)]
#[rstest]
fn relative_drag_fails_closed_when_cursor_calibration_does_not_converge() {
    let positions = RefCell::new(VecDeque::from([(100, 100), (100, 100), (100, 100)]));
    let requested_deltas = RefCell::new(Vec::new());

    let trace = run_windows_calibrated_relative_path(
        &[(108, 100)],
        2,
        0,
        || Ok(()),
        || {
            positions
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| "missing cursor sample".to_owned())
        },
        |dx, dy| {
            requested_deltas.borrow_mut().push((dx, dy));
            RelativeMoveInjection::accepted()
        },
        || {},
    );

    assert_eq!(trace["endpoint_reached"], false);
    assert_eq!(trace["waypoints_reached"], 0);
    assert_eq!(trace["failure"], "endpoint_not_reached");
    assert_eq!(trace["moves"].as_array().map(Vec::len), Some(2));
    assert_eq!(trace["move_attempt_budget"], 2);
    assert_eq!(trace["move_attempts_used"], 2);
    assert_eq!(requested_deltas.into_inner(), [(8, 0), (8, 0)]);
    assert_eq!(trace["moves"][1]["damping_applied"], json!([false, false]));
}

#[cfg(windows)]
#[rstest]
fn relative_drag_stops_on_the_first_incomplete_send_input_move() {
    let cursor_samples = RefCell::new(VecDeque::from([(100, 100)]));
    let injections = RefCell::new(0_u32);

    let trace = run_windows_calibrated_relative_path(
        &[(110, 100)],
        4,
        0,
        || Ok(()),
        || {
            cursor_samples
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| "unexpected cursor read after rejected injection".to_owned())
        },
        |_, _| {
            *injections.borrow_mut() += 1;
            RelativeMoveInjection::incomplete(0, "SendInput inserted 0/1")
        },
        || panic!("a rejected waypoint must not settle"),
    );

    assert_eq!(injections.into_inner(), 1);
    assert_eq!(trace["endpoint_reached"], false);
    assert_eq!(trace["failure"], "send_input_incomplete");
    assert_eq!(trace["moves"][0]["requested"], 1);
    assert_eq!(trace["moves"][0]["inserted"], 0);
}

#[cfg(windows)]
#[rstest]
fn relative_drag_stops_before_move_when_the_target_fence_is_lost() {
    let injections = RefCell::new(0_u32);

    let trace = run_windows_calibrated_relative_path(
        &[(110, 100)],
        4,
        0,
        || Err("exact target HWND lost foreground".to_owned()),
        || Ok((100, 100)),
        |_, _| {
            *injections.borrow_mut() += 1;
            RelativeMoveInjection::accepted()
        },
        || panic!("a fenced-out waypoint must not settle"),
    );

    assert_eq!(injections.into_inner(), 0);
    assert_eq!(trace["endpoint_reached"], false);
    assert_eq!(trace["failure"], "target_fence_lost");
    assert_eq!(trace["failure_detail"], "exact target HWND lost foreground");
    assert_eq!(trace["moves"], json!([]));
}

#[cfg(windows)]
#[rstest]
fn synthetic_touch_result_reports_api_acceptance_without_claiming_effect() {
    let accepted = windows_synthetic_touch_result(Ok(()), &test_window_target(), true);
    assert_eq!(accepted.value["success"], true);
    assert!(!accepted.degraded);
    assert_eq!(
        accepted.value["delivery"],
        json!({
            "mode": "foreground",
            "backend_id": WINDOWS_SYNTHETIC_TOUCH_BACKEND_ID,
            "api_accepted": true,
            "consumer_effect_confirmed": false,
            "completion_known": false,
            "verification_required": true,
            "retry_safe": false,
            "target_fence": {
                "process_id": 42,
                "window_handle": 7,
                "exact_window": true,
                "foreground_required": true,
                "foreground_verified": true
            }
        })
    );
    assert_eq!(accepted.value["effect"], "unverifiable");

    let rejected = windows_synthetic_touch_result(
        Err("InjectSyntheticPointerInput failed".into()),
        &test_window_target(),
        true,
    );
    assert_eq!(rejected.value["success"], false);
    assert_eq!(rejected.value["delivery"]["api_accepted"], false);
    assert_eq!(
        rejected.value["delivery"]["backend_error"],
        "InjectSyntheticPointerInput failed"
    );
    assert!(rejected.degraded);

    let foreground_lost = windows_synthetic_touch_result(
        Err("exact foreground target was lost".into()),
        &test_window_target(),
        false,
    );
    assert_eq!(
        foreground_lost.value["delivery"]["target_fence"]["foreground_verified"],
        false
    );
}
