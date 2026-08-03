use std::future::pending;
use std::io::Cursor;
use std::time::Duration;

use rstest::rstest;
use serde_json::json;

use super::*;
use crate::contracts::{
    DEFAULT_SNAPSHOT_MAX_DEPTH, DEFAULT_SNAPSHOT_MAX_ELEMENTS, MAX_SNAPSHOT_DEPTH,
    MAX_SNAPSHOT_ELEMENTS, MAX_TEXT_UTF16_UNITS,
};
use crate::driver_factory::{UPSTREAM_CURSOR_RENDERER_ENABLED, driver_host_options};
use crate::policy::*;
use crate::runtime::application::{launch_arguments, validate_launch_request};
use crate::runtime::{await_input_call, diagnostic_tool_check, tool_schema_from_inventory};
use crate::window_target::{WindowTarget, validate_target_policy};

#[rstest]
#[tokio::test]
async fn input_calls_have_a_hard_timeout() {
    let error = await_input_call(
        pending::<()>(),
        Duration::from_millis(1),
        "window activation",
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::InputFailed);
    assert!(error.message.contains("window activation timed out"));
    assert!(error.message.contains("window session was invalidated"));
}

#[rstest]
fn host_runtime_uses_the_upstream_cursor_renderer_only_where_it_can_run() {
    let options = driver_host_options();
    assert_eq!(UPSTREAM_CURSOR_RENDERER_ENABLED, cfg!(target_os = "linux"));
    assert_eq!(options.cursor.enabled, UPSTREAM_CURSOR_RENDERER_ENABLED);
    assert!(options.host_owns_permission_ux);
    assert!(options.prepare_desktop_environment);
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
fn native_provider_timeout_is_backend_unavailable() {
    let error = map_driver_error(
        "capture CUA window state",
        "InputFailed: get_window_state timed out (UIA provider unresponsive)",
    );
    assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
}

#[rstest]
fn tool_provider_timeout_is_backend_unavailable() {
    let result = cua_driver_sdk::ToolResult {
        is_error: true,
        error_code: Some("input_failed".into()),
        raw_json: "{}".into(),
        text: "get_window_state timed out: UIA provider unresponsive".into(),
        structured_json: None,
        images: Vec::new(),
        degraded: false,
        action: None,
        verification: None,
    };
    assert_eq!(
        ensure_tool_ok("capture CUA window", &result)
            .unwrap_err()
            .code,
        ComputerUseErrorCode::BackendUnavailable
    );
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
    assert!(validate_escalation_request("unknown", None).is_err());
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
fn type_chars_uses_cua_character_input_without_delivery_mode() {
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
    assert!(args.get("delivery_mode").is_none());
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
fn launch_requires_one_safe_application_selector() {
    assert!(validate_launch_request(&ComputerUseLaunchRequest::default()).is_err());
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            name: Some("Calculator".into()),
            ..Default::default()
        })
        .is_ok()
    );
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            name: Some("Calculator".into()),
            bundle_id: Some("com.example.Calculator".into()),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            urls: vec!["com.epicgames.launcher://fab/plugins/egl".into()],
            ..Default::default()
        })
        .is_ok()
    );
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            urls: vec!["file:///C:/Windows/System32/cmd.exe".into()],
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            launch_path: Some("powershell.exe".into()),
            ..Default::default()
        })
        .is_err()
    );
    let json = serde_json::to_value(ComputerUseLaunchRequest {
        name: Some("Calculator".into()),
        ..Default::default()
    })
    .expect("launch request should serialize");
    assert!(json.get("bundle_id").is_none());
    let scoped = launch_arguments(
        &ComputerUseLaunchRequest {
            name: Some("Calculator".into()),
            ..Default::default()
        },
        Some("private-runtime-session"),
    )
    .expect("session-scoped launch arguments");
    assert_eq!(scoped["session"], "private-runtime-session");
}

#[rstest]
fn clipboard_and_recording_requests_are_bounded() {
    assert!(
        validate_clipboard_write_request(&ComputerUseClipboardWriteRequest::default()).is_err()
    );
    assert!(
        validate_clipboard_write_request(&ComputerUseClipboardWriteRequest {
            text: Some("hello".into()),
            image_path: Some("C:\\image.png".into()),
            file_path: None,
        })
        .is_err()
    );
    assert!(
        validate_recording_start_request(&ComputerUseRecordingStartRequest {
            output_dir: "relative/output".into(),
            record_video: false,
        })
        .is_err()
    );
    assert!(
        validate_recording_start_request(&ComputerUseRecordingStartRequest {
            output_dir: "~/cua-recordings".into(),
            record_video: false,
        })
        .is_ok()
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
