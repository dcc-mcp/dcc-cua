use super::*;
use rstest::rstest;

/// Keep schema checks bound to the real SDK inventory. On Windows, inspect
/// metadata without requiring an interactive desktop or starting an overlay.
/// Other platforms retain the normal DCC runtime: Linux registry inspection
/// itself configures process-global cursor state and must not disable it here.
pub(super) async fn native_sdk_inventory() -> Value {
    #[cfg(windows)]
    {
        cua_driver_sdk::CuaDriver::inspect_host_tools(cua_driver_sdk::DriverHostOptions {
            cursor: cursor_overlay::CursorConfig {
                enabled: false,
                ..Default::default()
            },
            host_owns_permission_ux: true,
            host_bundle_id: None,
            claude_code_compatibility: false,
            prepare_desktop_environment: false,
            register_host_tools: None,
            authorization_host: None,
            activity_observer: None,
        })
    }
    #[cfg(not(windows))]
    {
        let driver = ComputerUseDriver::create().unwrap();
        driver.list_tools().await.unwrap()
    }
}

/// Read the real embedded SDK registry. No native input is dispatched.
#[rstest]
#[tokio::test]
async fn mapped_actions_use_only_actual_native_sdk_arguments() {
    let inventory = native_sdk_inventory().await;
    let target = test_window_target();
    let mut failures = Vec::new();
    for action_name in [
        "move",
        "click",
        "double_click",
        "right_click",
        "drag",
        "scroll",
        "type",
        "type_chars",
        "set_value",
        "keypress",
        "keyboard_shortcut",
    ] {
        let action = ComputerUseAction {
            action: action_name.into(),
            x: (action_name != "type_chars").then_some(20.0),
            y: (action_name != "type_chars").then_some(30.0),
            type_chars_only: action_name == "type_chars",
            text: Some("test".into()),
            keys: vec!["a".into()],
            scroll_y: (action_name == "scroll").then_some(1),
            element_token: (action_name == "set_value").then(|| "s00000001:1".into()),
            ..Default::default()
        };
        validate_action(&action).unwrap();
        let mut commands = vec![action_arguments(&action, "session", &target).unwrap()];
        if action_name == "set_value" {
            assert!(desktop_action_arguments(&action, "session").is_err());
        } else {
            commands.push(desktop_action_arguments(&action, "session").unwrap());
        }
        for command in commands {
            let schema = tool_schema_from_inventory(&inventory, command.tool).unwrap();
            let properties = schema["properties"].as_object().unwrap();
            for key in command.arguments.as_object().unwrap().keys() {
                if !properties.contains_key(key) {
                    failures.push(format!("{}: unsupported {key}", command.tool));
                }
            }
            for required in schema["required"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !command
                    .arguments
                    .as_object()
                    .unwrap()
                    .contains_key(required)
                {
                    failures.push(format!("{}: missing {required}", command.tool));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[rstest]
#[case("move")]
#[case("drag")]
fn coordinate_only_sdk_actions_refuse_semantic_selectors(#[case] action_name: &str) {
    let action = ComputerUseAction {
        action: action_name.into(),
        element_token: Some("snapshot:1".into()),
        ..Default::default()
    };
    assert_eq!(
        action_arguments(&action, "session", &test_window_target())
            .unwrap_err()
            .code,
        ComputerUseErrorCode::InvalidAction
    );
}

#[rstest]
fn unresolved_sdk_index_is_never_emitted_as_a_bare_row_token() {
    let action = ComputerUseAction {
        action: "click".into(),
        element_index: Some(1),
        ..Default::default()
    };
    assert_eq!(
        action_arguments(&action, "session", &test_window_target())
            .unwrap_err()
            .code,
        ComputerUseErrorCode::StaleObservation
    );
}

#[cfg(target_os = "linux")]
#[rstest]
fn unsupported_linux_character_delay_is_explicitly_refused() {
    let action = ComputerUseAction {
        action: "type_chars".into(),
        text: Some("hello".into()),
        type_chars_only: true,
        delay_ms: Some(20),
        ..Default::default()
    };
    validate_action(&action).unwrap();
    for result in [
        action_arguments(&action, "session", &test_window_target()),
        desktop_action_arguments(&action, "session"),
    ] {
        let error = result.unwrap_err();
        assert_eq!(error.code, ComputerUseErrorCode::InvalidAction);
        assert!(
            error
                .message
                .contains("does not support type_chars delay_ms")
        );
    }
}

#[rstest]
#[tokio::test]
async fn native_sdk_advertises_bounded_full_observation_shape() {
    let inventory = native_sdk_inventory().await;
    let schema = tool_schema_from_inventory(&inventory, "get_window_state").unwrap();
    assert!(
        schema["properties"]["tree_format"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("both"))
    );
    for bound in [
        "pid",
        "window_id",
        "include_screenshot",
        "max_elements",
        "max_depth",
    ] {
        assert!(schema["properties"].get(bound).is_some());
    }
}

/// Adjacent native menu calls keep the public owner/path shape. The trusted
/// `_session_id` is injected separately by the embedded SDK, not its public schema.
#[rstest]
#[tokio::test]
async fn native_sdk_accepts_exact_menu_request_shape() {
    let inventory = native_sdk_inventory().await;
    let schema = tool_schema_from_inventory(&inventory, "invoke_menu").unwrap();
    let properties = schema["properties"].as_object().unwrap();
    for key in ["pid", "window_id", "path"] {
        assert!(
            properties.contains_key(key),
            "unsupported menu argument: {key}"
        );
    }
    for required in schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        assert!(
            ["pid", "window_id", "path"].contains(&required),
            "unmapped menu requirement: {required}"
        );
    }
}
