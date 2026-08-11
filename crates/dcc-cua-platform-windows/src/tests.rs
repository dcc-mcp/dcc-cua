use rstest::rstest;
use serde_json::json;

use super::{
    UiaAction, UiaTarget,
    snapshot::{TOKEN_PREFIX, normalize, resolve_index},
};

#[cfg(windows)]
use super::PersistentWgcCapture;
#[cfg(windows)]
use super::windows::completed_action_result;
#[cfg(windows)]
use super::windows::foreground_restore_required;

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
