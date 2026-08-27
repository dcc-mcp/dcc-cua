use rstest::rstest;

#[cfg(windows)]
use crate::wgc::{
    WgcCompositorTiming, WgcCompositorTimingUnavailable, WgcFrameMeasurement,
    compositor_timing_from_100ns,
};
use serde_json::json;

#[cfg(windows)]
use super::{
    ExactWindowCaptureRoute, capture_identity::route_for_same_executable_root_count,
    capture_visible_window, exact_window_capture_route, exact_window_pixel_evidence,
};
use super::{
    UiaAction, UiaTarget, WindowsForegroundRelation, WindowsRawInputSnapshot,
    WindowsWindowIdentity,
    snapshot::{TOKEN_PREFIX, normalize, resolve_index},
};
#[cfg(windows)]
use crate::visible_capture::root_z_order_proves_unobscured;
#[cfg(windows)]
use crate::windows::{
    REQUEST_TIMEOUT, STARTUP_TIMEOUT, UIA_WORKER_PROTOCOL_VERSION, UiaWorker,
    retry_read_only_after_backend_failure, validate_worker_protocol_message,
    validate_worker_readiness_message,
};
#[cfg(windows)]
use windows::Win32::{
    Foundation::{HWND as WindowsHwnd, RECT as WindowsRect},
    Graphics::Gdi::{BLACK_BRUSH, FillRect, GetDC, GetStockObject, HBRUSH, ReleaseDC, WHITE_BRUSH},
    UI::WindowsAndMessaging::GetClientRect,
};
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND},
    System::Threading::GetCurrentProcessId,
    UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowThreadProcessId, HWND_TOPMOST, SWP_SHOWWINDOW,
        SendMessageW, SetWindowPos, WM_PAINT, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    },
};

#[cfg(windows)]
use crate::input::{
    WindowsInputCount, combined_source_move_and_left_down_inputs, release_all_keys, virtual_key,
    wait_until_interrupted,
};
#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_MOVE_NOCOALESCE, MOUSEEVENTF_VIRTUALDESK,
};

#[cfg(windows)]
#[rstest]
fn combined_batch_keeps_move_and_left_down_ordered() {
    let inputs = combined_source_move_and_left_down_inputs((960, 540), (0, 0, 1_920, 1_080));
    let source_move = unsafe { inputs[0].Anonymous.mi };
    let left_down = unsafe { inputs[1].Anonymous.mi };
    let expected =
        platform_windows::virtualdesk::to_virtualdesk_absolute(960, 540, 0, 0, 1_920, 1_080);
    assert_eq!(inputs[0].r#type, INPUT_MOUSE);
    assert_eq!((source_move.dx, source_move.dy), expected);
    assert_eq!(
        source_move.dwFlags,
        MOUSEEVENTF_MOVE
            | MOUSEEVENTF_MOVE_NOCOALESCE
            | MOUSEEVENTF_ABSOLUTE
            | MOUSEEVENTF_VIRTUALDESK
    );
    assert_eq!(inputs[1].r#type, INPUT_MOUSE);
    assert_eq!(left_down.dwFlags, MOUSEEVENTF_LEFTDOWN);
    assert_eq!((left_down.dx, left_down.dy), (0, 0));
}

#[cfg(windows)]
#[rstest]
fn held_key_mapping_rejects_unknown_names() {
    assert!(matches!(
        virtual_key("not-a-key"),
        Err(super::WindowsHeldKeyError::InvalidKey(_))
    ));
    assert_eq!(virtual_key("a").unwrap(), u16::from(b'A'));
}

#[cfg(windows)]
#[rstest]
fn held_key_cleanup_attempts_every_release_after_a_failure() {
    let mut attempted = Vec::new();
    let failures = release_all_keys(&[1, 2, 3], |key| {
        attempted.push(key);
        if key == 2 {
            WindowsInputCount::incomplete(0, "injected release failure")
        } else {
            WindowsInputCount::accepted()
        }
    });

    assert_eq!(attempted, [3, 2, 1]);
    assert_eq!(failures.len(), 1);
    assert!(failures[0].contains("injected release failure"));
}

#[cfg(windows)]
#[rstest]
fn held_key_wait_honors_interrupts_and_zero_duration() {
    assert!(wait_until_interrupted(1_000, || true));
    assert!(!wait_until_interrupted(0, || false));
}

#[cfg(windows)]
fn policy_fixture_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    GUARD
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(windows)]
fn evaluate_policy_fixture(
    worker: &mut UiaWorker,
    fixture: serde_json::Value,
) -> Result<serde_json::Value, super::UiaError> {
    let mut payload = fixture.as_object().cloned().ok_or_else(|| {
        super::UiaError::InvalidAction("policy fixture must be a JSON object".into())
    })?;
    payload.insert("mode".into(), json!("policy_fixture"));
    worker.request(&serde_json::Value::Object(payload))
}

#[cfg(windows)]
#[rstest]
#[case([1, 1, 8, 8], "unsampled corner overlap")]
#[case([49, 49, 3, 3], "single center-sample overlap")]
fn visible_crop_rejects_every_foreign_root_intersection(
    #[case] covering_bounds: [i32; 4],
    #[case] _label: &str,
) {
    let target_bounds = [0, 0, 100, 100];
    let roots = [
        (91_u64, covering_bounds, true),
        (77_u64, target_bounds, true),
    ];

    assert!(
        !root_z_order_proves_unobscured(77, target_bounds, &roots),
        "any visible root above the target that intersects its rectangle must fail closed"
    );
}

#[cfg(windows)]
#[rstest]
fn visible_crop_requires_complete_target_z_order_proof() {
    let target_bounds = [0, 0, 100, 100];
    assert!(root_z_order_proves_unobscured(
        77,
        target_bounds,
        &[
            (91, [10, 10, 20, 20], false),
            (77, target_bounds, true),
            (92, [10, 10, 20, 20], true),
        ],
    ));
    assert!(!root_z_order_proves_unobscured(
        77,
        target_bounds,
        &[(91, [200, 200, 20, 20], true)],
    ));
}

#[cfg(windows)]
use super::PersistentWgcCapture;
#[cfg(windows)]
use super::windows::{
    ActivationRaiseMode, activation_raise_mode, completed_action_result,
    exact_window_available_for_activation, exact_window_ownership_matches,
    foreground_restore_required, input_gated_window_mutation,
    run_restore_activate_mutation_sequence, window_frame_matches,
};

#[cfg(windows)]
#[rstest]
#[case(1, ExactWindowCaptureRoute::Wgc)]
#[case(2, ExactWindowCaptureRoute::VerifiedVisible)]
#[case(3, ExactWindowCaptureRoute::VerifiedVisible)]
fn same_executable_multi_window_capture_requires_independent_pixel_proof(
    #[case] root_count: usize,
    #[case] expected: ExactWindowCaptureRoute,
) {
    assert_eq!(route_for_same_executable_root_count(root_count), expected);
}

#[cfg(windows)]
#[rstest]
fn same_executable_windows_capture_pixels_from_their_exact_hwnds() {
    const SS_BLACKRECT: u32 = 0x0000_0004;
    const SS_WHITERECT: u32 = 0x0000_0006;
    let black = ExactCaptureTestWindow::new("dcc-cua-black", SS_BLACKRECT, 40, 40);
    let white = ExactCaptureTestWindow::new("dcc-cua-white", SS_WHITERECT, 360, 40);
    std::thread::sleep(std::time::Duration::from_millis(100));
    let process_id = unsafe { GetCurrentProcessId() };

    assert_eq!(
        exact_window_capture_route(process_id, black.raw()).unwrap(),
        ExactWindowCaptureRoute::VerifiedVisible
    );
    assert_eq!(
        exact_window_capture_route(process_id, white.raw()).unwrap(),
        ExactWindowCaptureRoute::VerifiedVisible
    );

    assert_exact_capture_luma_or_fail_closed(process_id, &black, 0..=31);
    assert_exact_capture_luma_or_fail_closed(process_id, &white, 224..=255);
}

#[cfg(windows)]
#[rstest]
fn exact_capture_primitives_reject_a_pid_that_does_not_own_the_hwnd() {
    let window = ExactCaptureTestWindow::new("dcc-cua-owner-check", 0x0000_0006, 40, 40);
    let wrong_process_id = unsafe { GetCurrentProcessId() }.wrapping_add(1);

    let visible_error = capture_visible_window(wrong_process_id, window.raw())
        .expect_err("visible capture must reject an unrelated process grant");
    assert!(
        visible_error.to_string().contains("granted process"),
        "unexpected visible capture error: {visible_error}"
    );

    let wgc_error = match PersistentWgcCapture::new(wrong_process_id, window.raw()) {
        Ok(_) => panic!("WGC construction must reject an unrelated process grant"),
        Err(error) => error,
    };
    assert!(
        wgc_error.to_string().contains("granted process"),
        "unexpected WGC capture error: {wgc_error}"
    );
}

#[cfg(windows)]
#[rstest]
fn controlled_no_provider_window_exposes_exact_native_pixel_evidence() {
    let window = ExactCaptureTestWindow::new("dcc-cua-no-provider", 0x0000_0006, 40, 40);
    let process_id = unsafe { GetCurrentProcessId() };
    eprintln!(
        "provider=dcc-cua runtime={} pid={process_id} hwnd={}",
        env!("CARGO_PKG_VERSION"),
        window.raw()
    );

    let evidence = exact_window_pixel_evidence(process_id, window.raw())
        .expect("read exact native evidence without UIA");

    assert_eq!(evidence.process_id, process_id);
    assert_eq!(evidence.window_handle, window.raw());
    assert!(evidence.visible);
    assert!(!evidence.minimized);
    assert!(evidence.dpi > 0);
    assert!(evidence.bounds[2] > 4 && evidence.bounds[3] > 4);
}

#[cfg(windows)]
#[rstest]
fn controlled_window_move_and_resize_changes_publication_evidence() {
    let window = ExactCaptureTestWindow::new("dcc-cua-moving-providerless", 0x0000_0006, 40, 40);
    let process_id = unsafe { GetCurrentProcessId() };
    eprintln!(
        "provider=dcc-cua runtime={} pid={process_id} hwnd={}",
        env!("CARGO_PKG_VERSION"),
        window.raw()
    );
    let before = exact_window_pixel_evidence(process_id, window.raw()).unwrap();
    unsafe {
        SetWindowPos(window.0, HWND_TOPMOST, 420, 180, 360, 260, SWP_SHOWWINDOW);
    }
    let after = exact_window_pixel_evidence(process_id, window.raw()).unwrap();

    assert_ne!(before.bounds, after.bounds);
    assert_eq!(before.process_id, after.process_id);
    assert_eq!(before.window_handle, after.window_handle);
}

#[cfg(windows)]
struct ExactCaptureTestWindow(HWND, bool);

#[cfg(windows)]
impl ExactCaptureTestWindow {
    fn new(title: &str, static_style: u32, x: i32, y: i32) -> Self {
        let class = wide("STATIC");
        let title = wide(title);
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE | static_style,
                x,
                y,
                280,
                220,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                HINSTANCE::default(),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null(), "create exact-window capture fixture");
        unsafe {
            SetWindowPos(hwnd, HWND_TOPMOST, x, y, 280, 220, SWP_SHOWWINDOW);
            SendMessageW(hwnd, WM_PAINT, 0, 0);
        }
        let window = Self(hwnd, static_style == 0x0000_0004);
        window.paint_fixture_color();
        window
    }

    fn raw(&self) -> u64 {
        self.0 as usize as u64
    }

    fn raise(&self) {
        unsafe {
            SetWindowPos(
                self.0,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                    | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                    | SWP_SHOWWINDOW,
            );
            SendMessageW(self.0, WM_PAINT, 0, 0);
        }
        self.paint_fixture_color();
    }

    fn paint_fixture_color(&self) {
        let hwnd = WindowsHwnd(self.0.cast());
        let mut rect = WindowsRect::default();
        unsafe { GetClientRect(hwnd, &mut rect) }.expect("read exact-window fixture client bounds");
        let dc = unsafe { GetDC(hwnd) };
        assert!(
            !dc.0.is_null(),
            "acquire exact-window fixture device context"
        );
        let stock = unsafe { GetStockObject(if self.1 { BLACK_BRUSH } else { WHITE_BRUSH }) };
        let painted = unsafe { FillRect(dc, &rect, HBRUSH(stock.0)) };
        unsafe { ReleaseDC(hwnd, dc) };
        assert_ne!(
            painted, 0,
            "paint deterministic exact-window fixture pixels"
        );
    }
}

#[cfg(windows)]
impl Drop for ExactCaptureTestWindow {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0) };
    }
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn center_luma(bgra: &[u8], width: u32, height: u32) -> u8 {
    let index = ((height as usize / 2) * width as usize + width as usize / 2) * 4;
    let blue = u16::from(bgra[index]);
    let green = u16::from(bgra[index + 1]);
    let red = u16::from(bgra[index + 2]);
    ((red + green + blue) / 3) as u8
}

#[cfg(windows)]
fn assert_exact_capture_luma_or_fail_closed(
    process_id: u32,
    window: &ExactCaptureTestWindow,
    expected: std::ops::RangeInclusive<u8>,
) {
    window.raise();
    match capture_visible_window(process_id, window.raw()) {
        Ok(capture) => {
            let luma = center_luma(&capture.bgra, capture.width, capture.height);
            assert!(
                expected.contains(&luma),
                "exact HWND returned unexpected center luma {luma}"
            );
        }
        Err(error) => assert!(
            error
                .to_string()
                .contains("complete root-window z-order could not be proven"),
            "ambiguous pixels must fail closed: {error}"
        ),
    }
}

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
            "policy_category": "ordinary",
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
                "policy_category": "publishing",
                "children": []
            }]
        }
    });
    let (snapshot, state) = normalize(&raw).unwrap();
    assert_eq!(
        snapshot["element_bounds_coordinate_space"],
        "virtual_desktop"
    );
    let elements = snapshot["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 2);
    assert_eq!(elements[1]["role"], "MenuItem");
    assert_eq!(elements[1]["name"], "DCC MCP");
    assert_eq!(elements[1]["policy_category"], "publishing");
    assert!(
        elements[1]["element_token"]
            .as_str()
            .unwrap()
            .starts_with(TOKEN_PREFIX)
    );
    let index_only = resolve_index(&state, Some(1), None).unwrap_err();
    assert!(matches!(index_only, super::UiaError::InvalidAction(_)));
    assert_eq!(
        resolve_index(&state, None, elements[1]["element_token"].as_str()).unwrap(),
        1
    );
    let fence = &state.fences[1];
    assert_eq!(fence.control_id, "uia:menu");
    assert_eq!(fence.identity, "menu");
    assert!(!fence.is_password);
    assert_eq!(fence.name, "DCC MCP");
    assert_eq!(fence.automation_id, "");
    assert_eq!(fence.class_name, "QAction");
    assert_eq!(fence.policy_tier, "task_grant");
    assert_eq!(fence.policy_category, "publishing");
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
fn windows_uia_click_supports_expandable_controls_without_raw_input() {
    let backend = include_str!("../assets/windows_uia_backend.ps1");

    assert!(backend.contains("ExpandCollapsePattern"));
    assert!(backend.contains("Expand-Collapse-Click-Operation-From-State"));
    assert!(backend.contains("$pattern.Expand()"));
    assert!(backend.contains("$pattern.Collapse()"));
}

#[cfg(windows)]
#[rstest]
fn worker_protocol_rejects_missing_or_mismatched_versions() {
    assert!(
        validate_worker_protocol_message(&json!({
            "protocol_version": UIA_WORKER_PROTOCOL_VERSION
        }))
        .is_ok()
    );
    for message in [json!({}), json!({"protocol_version": 999})] {
        assert!(matches!(
            validate_worker_protocol_message(&message),
            Err(super::UiaError::ProtocolMismatch { .. })
        ));
    }
}

#[cfg(windows)]
#[rstest]
fn worker_readiness_is_bound_to_the_spawned_process() {
    let expected_pid = 42;
    assert!(
        validate_worker_readiness_message(
            &json!({
                "type": "ready",
                "protocol_version": UIA_WORKER_PROTOCOL_VERSION,
                "process_id": expected_pid,
            }),
            expected_pid,
        )
        .is_ok()
    );
    for message in [
        json!({
            "type": "ready",
            "protocol_version": UIA_WORKER_PROTOCOL_VERSION,
        }),
        json!({
            "type": "ready",
            "protocol_version": UIA_WORKER_PROTOCOL_VERSION,
            "process_id": expected_pid + 1,
        }),
    ] {
        assert!(validate_worker_readiness_message(&message, expected_pid).is_err());
    }
}

#[cfg(windows)]
#[rstest]
fn uia_worker_cold_start_has_a_separate_bounded_budget() {
    assert_eq!(REQUEST_TIMEOUT, std::time::Duration::from_secs(15));
    assert_eq!(STARTUP_TIMEOUT, std::time::Duration::from_secs(30));
}

#[cfg(windows)]
#[rstest]
fn powershell_worker_readiness_is_behaviorally_fixture_tested() {
    let _guard = policy_fixture_test_guard();
    for attempt in 0..5 {
        let mut worker = UiaWorker::start()
            .unwrap_or_else(|error| panic!("start isolated UIA worker attempt {attempt}: {error}"));
        let response = evaluate_policy_fixture(
            &mut worker,
            json!({
                "operation": "expand_collapse_click_operation",
                "state": "Collapsed",
            }),
        )
        .unwrap_or_else(|error| panic!("request UIA worker attempt {attempt}: {error}"));
        assert_eq!(response["result"], "expand");
    }
}

#[cfg(windows)]
#[rstest]
fn powershell_policy_tiers_are_behaviorally_fixture_tested() {
    let _guard = policy_fixture_test_guard();
    let mut worker = UiaWorker::start().expect("start policy fixture worker");
    for (facts, expected_tier, expected_category) in [
        (
            json!({"is_password": true, "name": "", "automation_id": "", "class_name": "", "secret_marker": false}),
            "action_confirmation",
            "credential",
        ),
        (
            json!({"is_password": false, "name": "API credential", "automation_id": "", "class_name": "Button", "secret_marker": true}),
            "action_confirmation",
            "credential",
        ),
        (
            json!({"is_password": false, "name": "Authentication code", "automation_id": "", "class_name": "Edit", "secret_marker": false}),
            "action_confirmation",
            "credential",
        ),
        (
            json!({"is_password": false, "name": "PowerShell", "automation_id": "", "class_name": "Button", "secret_marker": false}),
            "hard_deny",
            "hard_deny",
        ),
        (
            json!({"is_password": false, "name": "Save", "automation_id": "", "class_name": "Button", "secret_marker": false}),
            "action_confirmation",
            "destructive_write",
        ),
        (
            json!({"is_password": false, "name": "Pay now", "automation_id": "", "class_name": "Button", "secret_marker": false}),
            "action_confirmation",
            "payment",
        ),
        (
            json!({"is_password": false, "name": "Publish item", "automation_id": "", "class_name": "Button", "secret_marker": false}),
            "action_confirmation",
            "publishing",
        ),
        (
            json!({"is_password": false, "name": "Delete", "automation_id": "", "class_name": "Button", "secret_marker": false}),
            "action_confirmation",
            "destructive",
        ),
        (
            json!({"is_password": false, "name": "Log in", "automation_id": "", "class_name": "Button", "secret_marker": false}),
            "pre_approval",
            "account_access",
        ),
        (
            json!({"is_password": false, "name": "Open", "automation_id": "", "class_name": "Button", "secret_marker": false}),
            "task_grant",
            "ordinary",
        ),
    ] {
        let response = evaluate_policy_fixture(
            &mut worker,
            json!({
                "operation": "control_policy_tier",
                "facts": facts.clone(),
            }),
        )
        .expect("policy fixture response");
        assert_eq!(response["result"], expected_tier);
        let category = evaluate_policy_fixture(
            &mut worker,
            json!({
                "operation": "control_policy_category",
                "facts": facts,
            }),
        )
        .expect("policy category fixture response");
        assert_eq!(category["result"], expected_category);
    }
}

#[cfg(windows)]
#[rstest]
fn powershell_expand_collapse_click_decisions_are_behaviorally_fixture_tested() {
    let _guard = policy_fixture_test_guard();
    let mut worker = UiaWorker::start().expect("start expand-collapse fixture worker");
    for (state, expected) in [
        ("Collapsed", "expand"),
        ("Expanded", "collapse"),
        ("PartiallyExpanded", "collapse"),
        ("LeafNode", "unsupported"),
        ("Unknown", "unsupported"),
    ] {
        let response = evaluate_policy_fixture(
            &mut worker,
            json!({
                "operation": "expand_collapse_click_operation",
                "state": state,
            }),
        )
        .expect("expand-collapse fixture response");
        assert_eq!(response["result"], expected);
    }
}

#[cfg(windows)]
#[rstest]
fn powershell_fence_and_sensitive_target_policies_are_behaviorally_fixture_tested() {
    let _guard = policy_fixture_test_guard();
    let mut worker = UiaWorker::start().expect("start policy fixture worker");
    let expected = json!({
        "identity": "42.7",
        "is_password": false,
        "name": "Save",
        "automation_id": "SaveButton",
        "class_name": "Button",
        "policy_tier": "action_confirmation",
        "policy_category": "destructive_write",
    });
    let facts = json!({
        "identity": "42.7",
        "is_password": false,
        "name": "Save",
        "automation_id": "SaveButton",
        "class_name": "Button",
        "policy_tier": "action_confirmation",
        "policy_category": "destructive_write",
    });
    let matching = evaluate_policy_fixture(
        &mut worker,
        json!({
            "operation": "matches_expected_fence",
            "facts": facts,
            "expected": expected,
        }),
    )
    .expect("matching fence fixture");
    assert_eq!(matching["result"], true);

    let changed_case = evaluate_policy_fixture(
        &mut worker,
        json!({
            "operation": "matches_expected_fence",
            "facts": facts.clone(),
            "expected": {
                "identity": "42.7",
                "is_password": false,
                "name": "save",
                "automation_id": "SaveButton",
                "class_name": "Button",
                "policy_tier": "action_confirmation",
                "policy_category": "destructive_write",
            },
        }),
    )
    .expect("case-changed fence fixture");
    assert_eq!(changed_case["result"], false);

    let stale = evaluate_policy_fixture(&mut worker, json!({
        "operation": "matches_expected_fence",
        "facts": {"identity": "changed", "is_password": false, "name": "Save", "automation_id": "SaveButton", "class_name": "Button", "policy_tier": "action_confirmation", "policy_category": "destructive_write"},
        "expected": expected,
    }))
    .expect("stale fence fixture");
    assert_eq!(stale["result"], false);

    for facts in [
        json!({"identity_verified": false, "process_name": "", "class_name": ""}),
        json!({"identity_verified": true, "process_name": "pwsh", "class_name": ""}),
        json!({"identity_verified": true, "process_name": "explorer", "class_name": "#32770"}),
    ] {
        let denied = evaluate_policy_fixture(
            &mut worker,
            json!({
                "operation": "denied_target_reason",
                "facts": facts,
            }),
        )
        .expect("sensitive target fixture");
        assert!(
            denied["result"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
    }
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
fn activation_fallback_never_enters_topmost_state() {
    assert_eq!(activation_raise_mode(), ActivationRaiseMode::NonTopMost);
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
        Some(Err(super::UiaError::OperationFailed(
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

    let terminal = retry_read_only_after_backend_failure(
        Err::<u32, _>(super::UiaError::OperationFailed(
            "provider rejected request".into(),
        )),
        || {
            retries.set(retries.get() + 1);
            Ok(7)
        },
    );
    assert!(matches!(terminal, Err(super::UiaError::OperationFailed(_))));
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
    let mut process_id = 0_u32;
    unsafe { GetWindowThreadProcessId(window_handle as usize as HWND, &mut process_id) };
    assert_ne!(process_id, 0, "resolve DCC_CUA_TEST_WINDOW_HANDLE owner");
    let mut capture =
        PersistentWgcCapture::new(process_id, window_handle).expect("persistent WGC session");
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
