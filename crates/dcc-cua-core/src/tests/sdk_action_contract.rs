use super::*;
use rstest::rstest;

/// Read the real embedded SDK registry. No native input is dispatched.
#[rstest]
#[tokio::test]
async fn mapped_actions_use_only_actual_native_sdk_arguments() {
    let driver = ComputerUseDriver::create().unwrap();
    let inventory = driver.list_tools().await.unwrap();
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
