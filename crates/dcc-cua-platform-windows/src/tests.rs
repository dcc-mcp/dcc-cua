use rstest::rstest;

#[cfg(windows)]
use crate::wgc::{
    WgcCompositorTiming, WgcCompositorTimingUnavailable, WgcFrameMeasurement,
    compositor_timing_from_100ns,
};
use serde_json::json;

use super::{
    UiaAction, UiaTarget, WindowsForegroundRelation, WindowsRawInputSnapshot,
    WindowsWindowIdentity,
    snapshot::{TOKEN_PREFIX, normalize, resolve_index},
};
#[cfg(windows)]
use crate::visible_capture::obscured_from_covered_samples;
#[cfg(windows)]
use crate::windows::retry_read_only_after_backend_failure;

#[cfg(windows)]
#[rstest]
#[case(0, false)]
#[case(1, false)]
#[case(2, true)]
#[case(5, true)]
fn visible_crop_requires_at_least_four_of_five_target_samples(
    #[case] covered_samples: usize,
    #[case] expected_obscured: bool,
) {
    assert_eq!(
        obscured_from_covered_samples(covered_samples),
        expected_obscured
    );
}

#[cfg(windows)]
use super::PersistentWgcCapture;
#[cfg(windows)]
use super::windows::{
    ActivationZOrder, activation_topmost_bounce, completed_action_result,
    exact_window_available_for_activation, exact_window_ownership_matches,
    foreground_restore_required, input_gated_window_mutation,
    run_restore_activate_mutation_sequence, window_frame_matches,
};

#[rstest]
fn snapshot_normalization_emits_flat_agent_friendly_elements() {
    let raw = json!({
        "ok": true,
        "focus_runtime_id": "focus-1",
        "root": {
            "runtime_id": "root",
            "fallback_path": "0",
            "name": "Maya",
            "automation_id": "",
            "class_name": "QtWindow",
            "control_type": "ControlType.Window",
            "is_password": false,
            "enabled": true,
            "offscreen": false,
            "focused": false,
            "bounds": {"x": 0, "y": 0, "width": 100, "height": 100},
            "value": null,
            "checked": null,
            "policy_tier": "task_grant",
            "children": [{
                "runtime_id": "menu",
                "fallback_path": "0.0",
                "name": "DCC MCP",
                "automation_id": "",
                "class_name": "QAction",
                "control_type": "ControlType.MenuItem",
                "is_password": false,
                "enabled": true,
                "offscreen": false,
                "focused": false,
                "bounds": {"x": 10, "y": 10, "width": 20, "height": 10},
                "value": null,
                "checked": null,
                "policy_tier": "task_grant",
                "children": []
            }]
        }
    });
    let (snapshot, state) = normalize(&raw).unwrap();
    let elements = snapshot["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 2);
    assert_eq!(elements[1]["role"], "MenuItem");
    assert_eq!(elements[1]["name"], "DCC MCP");
    assert!(
        elements[1]["element_token"]
            .as_str()
            .unwrap()
            .starts_with(TOKEN_PREFIX)
    );
    assert_eq!(resolve_index(&state, Some(1), None).unwrap(), 1);
    assert_eq!(
        resolve_index(&state, None, elements[1]["element_token"].as_str()).unwrap(),
        1
    );
    let fence = &state.fences[1];
    assert_eq!(fence.control_id, "uia:menu");
    assert_eq!(fence.identity, "menu");
    assert!(!fence.is_password);
    assert_eq!(fence.name, "dcc mcp");
    assert_eq!(fence.automation_id, "");
    assert_eq!(fence.class_name, "qaction");
    assert_eq!(fence.policy_tier, "task_grant");
}

#[rstest]
fn stale_or_foreign_tokens_are_rejected() {
    let raw = json!({"root": {
        "runtime_id": "root", "fallback_path": "0", "name": "Maya",
        "automation_id": "", "class_name": "QtWindow",
        "control_type": "ControlType.Window", "is_password": false,
        "policy_tier": "task_grant", "children": []
    }});
    let (_, state) = normalize(&raw).unwrap();
    assert!(resolve_index(&state, None, Some("dcc-wuia:old:0")).is_err());
    assert!(resolve_index(&state, Some(99), None).is_err());
}

#[rstest]
fn portable_contract_construction_does_not_require_windows() {
    let target = UiaTarget {
        process_id: 42,
        window_handle: 7,
    };
    let action = UiaAction {
        action: "click".into(),
        element_index: Some(1),
        ..Default::default()
    };
    assert_eq!(target.process_id, 42);
    assert_eq!(action.element_index, Some(1));
}

#[rstest]
fn windows_uia_click_has_a_scoped_legacy_default_action_fallback() {
    let backend = include_str!("../assets/windows_uia_backend.ps1");

    assert!(backend.contains("LegacyIAccessiblePattern"));
    assert!(backend.contains("DoDefaultAction()"));
}

#[rstest]
fn raw_input_debug_snapshot_has_a_stable_typed_wire_shape() {
    let snapshot = WindowsRawInputSnapshot {
        async_button_down: true,
        target: WindowsWindowIdentity {
            window_handle: 0x1234,
            process_id: 42,
        },
        foreground: Some(WindowsWindowIdentity {
            window_handle: 0x1234,
            process_id: 42,
        }),
        foreground_relation: WindowsForegroundRelation::ExactTarget,
        target_thread_capture: Some(WindowsWindowIdentity {
            window_handle: 0x5678,
            process_id: 42,
        }),
        capture_query_succeeded: true,
        capture_owned_by_target_process: true,
    };

    assert!(snapshot.allows_drag_path());
    assert_eq!(
        serde_json::to_value(snapshot).unwrap(),
        json!({
            "async_button_down": true,
            "target": {"window_handle": 0x1234, "process_id": 42},
            "foreground": {"window_handle": 0x1234, "process_id": 42},
            "foreground_relation": "exact_target",
            "target_thread_capture": {"window_handle": 0x5678, "process_id": 42},
            "capture_query_succeeded": true,
            "capture_owned_by_target_process": true,
        })
    );
}

#[rstest]
#[case(false, WindowsForegroundRelation::ExactTarget)]
#[case(true, WindowsForegroundRelation::SameProcess)]
#[case(true, WindowsForegroundRelation::ForeignProcess)]
#[case(true, WindowsForegroundRelation::NoForeground)]
fn raw_input_debug_snapshot_refuses_an_unobserved_down_or_non_exact_foreground(
    #[case] async_button_down: bool,
    #[case] foreground_relation: WindowsForegroundRelation,
) {
    let snapshot = WindowsRawInputSnapshot {
        async_button_down,
        target: WindowsWindowIdentity {
            window_handle: 0x1234,
            process_id: 42,
        },
        foreground: None,
        foreground_relation,
        target_thread_capture: None,
        capture_query_succeeded: false,
        capture_owned_by_target_process: false,
    };

    assert!(!snapshot.allows_drag_path());
}

#[cfg(windows)]
#[rstest]
#[case(10, 10, 42, 42, false)]
#[case(10, 20, 42, 42, true)]
#[case(10, 20, 7, 42, false)]
#[case(10, 0, 0, 42, true)]
fn background_action_only_restores_focus_stolen_by_the_controlled_process(
    #[case] expected: usize,
    #[case] current: usize,
    #[case] current_process_id: u32,
    #[case] controlled_process_id: u32,
    #[case] required: bool,
) {
    assert_eq!(
        foreground_restore_required(expected, current, current_process_id, controlled_process_id),
        required
    );
}

#[cfg(windows)]
#[rstest]
fn restore_activation_requires_the_exact_live_pid_hwnd_ownership_fence() {
    assert!(exact_window_ownership_matches(true, 42, 42));
    assert!(!exact_window_ownership_matches(false, 42, 42));
    assert!(!exact_window_ownership_matches(true, 42, 43));
}

#[cfg(windows)]
#[rstest]
fn activation_topmost_fallback_always_releases_topmost_state() {
    let mut steps = Vec::new();

    activation_topmost_bounce(|step| steps.push(step));

    assert_eq!(
        steps,
        vec![ActivationZOrder::TopMost, ActivationZOrder::NotTopMost]
    );
}

#[cfg(windows)]
#[rstest]
fn exact_window_frame_verification_is_coordinate_and_size_strict() {
    assert!(window_frame_matches(
        [100, 100, 698, 209],
        [100, 100, 698, 209]
    ));
    assert!(!window_frame_matches(
        [100, 100, 698, 209],
        [101, 100, 698, 209]
    ));
}

#[cfg(windows)]
#[rstest]
fn missing_wgc_frame_timestamp_is_typed_unavailable() {
    assert_eq!(
        compositor_timing_from_100ns(None, Some(20_000)),
        WgcCompositorTiming::Unavailable {
            reason: WgcCompositorTimingUnavailable::FrameTimestampUnavailable,
        }
    );
}

#[cfg(windows)]
#[rstest]
#[case(None, WgcCompositorTimingUnavailable::PerformanceCounterUnavailable)]
#[case(Some(9_999), WgcCompositorTimingUnavailable::TimestampAfterPublish)]
fn unavailable_publish_clock_never_fabricates_compositor_latency(
    #[case] publish_time_100ns: Option<i64>,
    #[case] expected_reason: WgcCompositorTimingUnavailable,
) {
    assert_eq!(
        compositor_timing_from_100ns(Some(10_000), publish_time_100ns),
        WgcCompositorTiming::Unavailable {
            reason: expected_reason,
        }
    );
}

#[cfg(windows)]
#[rstest]
fn wgc_measurement_keeps_source_wait_separate_from_readback() {
    let measurement = WgcFrameMeasurement::new(
        std::time::Duration::from_millis(900),
        std::time::Duration::from_millis(8),
        std::time::Duration::from_millis(5),
        std::time::Duration::from_millis(2),
        Some(10_000),
    )
    .at_publish_time_100ns(Some(20_000));

    assert_eq!(
        measurement.source_wait,
        std::time::Duration::from_millis(900)
    );
    assert_eq!(
        measurement.readback_total,
        std::time::Duration::from_millis(8)
    );
    assert_eq!(
        measurement.gpu_copy_map,
        std::time::Duration::from_millis(5)
    );
    assert_eq!(measurement.cpu_copy, std::time::Duration::from_millis(2));
    assert_eq!(
        measurement.compositor,
        WgcCompositorTiming::Available {
            system_relative_time_100ns: 10_000,
            compositor_to_publish: std::time::Duration::from_millis(1),
        }
    );
}

#[cfg(windows)]
#[rstest]
fn ordinary_activation_rejects_minimized_or_hidden_exact_targets() {
    assert!(exact_window_available_for_activation(
        true, true, false, 42, 42
    ));
    assert!(!exact_window_available_for_activation(
        true, true, true, 42, 42
    ));
    assert!(!exact_window_available_for_activation(
        true, false, false, 42, 42
    ));
    assert!(!exact_window_available_for_activation(
        true, true, false, 42, 43
    ));
}

#[cfg(windows)]
#[rstest]
fn locked_desktop_gate_prevents_each_platform_window_mutation() {
    let mutations = std::cell::Cell::new(0);

    let result = input_gated_window_mutation(
        || Err::<(), _>("desktop locked"),
        || {
            mutations.set(mutations.get() + 1);
            Ok(())
        },
    );

    assert_eq!(result, Err("desktop locked"));
    assert_eq!(mutations.get(), 0);
}

#[cfg(windows)]
#[rstest]
fn restore_activate_sequence_stops_at_each_failed_input_gate() {
    let restore_calls = std::cell::Cell::new(0);
    let activate_calls = std::cell::Cell::new(0);
    let first_gate = run_restore_activate_mutation_sequence(
        || Err::<(), _>("locked before restore"),
        || {
            restore_calls.set(restore_calls.get() + 1);
            Ok(())
        },
        || Ok(()),
        || {
            activate_calls.set(activate_calls.get() + 1);
            Ok(())
        },
    );
    assert_eq!(first_gate, Err("locked before restore"));
    assert_eq!((restore_calls.get(), activate_calls.get()), (0, 0));

    let second_gate = run_restore_activate_mutation_sequence(
        || Ok(()),
        || {
            restore_calls.set(restore_calls.get() + 1);
            Ok(())
        },
        || Err("locked before activate"),
        || {
            activate_calls.set(activate_calls.get() + 1);
            Ok(())
        },
    );
    assert_eq!(second_gate, Err("locked before activate"));
    assert_eq!((restore_calls.get(), activate_calls.get()), (1, 0));

    let success = run_restore_activate_mutation_sequence(
        || Ok::<(), &str>(()),
        || {
            restore_calls.set(restore_calls.get() + 1);
            Ok(())
        },
        || Ok(()),
        || {
            activate_calls.set(activate_calls.get() + 1);
            Ok(())
        },
    );
    assert_eq!(success, Ok(()));
    assert_eq!((restore_calls.get(), activate_calls.get()), (2, 1));
}

#[cfg(windows)]
#[rstest]
fn completed_background_action_reports_restore_failure_without_becoming_retryable() {
    let result = completed_action_result(
        &json!({"ok": true, "message": "clicked", "control": {"name": "OK"}}),
        Some(Err(super::UiaError::BackendUnavailable(
            "foreground changed".into(),
        ))),
    )
    .unwrap();

    assert_eq!(result["success"], true);
    assert_eq!(result["action_executed"], true);
    assert_eq!(result["foreground_restore"]["requested"], true);
    assert_eq!(result["foreground_restore"]["success"], false);
    assert_eq!(
        result["foreground_restore"]["message"],
        "foreground changed"
    );
}

#[cfg(windows)]
#[rstest]
fn backend_action_failure_remains_an_error_even_when_restore_succeeds() {
    let result = completed_action_result(
        &json!({"ok": false, "error": "not_found", "message": "control disappeared"}),
        Some(Ok(())),
    );

    assert!(matches!(result, Err(super::UiaError::StaleSnapshot(_))));
}

#[cfg(windows)]
#[rstest]
fn read_only_uia_retry_is_single_and_backend_failure_only() {
    let retries = std::cell::Cell::new(0);
    let recovered = retry_read_only_after_backend_failure(
        Err::<u32, _>(super::UiaError::BackendUnavailable("timed out".into())),
        || {
            retries.set(retries.get() + 1);
            Ok(42)
        },
    );
    assert_eq!(recovered.expect("read-only retry should recover"), 42);
    assert_eq!(retries.get(), 1);

    let invalid = retry_read_only_after_backend_failure(
        Err::<u32, _>(super::UiaError::InvalidTarget("closed".into())),
        || {
            retries.set(retries.get() + 1);
            Ok(7)
        },
    );
    assert!(matches!(invalid, Err(super::UiaError::InvalidTarget(_))));
    assert_eq!(retries.get(), 1);
}

#[cfg(windows)]
#[rstest]
#[ignore = "requires DCC_CUA_TEST_WINDOW_HANDLE for an existing rendered window"]
fn persistent_wgc_captures_consecutive_real_frames() {
    let window_handle = std::env::var("DCC_CUA_TEST_WINDOW_HANDLE")
        .expect("DCC_CUA_TEST_WINDOW_HANDLE")
        .parse::<u64>()
        .expect("numeric HWND");
    let mut capture = PersistentWgcCapture::new(window_handle).expect("persistent WGC session");
    let started = std::time::Instant::now();
    let (_, first_width, first_height) = capture
        .next_frame(std::time::Duration::from_secs(3))
        .expect("first WGC frame");
    let first_elapsed = started.elapsed();
    let second_started = std::time::Instant::now();
    let (_, second_width, second_height) = capture
        .next_frame(std::time::Duration::from_secs(3))
        .expect("second WGC frame");
    let second_elapsed = second_started.elapsed();
    assert_eq!((first_width, first_height), (second_width, second_height));
    assert!(first_width > 0 && first_height > 0);
    println!(
        "persistent WGC {first_width}x{first_height}: first={}ms second={}ms",
        first_elapsed.as_millis(),
        second_elapsed.as_millis()
    );
}
