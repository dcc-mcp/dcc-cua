use std::cell::{Cell, RefCell};
#[cfg(windows)]
use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dcc_cua_indicator::{BannerActivity, IndicatorError};
use rstest::rstest;
use serde_json::{Value, json};

use super::*;
use crate::contracts::{
    DEFAULT_SNAPSHOT_MAX_DEPTH, DEFAULT_SNAPSHOT_MAX_ELEMENTS, MAX_SNAPSHOT_DEPTH,
    MAX_SNAPSHOT_ELEMENTS, MAX_TEXT_UTF16_UNITS, MOUSE_CURSOR_THEME,
};
use crate::driver_factory::{
    BUNDLED_CURSOR_THEME, UPSTREAM_CURSOR_RENDERER_ENABLED, driver_host_options,
};
use crate::interactive_desktop::{
    platform_managed_diagnostic, require_desktop_observation_from,
    require_exact_window_observation_from, require_input_available_from,
    require_window_activation_from, windows_diagnostic_base,
    windows_diagnostic_with_thread_fallback,
};
use crate::live_observation::{
    CaptureFailureDisposition, LiveObservation, LiveObservationFence, LiveObservationFrame,
    LiveObservationStatus, decode_png_to_bgra, live_capture_failure_disposition,
    observation_sequence_fence, terminal_capture_error, wait_for_latest_frame,
};
use crate::policy::*;

#[cfg(windows)]
#[rstest]
fn held_key_wait_stops_when_the_control_banner_is_interrupted() {
    let started = dcc_cua_indicator::interrupt_generation().wrapping_add(1);

    assert!(crate::runtime::windows_held_key::wait_for_held_key_duration(1_000, started));
}

#[cfg(windows)]
#[rstest]
fn held_key_wait_completes_without_an_interrupt() {
    let started = dcc_cua_indicator::interrupt_generation();

    assert!(!crate::runtime::windows_held_key::wait_for_held_key_duration(0, started));
}
#[cfg(windows)]
use crate::runtime::RawDragSequenceOutcome;
use crate::runtime::{
    ActionBannerPhase, CombinedDownDragAfterDown, CombinedDownDragCleanup, CombinedDownDragPrelude,
    CombinedDownInjection, LiveObservationStartDisposition, RecordingHealth, RecordingKeepalive,
    SingleInputInjection, WindowWaitProbeOutcome, aggregate_recording_state, attach_banner_status,
    attach_indicator_motion_to_activation, banner_activity_for_action_phase,
    banner_activity_for_bound_tool, diagnostic_tool_check, ensure_target_available_for_action,
    gated_cursor_operation, gated_desktop_observation, gated_exact_window_observation,
    gated_exact_window_publication, held_coordinate_click_as_drag, input_backend_rejection_result,
    live_observation_start_disposition, map_indicator_error, preflight_live_observation_start,
    run_windows_combined_down_drag_sequence, run_windows_fenced_absolute_path,
    run_windows_fenced_absolute_path_with_trace, run_windows_separated_raw_drag_sequence,
    tool_schema_from_inventory, wait_for_window_probe_until,
};
#[cfg(windows)]
use crate::runtime::{
    RELATIVE_DRAG_MAX_ATTEMPTS_PER_WAYPOINT, RelativeMoveInjection, WindowsForegroundDragBackend,
    WindowsPostButtonUpSnapshot, WindowsRawDragInputTrace,
    inject_windows_combined_input_batch_with, map_windows_window_mutation_error,
    run_windows_calibrated_relative_path, select_windows_foreground_drag_backend,
    uses_windows_foreground_fast_path, uses_windows_local_foreground_path,
    windows_combined_raw_drag_outcome, windows_combined_source_move_and_left_down_inputs,
    windows_raw_drag_delivery, windows_synthetic_touch_attempt, windows_synthetic_touch_result,
};
use crate::window_target::{WindowTarget, validate_target_policy};

macro_rules! run_combined_down_drag_sequence {
    (
        $inspect_pre:expr,
        $allow_pre:expr,
        $inject_batch:expr,
        $inspect_after:expr,
        $allow_path:expr,
        $move_path:expr,
        $settle:expr,
        $inject_up:expr,
        $inspect_post:expr,
        $allow_release:expr $(,)?
    ) => {
        run_windows_combined_down_drag_sequence(
            CombinedDownDragPrelude::new($inspect_pre, $allow_pre, $inject_batch),
            CombinedDownDragAfterDown::new($inspect_after, $allow_path),
            $move_path,
            $settle,
            CombinedDownDragCleanup::new($inject_up, $inspect_post, $allow_release),
        )
    };
}

mod drag;
mod drag_windows;
mod error_contracts;
mod interactive_desktop_fallback;
mod issues_58_60;
mod launch;
mod live_observation;
mod recording_session;

#[rstest]
#[tokio::test(start_paused = true)]
async fn window_wait_probe_obeys_the_absolute_request_deadline() {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(25);

    let outcome = wait_for_window_probe_until(deadline, async {
        tokio::time::sleep(Duration::from_secs(30)).await;
        7_u8
    })
    .await;

    assert!(matches!(outcome, WindowWaitProbeOutcome::TimedOut));
}

#[rstest]
#[case("click", ActionBannerPhase::Preparing, BannerActivity::Operating)]
#[case("type", ActionBannerPhase::Preparing, BannerActivity::Operating)]
#[case("click", ActionBannerPhase::Injecting, BannerActivity::PointerInput)]
#[case("type", ActionBannerPhase::Injecting, BannerActivity::KeyboardInput)]
fn action_banner_activity_distinguishes_preparation_from_real_injection(
    #[case] action: &str,
    #[case] phase: ActionBannerPhase,
    #[case] expected: BannerActivity,
) {
    assert_eq!(
        banner_activity_for_action_phase(
            &ComputerUseAction {
                action: action.into(),
                ..Default::default()
            },
            phase,
        ),
        expected
    );
}

#[rstest]
#[case("browser_navigate", BannerActivity::Navigating)]
#[case("get_browser_state", BannerActivity::Observing)]
#[case("browser_type", BannerActivity::Operating)]
#[case("unknown_extension", BannerActivity::Operating)]
fn opaque_input_tools_never_claim_a_specific_injection_state(
    #[case] name: &str,
    #[case] expected: BannerActivity,
) {
    assert_eq!(banner_activity_for_bound_tool(name), expected);
}

#[rstest]
#[tokio::test]
async fn cursor_input_gate_blocks_only_real_pointer_movement_before_backend_execution() {
    let move_called = Cell::new(false);
    let denied = || {
        Err(ComputerUseError::new(
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
            "Windows input surface is unavailable",
        ))
    };
    let move_error = gated_cursor_operation(true, denied, || {
        move_called.set(true);
        async { Ok::<_, ComputerUseError>(()) }
    })
    .await
    .expect_err("move_cursor must stop at the input gate");
    assert_eq!(
        move_error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert!(!move_called.get());

    let visual_called = Cell::new(false);
    gated_cursor_operation(false, denied, || {
        visual_called.set(true);
        async { Ok::<_, ComputerUseError>(()) }
    })
    .await
    .expect("cursor marker/theme/state tools remain available without raw input");
    assert!(visual_called.get());
}

#[rstest]
#[case(
    IndicatorError::InvalidTarget("gone".into()),
    ComputerUseErrorCode::InvalidTarget
)]
#[case(
    IndicatorError::Backend("paint failed".into()),
    ComputerUseErrorCode::BackendUnavailable
)]
fn indicator_errors_keep_typed_core_failure_semantics(
    #[case] error: IndicatorError,
    #[case] expected: ComputerUseErrorCode,
) {
    assert_eq!(map_indicator_error("start banner", error).code, expected);
}

#[rstest]
fn banner_debug_state_is_attached_without_hiding_upstream_session_state() {
    let state = attach_banner_status(
        json!({"active": true, "capture_scope": "window"}),
        json!({
            "activity": "observing",
            "recording": true,
            "motion": {
                "requested": "auto",
                "resolved": "reduce",
                "motion_enabled": false,
                "source": "system_preference"
            }
        }),
    );

    assert_eq!(state["active"], true);
    assert_eq!(state["capture_scope"], "window");
    assert_eq!(state["banner"]["activity"], "observing");
    assert_eq!(state["banner"]["recording"], true);
    assert_eq!(state["banner"]["motion"]["requested"], "auto");
    assert_eq!(state["banner"]["motion"]["resolved"], "reduce");
    assert_eq!(state["banner"]["motion"]["motion_enabled"], false);
    assert_eq!(state["banner"]["motion"]["source"], "system_preference");
}

#[rstest]
fn banner_debug_state_wraps_non_object_upstream_values() {
    let state = attach_banner_status(json!(null), json!({"activity": "ready"}));

    assert_eq!(state["cua"], Value::Null);
    assert_eq!(state["banner"]["activity"], "ready");
}

#[rstest]
fn bootstrap_activation_evidence_reports_the_resolved_indicator_motion() {
    let activation = attach_indicator_motion_to_activation(
        json!({"success": true, "target": {"pid": 42, "window_id": 31337}}),
        &json!({
            "motion": {
                "requested": "animate",
                "resolved": "animate",
                "motion_enabled": true,
                "source": "session_override"
            }
        }),
    );

    assert_eq!(activation["indicator_motion"]["requested"], "animate");
    assert_eq!(activation["indicator_motion"]["resolved"], "animate");
    assert_eq!(activation["indicator_motion"]["motion_enabled"], true);
    assert_eq!(activation["indicator_motion"]["source"], "session_override");
}

#[cfg(windows)]
#[rstest]
fn windows_fast_route_is_bounded_to_foreground_raw_actions() {
    assert!(uses_windows_foreground_fast_path(&ComputerUseAction {
        action: "click".into(),
        delivery_mode: Some("foreground".into()),
        ..Default::default()
    }));
    assert!(!uses_windows_foreground_fast_path(&ComputerUseAction {
        action: "click".into(),
        delivery_mode: Some("background".into()),
        ..Default::default()
    }));
    assert!(!uses_windows_foreground_fast_path(&ComputerUseAction {
        action: "type".into(),
        delivery_mode: Some("foreground".into()),
        ..Default::default()
    }));
    assert!(uses_windows_foreground_fast_path(&ComputerUseAction {
        action: "keypress".into(),
        delivery_mode: Some("foreground".into()),
        ..Default::default()
    }));
}

#[cfg(windows)]
#[rstest]
fn windows_local_route_includes_foreground_cursor_move() {
    assert!(uses_windows_local_foreground_path(&ComputerUseAction {
        action: "move".into(),
        delivery_mode: Some("foreground".into()),
        ..Default::default()
    }));
    assert!(!uses_windows_local_foreground_path(&ComputerUseAction {
        action: "move".into(),
        delivery_mode: Some("background".into()),
        ..Default::default()
    }));
}

#[rstest]
fn post_click_focus_loss_is_sent_but_never_retry_safe() {
    let outcome = windows_post_input_focus_loss(
        "foreground_unavailable: exact target HWND 0x3b61b48 or a verified same-process \
         post-action window was not foreground after the click \
         (actual foreground HWND 0x2792020)",
    )
    .expect("post-input focus loss must be classified");

    assert_eq!(
        outcome.actual_foreground_window.as_deref(),
        Some("0x2792020")
    );
    assert!(outcome.input_sent);
    assert!(!outcome.delivery_confirmed);
    assert!(!outcome.retry_safe);
    assert!(outcome.verification_required);
    assert_eq!(outcome.effect, "unverifiable");
}

#[rstest]
fn pre_input_activation_failure_remains_a_hard_error() {
    assert!(
        windows_post_input_focus_loss(
            "foreground_unavailable: Windows did not activate exact target HWND 0x3b61b48"
        )
        .is_none()
    );
}

#[cfg(windows)]
#[rstest]
#[case(
    "target_minimized: exact activation target must be visible",
    ComputerUseErrorCode::TargetMinimized
)]
#[case(
    "the exact activation target no longer exists or belongs to the granted process",
    ComputerUseErrorCode::TargetUnavailable
)]
fn window_mutation_identity_fences_keep_typed_target_failures(
    #[case] message: &str,
    #[case] expected: ComputerUseErrorCode,
) {
    let error = map_windows_window_mutation_error(
        "activate exact target",
        dcc_cua_platform_windows::UiaError::InvalidTarget(message.into()),
    );

    assert_eq!(error.code, expected);
}

#[cfg(windows)]
#[rstest]
fn late_synthetic_touch_input_gate_stays_typed_and_never_injects() {
    let injection_calls = Cell::new(0_u8);
    let error = windows_synthetic_touch_attempt(
        Err(ComputerUseError::new(
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
            "input_gate_stage=synthetic_touch_activation: secure desktop",
        )),
        || {
            injection_calls.set(injection_calls.get() + 1);
            Ok(())
        },
        &test_window_target(),
    )
    .expect_err("a late input gate must remain a typed hard error");

    assert_eq!(
        error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert_eq!(injection_calls.get(), 0);
}

#[rstest]
fn held_coordinate_click_reuses_the_drag_contract() {
    let drag = held_coordinate_click_as_drag(&ComputerUseAction {
        action: "click".into(),
        x: Some(12.0),
        y: Some(34.0),
        duration_ms: Some(320),
        ..Default::default()
    })
    .expect("held click becomes a stationary drag");

    assert_eq!(drag.action, "drag");
    assert_eq!(drag.path, vec![ComputerUsePoint { x: 12.0, y: 34.0 }; 2]);
    assert_eq!(drag.duration_ms, Some(320));
    assert_eq!(drag.steps, Some(20));
}

#[rstest]
fn host_runtime_uses_the_upstream_cursor_renderer_only_where_it_can_run() {
    let options = driver_host_options();
    assert_eq!(
        UPSTREAM_CURSOR_RENDERER_ENABLED,
        cfg!(any(windows, target_os = "linux"))
    );
    assert_eq!(options.cursor.enabled, UPSTREAM_CURSOR_RENDERER_ENABLED);
    assert!(options.host_owns_permission_ux);
    assert!(options.prepare_desktop_environment);
}

#[rstest]
fn bundled_cursor_theme_matches_runtime_contract() {
    let theme = cursor_overlay::decode_theme(BUNDLED_CURSOR_THEME).expect("valid bundled theme");
    assert_eq!(theme.id, MOUSE_CURSOR_THEME);
    assert_eq!(theme.actions.len(), 12);
}

#[rstest]
fn snapshot_bounds_use_agent_defaults_and_cap_context() {
    assert_eq!(bounded_snapshot_elements(0), DEFAULT_SNAPSHOT_MAX_ELEMENTS);
    assert_eq!(bounded_snapshot_depth(0), DEFAULT_SNAPSHOT_MAX_DEPTH);
    assert_eq!(bounded_snapshot_elements(u32::MAX), MAX_SNAPSHOT_ELEMENTS);
    assert_eq!(bounded_snapshot_depth(u32::MAX), MAX_SNAPSHOT_DEPTH);
}

#[rstest]
fn diagnostics_prefer_upstream_structured_content() {
    let check = diagnostic_tool_check(Ok(ComputerUseToolResult {
        value: json!({"structuredContent":{"overall":"ok"}}),
        text: "healthy".into(),
        images: Vec::new(),
        degraded: false,
    }));
    assert_eq!(check["success"], true);
    assert_eq!(check["result"]["overall"], "ok");
    assert_eq!(check["summary"], "healthy");

    let failed = diagnostic_tool_check(Err(ComputerUseError::new(
        ComputerUseErrorCode::BackendUnavailable,
        "screen capture unavailable",
    )));
    assert_eq!(failed["success"], false);
    assert_eq!(failed["code"], "backend_unavailable");
}

#[rstest]
fn diagnostic_health_routes_distinguish_visual_capture_from_uia() {
    let health = json!({
        "result": {
            "checks": [
                {"name": "screen_capture_capability", "status": "pass"},
                {"name": "ax_capability", "status": "fail"}
            ]
        }
    });

    assert!(diagnostic_health_check_passes(
        &health,
        "screen_capture_capability"
    ));
    assert!(!diagnostic_health_check_passes(&health, "ax_capability"));
    assert!(!diagnostic_health_check_passes(&health, "missing"));
}

#[rstest]
fn platform_managed_desktop_preserves_the_portable_input_contract() {
    let diagnostic = platform_managed_diagnostic();

    assert!(require_input_available_from(&diagnostic).is_ok());
    assert_eq!(diagnostic["input_ready"], true);
}

#[rstest]
fn active_default_input_desktop_without_foreground_is_ready() {
    let diagnostic = windows_diagnostic_base(Ok(0), Ok(Some("Default")), Ok(()), false);

    assert_eq!(diagnostic["success"], true);
    assert_eq!(diagnostic["code"], "interactive_desktop_ready");
    assert_eq!(diagnostic["observation_ready"], true);
    assert_eq!(diagnostic["input_ready"], true);
    assert_eq!(diagnostic["input_surface_ready"], true);
    assert_eq!(diagnostic["input_desktop"], "Default");
    assert_eq!(diagnostic["foreground_present"], false);
    assert!(
        diagnostic["message"]
            .as_str()
            .expect("diagnostic message")
            .contains("no foreground window")
    );
}

#[rstest]
fn unreadable_input_surface_blocks_input_without_stopping_observation() {
    let diagnostic = windows_diagnostic_base(
        Ok(0),
        Ok(Some("Default")),
        Err("GetCursorPos failed: Access is denied. (os error 5)"),
        false,
    );

    assert_eq!(diagnostic["success"], true);
    assert_eq!(diagnostic["code"], "interactive_desktop_ready");
    assert_eq!(diagnostic["observation_ready"], true);
    assert_eq!(diagnostic["input_ready"], false);
    assert_eq!(
        diagnostic["input_code"],
        "interactive_input_surface_unavailable"
    );
    assert_eq!(diagnostic["input_desktop"], "Default");
    assert_eq!(diagnostic["input_surface_ready"], false);
    assert_eq!(
        diagnostic["input_surface_error"],
        "GetCursorPos failed: Access is denied. (os error 5)"
    );
    assert_eq!(diagnostic["foreground_present"], false);
}

#[rstest]
fn active_secure_input_desktop_fails_closed() {
    let diagnostic = windows_diagnostic_base(Ok(0), Ok(Some("Winlogon")), Ok(()), false);

    assert_eq!(diagnostic["success"], false);
    assert_eq!(diagnostic["code"], "interactive_desktop_unavailable");
    assert_eq!(diagnostic["observation_ready"], false);
    assert_eq!(diagnostic["input_ready"], false);
    assert_eq!(diagnostic["input_desktop"], "Winlogon");
    assert_eq!(diagnostic["foreground_present"], false);
}

#[rstest]
fn input_desktop_probe_error_fails_closed() {
    let diagnostic =
        windows_diagnostic_base(Ok(0), Err("OpenInputDesktop: access denied"), Ok(()), false);

    assert_eq!(diagnostic["success"], false);
    assert_eq!(diagnostic["observation_ready"], true);
    assert_eq!(diagnostic["input_ready"], false);
    assert_eq!(diagnostic["code"], "interactive_desktop_unknown");
    assert_eq!(diagnostic["input_desktop"], serde_json::Value::Null);
    assert_eq!(
        diagnostic["input_desktop_error"],
        "OpenInputDesktop: access denied"
    );
    assert_eq!(diagnostic["foreground_present"], false);
}

#[rstest]
fn unreadable_input_desktop_only_allows_an_exact_window_observation_attempt() {
    let diagnostic =
        windows_diagnostic_base(Ok(0), Err("OpenInputDesktop: access denied"), Ok(()), false);

    assert!(require_exact_window_observation_from(&diagnostic).is_ok());
    let desktop_error = require_desktop_observation_from(&diagnostic)
        .expect_err("desktop observation must retain the readable Default-desktop gate");
    assert_eq!(
        desktop_error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
}

#[rstest]
fn missing_input_desktop_identity_fails_closed() {
    let diagnostic = windows_diagnostic_base(Ok(0), Ok(None), Ok(()), false);

    assert_eq!(diagnostic["success"], false);
    assert_eq!(diagnostic["code"], "interactive_desktop_unknown");
    assert_eq!(diagnostic["input_desktop"], serde_json::Value::Null);
    assert_eq!(diagnostic["foreground_present"], false);
}

#[rstest]
#[case(Ok(0), true, true, "interactive_desktop_ready", "active")]
#[case(Ok(0), false, true, "interactive_desktop_ready", "active")]
#[case(Ok(4), true, false, "interactive_session_not_active", "disconnected")]
#[case(Ok(1), true, false, "interactive_session_not_active", "not_active")]
fn windows_session_state_fences_raw_input(
    #[case] state: Result<i32, String>,
    #[case] foreground: bool,
    #[case] success: bool,
    #[case] code: &str,
    #[case] state_name: &str,
) {
    let diagnostic = windows_diagnostic_base(state, Ok(Some("Default")), Ok(()), foreground);
    assert_eq!(diagnostic["success"], success);
    assert_eq!(diagnostic["code"], code);
    assert_eq!(diagnostic["observation_ready"], success);
    assert_eq!(diagnostic["input_ready"], success);
    assert_eq!(diagnostic["session_state"], state_name);
    assert_eq!(diagnostic["input_desktop"], "Default");
    assert_eq!(diagnostic["foreground_present"], foreground);
}

#[rstest]
fn windows_session_query_failure_is_not_ready() {
    let diagnostic = windows_diagnostic_base(
        Err("access denied".into()),
        Ok(Some("Default")),
        Ok(()),
        true,
    );
    assert_eq!(diagnostic["success"], false);
    assert_eq!(diagnostic["code"], "interactive_session_unknown");
    assert_eq!(diagnostic["input_desktop"], "Default");
    assert_eq!(diagnostic["foreground_present"], true);
    assert!(
        diagnostic["message"]
            .as_str()
            .unwrap()
            .contains("access denied")
    );
}

#[rstest]
fn scope_requires_exact_identity_and_action_rejects_unbounded_text() {
    assert!(ComputerUseTargetScope::default().validate().is_err());
    assert!(
        ComputerUseTargetScope {
            window_title: Some(String::new()),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    let action = ComputerUseAction {
        action: "type".into(),
        text: Some("x".repeat(MAX_TEXT_UTF16_UNITS + 1)),
        ..Default::default()
    };
    assert_eq!(
        validate_action(&action).unwrap_err().code,
        ComputerUseErrorCode::InvalidAction
    );
}

#[rstest]
fn bootstrap_activation_requires_exact_pid_and_window_handle() {
    let request = ComputerUseSessionStartRequest {
        activate_before: true,
        indicator_motion: IndicatorMotionPolicy::Auto,
    };
    for scope in [
        ComputerUseTargetScope {
            process_id: Some(42),
            ..Default::default()
        },
        ComputerUseTargetScope {
            window_handle: Some(31337),
            ..Default::default()
        },
        ComputerUseTargetScope {
            window_title: Some("Synthetic Test App".into()),
            ..Default::default()
        },
    ] {
        let error = request.validate_for_scope(&scope).unwrap_err();
        assert_eq!(error.code, ComputerUseErrorCode::InvalidTarget);
        assert!(error.message.contains("exact process_id and window_handle"));
    }

    assert!(
        request
            .validate_for_scope(&ComputerUseTargetScope {
                process_id: Some(42),
                window_handle: Some(31337),
                window_title: None,
            })
            .is_ok()
    );
    assert!(
        ComputerUseSessionStartRequest::default()
            .validate_for_scope(&ComputerUseTargetScope {
                window_title: Some("Synthetic Test App".into()),
                ..Default::default()
            })
            .is_ok()
    );
}

#[rstest]
fn session_start_motion_policy_is_explicit_non_persistent_and_defaults_accessibly() {
    let default_request: ComputerUseSessionStartRequest =
        serde_json::from_value(json!({})).expect("default request");
    let animated_request: ComputerUseSessionStartRequest =
        serde_json::from_value(json!({"indicator_motion": "animate"})).expect("animated request");
    let next_request: ComputerUseSessionStartRequest =
        serde_json::from_value(json!({})).expect("next default request");

    assert_eq!(
        default_request.indicator_motion,
        IndicatorMotionPolicy::Auto
    );
    assert_eq!(
        animated_request.indicator_motion,
        IndicatorMotionPolicy::Animate
    );
    assert_eq!(next_request.indicator_motion, IndicatorMotionPolicy::Auto);
    assert_eq!(
        serde_json::to_value(animated_request).expect("request serializes")["indicator_motion"],
        "animate"
    );
    assert!(
        serde_json::from_value::<ComputerUseSessionStartRequest>(
            json!({"indicator_motion": "sometimes"})
        )
        .is_err()
    );
}

#[rstest]
fn window_frame_accepts_fractional_multi_monitor_coordinates() {
    assert!(
        ComputerUseWindowFrameRequest {
            x: -1919.5,
            y: 42.25,
            width: 1280.5,
            height: 720.25,
        }
        .validate()
        .is_ok()
    );
}

#[rstest]
fn native_menu_paths_match_the_upstream_contract_bounds() {
    assert!(
        ComputerUseMenuRequest {
            path: ["Window", "Arrange", "Left"].map(str::to_owned).to_vec(),
        }
        .validate()
        .is_ok()
    );
    for path in [
        Vec::new(),
        vec![" ".into()],
        vec!["x".repeat(201)],
        vec!["item".into(); 17],
    ] {
        assert_eq!(
            ComputerUseMenuRequest { path }.validate().unwrap_err().code,
            ComputerUseErrorCode::InvalidAction
        );
    }
}

#[rstest]
#[case(f64::NAN, 0.0, 100.0, 100.0)]
#[case(0.0, f64::INFINITY, 100.0, 100.0)]
#[case(0.0, 0.0, 0.0, 100.0)]
#[case(0.0, 0.0, 100.0, -1.0)]
fn invalid_window_frames_fail_before_cua(
    #[case] x: f64,
    #[case] y: f64,
    #[case] width: f64,
    #[case] height: f64,
) {
    let error = ComputerUseWindowFrameRequest {
        x,
        y,
        width,
        height,
    }
    .validate()
    .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::InvalidAction);
}

#[rstest]
fn native_target_policy_denies_terminal_processes() {
    let mut target = test_window_target();
    target.app_name = "powershell.exe".into();
    assert_eq!(
        validate_target_policy(&target).unwrap_err().code,
        ComputerUseErrorCode::InvalidTarget
    );
}

#[rstest]
fn generic_application_labels_resolve_from_the_exact_window() {
    let target = WindowTarget {
        pid: 42,
        window_id: 84,
        title: "Synthetic Test App".into(),
        app_name: "synthetic-test-app.exe".into(),
        bounds: [0, 0, 1280, 720],
        is_on_screen: true,
        is_minimized: false,
        z_index: Some(0),
        is_foreground: true,
    };

    assert_eq!(
        crate::runtime::resolved_application_name("Application", &target),
        "Synthetic Test App",
    );
    assert_eq!(
        crate::runtime::resolved_application_name("Maya 2024", &target),
        "Maya 2024",
        "an explicit profile/application identity must remain authoritative",
    );
}

#[rstest]
fn window_target_accepts_fractional_platform_bounds() {
    let target = WindowTarget::from_value(&json!({
        "pid": 42,
        "window_id": 7,
        "title": "DCC",
        "app_name": "dcc",
        "bounds": {"x": 120.5, "y": -0.5, "width": 940.4, "height": 779.6}
    }))
    .expect("macOS floating-point bounds should parse");
    assert_eq!(target.bounds, [121, -1, 940, 780]);
}

#[rstest]
fn verify_state_bounds_are_rejected_before_backend_call() {
    assert!(validate_verify_state_request(&json!([]), None, None).is_err());
    assert!(
        validate_verify_state_request(&json!([{"window": {"exists": true}}]), Some(10_001), None,)
            .is_err()
    );
    assert!(
        validate_verify_state_request(&json!([{"window": {"exists": true}}]), None, Some(6),)
            .is_err()
    );
    assert!(
            validate_verify_state_request(
                &json!([{"window": {"exists": true}}]),
                Some(1_000),
                Some(2),
            )
            .is_ok()
        );
}

#[rstest]
fn zoom_is_fenced_to_the_latest_observation_and_bounds() {
    let observation = ComputerUseObservation {
        observation_id: "obs-1".into(),
        window_handle: 7,
        process_id: 42,
        window_title: "DCC".into(),
        width: 1280,
        height: 720,
        source_rect: [0, 0, 1280, 720],
        capture_backend: "test".into(),
        capture_provenance: json!({}),
        session_id: "session".into(),
    };
    let valid = ComputerUseZoomRequest {
        observation_id: "obs-1".into(),
        x1: 10.0,
        y1: 20.0,
        x2: 400.0,
        y2: 200.0,
    };
    assert!(validate_zoom_request(&valid, &observation).is_ok());
    assert_eq!(
        validate_zoom_request(
            &ComputerUseZoomRequest {
                observation_id: "old".into(),
                ..valid.clone()
            },
            &observation,
        )
        .unwrap_err()
        .code,
        ComputerUseErrorCode::StaleObservation
    );
    assert!(
        validate_zoom_request(
            &ComputerUseZoomRequest {
                x2: 511.0,
                ..valid.clone()
            },
            &observation,
        )
        .is_err()
    );
    assert!(
        validate_zoom_request(
            &ComputerUseZoomRequest {
                x2: 1_281.0,
                ..valid
            },
            &observation,
        )
        .is_err()
    );
}

#[rstest]
#[tokio::test]
async fn denied_desktop_fallback_never_reaches_the_capture_backend() {
    let capture_called = Cell::new(false);
    let denied = Err(ComputerUseError::new(
        ComputerUseErrorCode::InteractiveDesktopUnavailable,
        "Windows input desktop could not be read",
    ));

    let error = gated_desktop_observation(denied, || {
        capture_called.set(true);
        async { Ok::<_, ComputerUseError>(()) }
    })
    .await
    .expect_err("desktop fallback must stop at its desktop-observation gate");

    assert_eq!(
        error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert!(!capture_called.get());
}

#[rstest]
#[tokio::test]
async fn exact_observation_publication_rechecks_readiness_after_the_capture_await() {
    let gate_calls = Cell::new(0_u8);
    let capture_calls = Cell::new(0_u8);

    let error = gated_exact_window_observation(
        || {
            let call = gate_calls.get() + 1;
            gate_calls.set(call);
            if call == 1 {
                Ok(())
            } else {
                Err(ComputerUseError::new(
                    ComputerUseErrorCode::InteractiveDesktopUnavailable,
                    "workstation locked while capture was in flight",
                ))
            }
        },
        || async {
            capture_calls.set(capture_calls.get() + 1);
            Ok::<_, ComputerUseError>("captured evidence")
        },
    )
    .await
    .expect_err("post-lock evidence must not be published");

    assert_eq!(
        error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert_eq!(gate_calls.get(), 2);
    assert_eq!(capture_calls.get(), 1);
}

#[rstest]
#[tokio::test]
async fn exact_observation_publication_rechecks_after_the_final_async_stage() {
    let gate_calls = Cell::new(0_u8);
    let publish_calls = Cell::new(0_u8);

    let error = gated_exact_window_publication(
        || {
            let call = gate_calls.get() + 1;
            gate_calls.set(call);
            if call < 3 {
                Ok(())
            } else {
                Err(ComputerUseError::new(
                    ComputerUseErrorCode::InteractiveDesktopUnavailable,
                    "the desktop locked during final image processing",
                ))
            }
        },
        || async { Ok::<_, ComputerUseError>("captured") },
        |captured| async move { Ok::<_, ComputerUseError>(format!("{captured}-encoded")) },
        |_| {
            publish_calls.set(publish_calls.get() + 1);
        },
    )
    .await
    .expect_err("the last readiness fence must run immediately before publication");

    assert_eq!(
        error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert_eq!(gate_calls.get(), 3);
    assert_eq!(publish_calls.get(), 0);
}

#[rstest]
#[tokio::test]
async fn exact_observation_publication_rejects_a_target_minimized_during_finalization() {
    let gate_calls = Cell::new(0_u8);
    let publish_calls = Cell::new(0_u8);

    let error = gated_exact_window_publication(
        || {
            let call = gate_calls.get() + 1;
            gate_calls.set(call);
            if call < 3 {
                Ok(())
            } else {
                Err(ComputerUseError::new(
                    ComputerUseErrorCode::TargetMinimized,
                    "target_minimized: the exact target changed during finalization",
                ))
            }
        },
        || async { Ok::<_, ComputerUseError>("captured") },
        |captured| async move { Ok::<_, ComputerUseError>(format!("{captured}-finalized")) },
        |_| {
            publish_calls.set(publish_calls.get() + 1);
        },
    )
    .await
    .expect_err("a newly minimized target must not publish the completed frame");

    assert_eq!(error.code, ComputerUseErrorCode::TargetMinimized);
    assert_eq!(gate_calls.get(), 3);
    assert_eq!(publish_calls.get(), 0);
}

#[rstest]
fn desktop_fallback_crops_an_8_bit_rgba_png() {
    let mut source = Vec::new();
    let mut encoder = png::Encoder::new(&mut source, 4, 3);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(&(0..48).map(|value| value as u8).collect::<Vec<_>>())
        .unwrap();
    writer.finish().unwrap();

    let cropped = crop_png_to_bounds(&source, [1, 1, 2, 2]).unwrap();
    assert_eq!(png_dimensions(&cropped), Some((2, 2)));
    let mut reader = png::Decoder::new(Cursor::new(cropped)).read_info().unwrap();
    let mut bytes = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut bytes).unwrap();
    assert_eq!(
        &bytes[..info.buffer_size()],
        &(20..28).chain(36..44).map(|v| v as u8).collect::<Vec<_>>()
    );
}

#[rstest]
fn exact_window_identity_failures_never_degrade_to_desktop_pixels() {
    assert!(
        !crate::runtime::exact_capture_failure_allows_desktop_fallback(
            ComputerUseErrorCode::InvalidTarget
        )
    );
    assert!(
        crate::runtime::exact_capture_failure_allows_desktop_fallback(
            ComputerUseErrorCode::CaptureFailed
        )
    );
}

#[rstest]
fn desktop_fallback_maps_per_monitor_dpi_bounds_to_capture_pixels() {
    assert_eq!(
        scale_bounds_for_dpi([1, 1, 3118, 1982], 192).unwrap(),
        [0, 0, 1560, 992]
    );
    assert_eq!(
        scale_bounds_for_dpi([10, 20, 300, 400], 96).unwrap(),
        [10, 20, 300, 400]
    );
}

#[rstest]
fn foreground_fallback_uses_the_highest_known_z_index() {
    let mut back = test_window_target();
    back.window_id = 1;
    back.z_index = Some(3);
    back.is_foreground = false;
    let mut front = test_window_target();
    front.window_id = 2;
    front.z_index = Some(7);
    front.is_foreground = false;
    let mut windows = vec![back, front];
    mark_foreground_by_z_index(&mut windows);
    assert!(!windows[0].is_foreground);
    assert!(windows[1].is_foreground);
}

#[rstest]
fn native_tool_boundary_rejects_reserved_and_dedicated_routes() {
    assert!(validate_native_tool_request("debug_window_info", &json!({})).is_ok());
    assert!(validate_native_tool_request("bad-name", &json!({})).is_err());
    assert!(validate_native_tool_request("debug_window_info", &json!({"_tool":"x"})).is_err());
    for name in cua_driver_contract::ACTION_RESULT_TOOLS {
        assert!(
            !native_tool_allowed_in_window_session(name),
            "canonical action tool {name} bypassed its dedicated route"
        );
    }
    assert!(!native_tool_allowed_in_window_session("list_windows"));
    assert!(!native_tool_allowed_in_window_session("get_browser_state"));
    assert!(!native_tool_allowed_in_window_session("page"));
    assert!(!native_tool_allowed_in_window_session("get_session_state"));
    assert!(!native_tool_allowed_in_window_session("browser_navigate"));
    assert!(native_tool_allowed_in_window_session("debug_window_info"));
    assert!(native_tool_allowed_globally("health_report"));
    assert!(native_tool_allowed_globally("get_accessibility_tree"));
    assert!(!native_tool_allowed_globally("launch_app"));
    assert!(validate_escalation_request("other", Some("reason")).is_ok());
    assert!(validate_escalation_request("uia_timeout", None).is_ok());
    let escalation_error = validate_escalation_request("unknown", None).unwrap_err();
    assert!(escalation_error.message.contains("allowed values"));
    for reason in COMPUTER_USE_ESCALATION_REASONS {
        assert!(escalation_error.message.contains(reason.value));
    }
    assert!(cursor_tool_allowed("get_agent_cursor_state"));
    assert!(cursor_tool_allowed("move_cursor"));
}

#[rstest]
fn tool_schema_lookup_uses_exact_inventory_names() {
    let inventory = json!({
        "tools": [{
            "name": "debug_window_info",
            "inputSchema": {"type": "object"}
        }]
    });
    assert_eq!(
        tool_schema_from_inventory(&inventory, "debug_window_info").unwrap()["type"],
        "object"
    );
    assert!(tool_schema_from_inventory(&inventory, "debug_window_info_extra").is_err());
}

#[rstest]
fn window_cursor_move_is_bounded_to_window_scope() {
    let mut valid = serde_json::json!({"x": 10, "y": 20});
    let target = WindowTarget {
        pid: 7,
        window_id: 9,
        title: "target".into(),
        app_name: "app".into(),
        bounds: [100, 200, 800, 600],
        is_on_screen: true,
        is_minimized: false,
        z_index: Some(0),
        is_foreground: true,
    };
    map_window_cursor_move(valid.as_object_mut().unwrap(), &target).unwrap();
    assert_eq!(valid["x"], 110.0);
    assert_eq!(valid["y"], 220.0);
    assert_eq!(valid["scope"], "window");

    let desktop = serde_json::json!({"x": 10, "y": 20, "scope": "desktop"});
    assert!(validate_window_cursor_move(desktop.as_object().unwrap()).is_err());

    let negative = serde_json::json!({"x": -1, "y": 20});
    assert!(validate_window_cursor_move(negative.as_object().unwrap()).is_err());

    let mut outside = serde_json::json!({"x": 800, "y": 20});
    assert!(map_window_cursor_move(outside.as_object_mut().unwrap(), &target).is_err());
}

#[rstest]
fn png_dimensions_reads_the_png_header() {
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    data.extend_from_slice(&[0; 8]);
    data.extend_from_slice(&1280_u32.to_be_bytes());
    data.extend_from_slice(&720_u32.to_be_bytes());
    assert_eq!(png_dimensions(&data), Some((1280, 720)));
}

#[rstest]
fn semantic_element_actions_replace_pixel_coordinates() {
    let action = ComputerUseAction {
        action: "click".into(),
        element_index: Some(7),
        x: Some(12.0),
        y: Some(13.0),
        ..Default::default()
    };
    let args = action_arguments(
        &action,
        "session",
        &WindowTarget {
            pid: 42,
            window_id: 7,
            title: "DCC".into(),
            app_name: "dcc".into(),
            bounds: [0, 0, 100, 100],
            is_on_screen: true,
            is_minimized: false,
            z_index: Some(1),
            is_foreground: true,
        },
    );
    assert_eq!(args["element_index"], 7);
    assert!(args.get("x").is_none());
    assert!(args.get("y").is_none());
}

#[rstest]
fn semantic_tokens_and_background_delivery_reach_cua() {
    let action = ComputerUseAction {
        action: "click".into(),
        element_index: Some(7),
        element_token: Some("element-token".into()),
        delivery_mode: Some("background".into()),
        x: Some(12.0),
        y: Some(13.0),
        ..Default::default()
    };
    let args = action_arguments(
        &action,
        "session",
        &WindowTarget {
            pid: 42,
            window_id: 7,
            title: "DCC".into(),
            app_name: "dcc".into(),
            bounds: [0, 0, 100, 100],
            is_on_screen: true,
            is_minimized: false,
            z_index: Some(1),
            is_foreground: true,
        },
    );
    assert_eq!(args["element_token"], "element-token");
    assert!(args.get("element_index").is_none());
    assert_eq!(args["delivery_mode"], "background");
    assert!(args.get("x").is_none());
    assert!(args.get("y").is_none());
}

#[rstest]
fn windows_uia_tokens_route_only_supported_semantic_actions() {
    let observation = ComputerUseObservation {
        observation_id: "observation".into(),
        window_handle: 7,
        process_id: 42,
        window_title: "DCC".into(),
        width: 100,
        height: 100,
        source_rect: [0, 0, 100, 100],
        capture_backend: "native".into(),
        capture_provenance: json!({"accessibility_backend":"windows_uia"}),
        session_id: "session".into(),
    };
    assert!(is_windows_uia_semantic_action(
        &ComputerUseAction {
            action: "click".into(),
            element_token: Some("dcc-wuia:snapshot:2".into()),
            ..Default::default()
        },
        &observation,
    ));
    assert!(is_windows_uia_semantic_action(
        &ComputerUseAction {
            action: "set_value".into(),
            element_index: Some(2),
            ..Default::default()
        },
        &observation,
    ));
    assert!(!is_windows_uia_semantic_action(
        &ComputerUseAction {
            action: "keypress".into(),
            element_token: Some("dcc-wuia:snapshot:2".into()),
            ..Default::default()
        },
        &observation,
    ));
}

#[rstest]
fn windows_uia_semantic_actions_do_not_require_the_physical_input_desktop() {
    let observation = ComputerUseObservation {
        observation_id: "observation".into(),
        window_handle: 7,
        process_id: 42,
        window_title: "DCC".into(),
        width: 100,
        height: 100,
        source_rect: [0, 0, 100, 100],
        capture_backend: "windows_uia".into(),
        capture_provenance: json!({"accessibility_backend":"windows_uia"}),
        session_id: "session".into(),
    };

    assert!(!action_requires_physical_input_desktop(
        &ComputerUseAction {
            action: "click".into(),
            element_token: Some("dcc-wuia:snapshot:2".into()),
            ..Default::default()
        },
        &observation,
    ));
    assert!(action_requires_physical_input_desktop(
        &ComputerUseAction {
            action: "click".into(),
            x: Some(10.0),
            y: Some(20.0),
            ..Default::default()
        },
        &observation,
    ));
}

#[rstest]
fn semantic_only_observations_reject_unscoped_pixel_actions() {
    let observation = ComputerUseObservation {
        observation_id: "observation".into(),
        window_handle: 7,
        process_id: 42,
        window_title: "DCC".into(),
        width: 100,
        height: 100,
        source_rect: [0, 0, 100, 100],
        capture_backend: "windows_uia".into(),
        capture_provenance: json!({"pixels_captured":false}),
        session_id: "session".into(),
    };
    let error = validate_action_observation(
        &ComputerUseAction {
            action: "click".into(),
            x: Some(10.0),
            y: Some(20.0),
            ..Default::default()
        },
        &observation,
    )
    .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
    assert!(
        validate_action_observation(
            &ComputerUseAction {
                action: "click".into(),
                element_index: Some(2),
                ..Default::default()
            },
            &observation,
        )
        .is_ok()
    );
}

#[rstest]
fn upstream_window_inventory_miss_routes_to_the_exact_native_capture() {
    assert!(is_uia_snapshot_failure(&cua_driver_sdk::ToolResult {
        is_error: true,
        error_code: Some("missing_window".into()),
        raw_json: "{}".into(),
        text: "inventory miss".into(),
        structured_json: None,
        images: Vec::new(),
        degraded: false,
        action: None,
        verification: None,
    }));
}

#[rstest]
fn action_rejects_unknown_delivery_mode_and_unbounded_token() {
    assert_eq!(
        validate_action(&ComputerUseAction {
            action: "click".into(),
            delivery_mode: Some("guess".into()),
            ..Default::default()
        })
        .unwrap_err()
        .code,
        ComputerUseErrorCode::InvalidAction
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "click".into(),
            element_token: Some("x".repeat(MAX_ELEMENT_TOKEN_CHARS + 1)),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "drag".into(),
            button: Some("side".into()),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "keypress".into(),
            modifiers: vec!["x".repeat(MAX_MODIFIER_CHARS + 1)],
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "drag".into(),
            duration_ms: Some(10_001),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "click".into(),
            x: Some(10.0),
            y: Some(20.0),
            duration_ms: Some(100),
            ..Default::default()
        })
        .is_ok()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "keypress".into(),
            keys: vec!["W".into()],
            duration_ms: Some(1_000),
            delivery_mode: Some("foreground".into()),
            ..Default::default()
        })
        .is_ok()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "keypress".into(),
            keys: vec!["W".into(), "A".into()],
            duration_ms: Some(1_000),
            ..Default::default()
        })
        .is_ok()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "keypress".into(),
            keys: vec!["W".into(), "Enter".into()],
            duration_ms: Some(1_000),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "click".into(),
            element_index: Some(1),
            duration_ms: Some(100),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "drag".into(),
            steps: Some(201),
            ..Default::default()
        })
        .is_err()
    );
}

#[rstest]
fn unsupported_input_backend_is_a_structured_non_fallback_result() {
    let result = input_backend_rejection_result(
        "windows.unknown.v1",
        "unsupported input backend",
        &test_window_target(),
    );
    assert_eq!(result.value["success"], false);
    assert_eq!(result.value["route"], "input_backend_selection");
    assert_eq!(
        result.value["delivery"],
        json!({
            "mode": "foreground",
            "backend_id": "windows.unknown.v1",
            "api_accepted": false,
            "consumer_effect_confirmed": false,
            "completion_known": false,
            "verification_required": true,
            "retry_safe": false,
            "fallback_attempted": false,
            "rejection_reason": "unsupported input backend",
            "target_fence": {
                "process_id": 42,
                "window_handle": 7,
                "exact_window": true,
                "foreground_required": true,
                "foreground_verified": true
            }
        })
    );
    assert_eq!(result.value["effect"], "not_attempted");
    assert!(result.degraded);
}

#[rstest]
fn semantic_value_actions_require_and_encode_element_values() {
    let action = ComputerUseAction {
        action: "set_text".into(),
        element_index: Some(11),
        text: Some("Hero".into()),
        ..Default::default()
    };
    let args = action_arguments(
        &action,
        "session",
        &WindowTarget {
            pid: 42,
            window_id: 7,
            title: "DCC".into(),
            app_name: "dcc".into(),
            bounds: [0, 0, 100, 100],
            is_on_screen: true,
            is_minimized: false,
            z_index: Some(1),
            is_foreground: true,
        },
    );
    assert_eq!(args["_tool"], "set_value");
    assert_eq!(args["element_index"], 11);
    assert_eq!(args["value"], "Hero");
    assert_eq!(
        validate_action(&ComputerUseAction {
            action: "set_value".into(),
            text: Some("Hero".into()),
            ..Default::default()
        })
        .unwrap_err()
        .code,
        ComputerUseErrorCode::InvalidAction
    );
}

#[rstest]
fn coordinate_text_and_key_actions_forward_cua_focus_arguments() {
    let type_action = ComputerUseAction {
        action: "type".into(),
        x: Some(20.0),
        y: Some(30.0),
        text: Some("Fab".into()),
        ..Default::default()
    };
    assert!(validate_action(&type_action).is_ok());
    let type_args = action_arguments(&type_action, "session", &test_window_target());
    assert_eq!(type_args["_tool"], "type_text");
    assert_eq!(type_args["x"], 20.0);
    assert_eq!(type_args["y"], 30.0);

    let press_action = ComputerUseAction {
        action: "keypress".into(),
        x: Some(20.0),
        y: Some(30.0),
        keys: vec!["S".into()],
        modifiers: vec!["CTRL".into(), "SHIFT".into()],
        ..Default::default()
    };
    let press_args = action_arguments(&press_action, "session", &test_window_target());
    assert_eq!(press_args["x"], 20.0);
    assert_eq!(press_args["modifiers"], json!(["CTRL", "SHIFT"]));

    let drag_action = ComputerUseAction {
        action: "drag".into(),
        path: vec![
            ComputerUsePoint { x: 1.0, y: 2.0 },
            ComputerUsePoint { x: 3.0, y: 4.0 },
        ],
        button: Some("middle".into()),
        modifiers: vec!["ALT".into()],
        duration_ms: Some(750),
        steps: Some(32),
        ..Default::default()
    };
    let drag_args = action_arguments(&drag_action, "session", &test_window_target());
    assert_eq!(drag_args["button"], "middle");
    assert_eq!(drag_args["modifier"], json!(["ALT"]));
    assert_eq!(drag_args["duration_ms"], 750);
    assert_eq!(drag_args["steps"], 32);
}

#[rstest]
fn keyboard_shortcuts_merge_explicit_modifiers_for_every_input_route() {
    let action = ComputerUseAction {
        action: "keyboard_shortcut".into(),
        keys: vec!["T".into()],
        modifiers: vec!["CTRL".into()],
        ..Default::default()
    };

    for arguments in [
        action_arguments(&action, "session", &test_window_target()),
        desktop_action_arguments(&action, "session"),
    ] {
        assert_eq!(arguments["_tool"], "hotkey");
        assert_eq!(arguments["keys"], json!(["CTRL", "T"]));
    }
}

#[rstest]
#[case(-4, 0, "left", 4)]
#[case(4, 0, "right", 4)]
#[case(0, -5, "up", 5)]
#[case(0, 5, "down", 5)]
fn scroll_axes_reach_window_and_desktop_cua(
    #[case] scroll_x: i32,
    #[case] scroll_y: i32,
    #[case] direction: &str,
    #[case] amount: u32,
) {
    let action = ComputerUseAction {
        action: "scroll".into(),
        scroll_x: Some(scroll_x),
        scroll_y: Some(scroll_y),
        scroll_by: Some("page".into()),
        ..Default::default()
    };
    validate_action(&action).unwrap();
    for arguments in [
        action_arguments(&action, "session", &test_window_target()),
        desktop_action_arguments(&action, "session"),
    ] {
        assert_eq!(arguments["direction"], direction);
        assert_eq!(arguments["amount"], amount);
        assert_eq!(arguments["by"], "page");
    }
}

#[rstest]
fn scroll_rejects_diagonal_unbounded_and_unscoped_requests() {
    for action in [
        ComputerUseAction {
            action: "scroll".into(),
            scroll_x: Some(1),
            scroll_y: Some(1),
            ..Default::default()
        },
        ComputerUseAction {
            action: "scroll".into(),
            scroll_y: Some(51),
            ..Default::default()
        },
        ComputerUseAction {
            action: "scroll".into(),
            ..Default::default()
        },
        ComputerUseAction {
            action: "scroll".into(),
            element_index: Some(1),
            scroll_by: Some("pixel".into()),
            ..Default::default()
        },
    ] {
        assert!(validate_action(&action).is_err());
    }

    let element_scroll = ComputerUseAction {
        action: "scroll".into(),
        element_index: Some(1),
        ..Default::default()
    };
    validate_action(&element_scroll).unwrap();
    let arguments = action_arguments(&element_scroll, "session", &test_window_target());
    assert_eq!(arguments["direction"], "down");
    assert!(arguments.get("amount").is_none());
}

#[rstest]
fn semantic_type_uses_cua_canonical_type_text() {
    let action = ComputerUseAction {
        action: "type".into(),
        element_index: Some(8),
        text: Some("hello".into()),
        ..Default::default()
    };
    let arguments = action_arguments(&action, "session", &test_window_target());
    assert_eq!(arguments["_tool"], "type_text");
    assert_eq!(arguments["element_index"], 8);
    assert_eq!(arguments["text"], "hello");
}

fn test_window_target() -> WindowTarget {
    WindowTarget {
        pid: 42,
        window_id: 7,
        title: "DCC".into(),
        app_name: "dcc".into(),
        bounds: [0, 0, 100, 100],
        is_on_screen: true,
        is_minimized: false,
        z_index: Some(1),
        is_foreground: true,
    }
}

#[rstest]
fn minimized_exact_target_pauses_every_action_before_any_input_path() {
    let mut target = test_window_target();
    target.is_minimized = true;
    target.is_on_screen = false;
    target.is_foreground = false;

    let error = ensure_target_available_for_action(&target).unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::TargetMinimized);
    assert!(error.message.contains("target_minimized"));
    assert!(error.message.contains("automatic_input=false"));
    assert!(error.message.contains("restore_activate"));
}

#[rstest]
fn type_chars_forwards_delivery_mode_to_cua_character_input() {
    let action = ComputerUseAction {
        action: "type_chars".into(),
        element_index: Some(4),
        text: Some("Fab".into()),
        delay_ms: Some(20),
        delivery_mode: Some("foreground".into()),
        ..Default::default()
    };
    assert!(validate_action(&action).is_ok());
    let args = action_arguments(
        &action,
        "session",
        &WindowTarget {
            pid: 42,
            window_id: 7,
            title: "DCC".into(),
            app_name: "dcc".into(),
            bounds: [0, 0, 100, 100],
            is_on_screen: true,
            is_minimized: false,
            z_index: Some(1),
            is_foreground: true,
        },
    );
    assert_eq!(args["_tool"], "type_text");
    assert_eq!(args["text"], "Fab");
    assert_eq!(args["delay_ms"], 20);
    assert_eq!(args["element_index"], 4);
    assert_eq!(args["delivery_mode"], "foreground");
    assert!(args.get("type_chars_only").is_none());

    let focused = ComputerUseAction {
        action: "type_chars".into(),
        text: Some("UE".into()),
        type_chars_only: true,
        ..Default::default()
    };
    assert!(validate_action(&focused).is_ok());
    assert!(
        validate_action(&ComputerUseAction {
            action: "type_chars".into(),
            text: Some("UE".into()),
            ..Default::default()
        })
        .is_err()
    );
}

#[rstest]
fn desktop_actions_are_screen_scoped_and_observation_bound() {
    let action = ComputerUseAction {
        action: "click".into(),
        x: Some(100.0),
        y: Some(200.0),
        ..Default::default()
    };
    let args = desktop_action_arguments(&action, "desktop-session");
    assert_eq!(args["_tool"], "click");
    assert_eq!(args["scope"], "desktop");
    assert_eq!(args["session"], "desktop-session");
    assert_eq!(args["x"], 100.0);
    assert!(args.get("pid").is_none());

    let toggle = ComputerUseAction {
        action: "toggle".into(),
        x: Some(100.0),
        y: Some(200.0),
        ..Default::default()
    };
    let toggle_args = desktop_action_arguments(&toggle, "desktop-session");
    assert_eq!(toggle_args["_tool"], "click");
    assert_eq!(toggle_args["count"], 1);

    let type_chars = ComputerUseAction {
        action: "type_chars".into(),
        text: Some("Fab".into()),
        delay_ms: Some(20),
        type_chars_only: true,
        ..Default::default()
    };
    let type_args = desktop_action_arguments(&type_chars, "desktop-session");
    assert_eq!(type_args["_tool"], "type_text");
    assert_eq!(type_args["delay_ms"], 20);
    assert!(type_args.get("type_chars_only").is_none());
}

#[rstest]
fn window_visual_fallback_maps_capture_pixels_to_the_exact_target() {
    let observation = ComputerUseObservation {
        observation_id: "obs".into(),
        window_handle: 7,
        process_id: 42,
        window_title: "UE".into(),
        width: 1560,
        height: 992,
        source_rect: [1, 1, 3120, 1984],
        capture_backend: "cua-driver-sdk-desktop-crop".into(),
        capture_provenance: json!({"desktop_crop_bounds": [20, 30, 1560, 992]}),
        session_id: "session".into(),
    };
    let target = test_window_target();
    let validated = action_for_window_visual_fallback(
        &ComputerUseAction {
            action: "move".into(),
            x: Some(780.0),
            y: Some(120.0),
            ..Default::default()
        },
        &observation,
    )
    .unwrap();
    assert_eq!((validated.x, validated.y), (Some(1560.0), Some(240.0)));
    let args = action_arguments(&validated, "session", &target);
    assert_eq!(args["pid"], 42);
    assert_eq!(args["window_id"], 7);
    assert!(args.get("scope").is_none());
    assert!(
        action_for_window_visual_fallback(
            &ComputerUseAction {
                action: "click".into(),
                element_index: Some(1),
                ..Default::default()
            },
            &observation,
        )
        .is_err()
    );
}

#[rstest]
fn window_queries_require_a_selector_and_match_native_rows() {
    assert!(ComputerUseWindowQuery::default().validate().is_err());
    assert!(
        ComputerUseWindowQuery {
            app: Some(String::new()),
            ..Default::default()
        }
        .validate()
        .is_err()
    );

    let query = ComputerUseWindowQuery {
        app: Some("ue5editor.exe".into()),
        window_title: Some("PCG Fab".into()),
        ..Default::default()
    };
    assert!(query.matches_window(
        &json!({"app_name":"UE5Editor.exe", "title":"PCG Fab", "pid":42, "window_id":7})
    ));
    assert!(!query.matches_window(
        &json!({"app_name":"UE5Editor.exe", "title":"Other", "pid":42, "window_id":7})
    ));
    assert!(query.validate_selectors().is_ok());
}
