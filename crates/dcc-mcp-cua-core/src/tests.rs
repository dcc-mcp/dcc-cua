use rstest::rstest;

use super::*;

#[rstest]
fn snapshot_bounds_use_agent_defaults_and_cap_context() {
    assert_eq!(bounded_snapshot_elements(0), DEFAULT_SNAPSHOT_MAX_ELEMENTS);
    assert_eq!(bounded_snapshot_depth(0), DEFAULT_SNAPSHOT_MAX_DEPTH);
    assert_eq!(bounded_snapshot_elements(u32::MAX), MAX_SNAPSHOT_ELEMENTS);
    assert_eq!(bounded_snapshot_depth(u32::MAX), MAX_SNAPSHOT_DEPTH);
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
fn native_tool_boundary_rejects_reserved_and_dedicated_routes() {
    assert!(validate_native_tool_request("debug_window_info", &json!({})).is_ok());
    assert!(validate_native_tool_request("bad-name", &json!({})).is_err());
    assert!(validate_native_tool_request("debug_window_info", &json!({"_tool":"x"})).is_err());
    assert!(!native_tool_allowed_in_window_session("click"));
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
    let valid = serde_json::json!({"x": 10, "y": 20});
    assert!(validate_window_cursor_move(valid.as_object().unwrap()).is_ok());

    let desktop = serde_json::json!({"x": 10, "y": 20, "scope": "desktop"});
    assert!(validate_window_cursor_move(desktop.as_object().unwrap()).is_err());

    let negative = serde_json::json!({"x": -1, "y": 20});
    assert!(validate_window_cursor_move(negative.as_object().unwrap()).is_err());
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
            z_index: 1,
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
            z_index: 1,
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
            z_index: 1,
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
        z_index: 1,
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
            z_index: 1,
            is_foreground: true,
        },
    );
    assert_eq!(args["_tool"], "type_text_chars");
    assert_eq!(args["text"], "Fab");
    assert_eq!(args["delay_ms"], 20);
    assert_eq!(args["element_index"], 4);
    assert!(args.get("delivery_mode").is_none());

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
