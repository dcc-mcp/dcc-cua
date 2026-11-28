use super::*;
use rstest::rstest;

#[rstest]
fn action_schema_exposes_required_fields_and_coordinate_contract() {
    let call = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "dcc_cua_task_call")
        .unwrap();
    let branch = call["inputSchema"]["allOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["if"]["properties"]["method"]["const"] == "execute_action")
        .unwrap();
    let params = &branch["then"]["properties"]["params"];
    assert_eq!(params["required"], json!(["observation_id", "action"]));
    let action = &params["properties"]["action"];
    assert_eq!(
        action["required"],
        json!(["action", "input_kind", "intent"])
    );
    assert!(
        action["properties"]["x"]["description"]
            .as_str()
            .unwrap()
            .contains("snapshot.coordinate_space")
    );
    assert!(
        action["properties"]["keys"]["description"]
            .as_str()
            .unwrap()
            .contains("not key")
    );
    assert_eq!(action["properties"]["path"]["minItems"], 2);
    assert_eq!(action["properties"]["path"]["maxItems"], 201);
    assert_eq!(action["properties"]["duration_ms"]["maximum"], 10000);
    assert!(
        params["properties"]["capture_after"]["description"]
            .as_str()
            .unwrap()
            .contains("do not call snapshot again")
    );
    assert_eq!(
        params["allOf"][0]["then"]["required"],
        json!(["accessibility_state_id"])
    );
}

#[rstest]
fn task_action_schema_matches_the_closed_runtime_contract() {
    let schema = schema::task_action_scope_schema();
    let variants = schema["oneOf"].as_array().unwrap();
    assert_eq!(variants.len(), 10);
    let audio = variants
        .iter()
        .filter(|scope| scope["properties"]["input_kind"]["const"] == "playback_audio")
        .collect::<Vec<_>>();
    assert_eq!(audio, vec![&recording_audio::scope_schema()]);

    let variant = |input_kind: &str| {
        variants
            .iter()
            .find(|variant| {
                variant["properties"]["input_kind"]["const"].as_str() == Some(input_kind)
            })
            .unwrap()
    };
    for action in [
        "minimize_window",
        "set_window_frame",
        "dismiss_menu",
        "capture_preparation_begin",
    ] {
        let mut matching = variants
            .iter()
            .filter(|scope| scope["properties"]["action"]["const"].as_str() == Some(action));
        let scope = matching.next().unwrap();
        assert!(matching.next().is_none());
        assert_eq!(scope["type"], "object");
        assert_eq!(scope["additionalProperties"], false);
        assert_eq!(
            scope["required"],
            json!([
                "action",
                "input_kind",
                "secret_input",
                "authorization_category"
            ])
        );
        assert_eq!(
            scope["properties"],
            json!({
                "action": {"const": action},
                "input_kind": {"const": "window_state"},
                "secret_input": {"const": false},
                "authorization_category": {"const": "window_state"},
                "browser_origin": {"type": "null"}
            })
        );
    }
    for kind in ["semantic", "raw_input", "browser", "clipboard"] {
        let input = &variant(kind)["properties"];
        for window_action in ["capture_preparation_begin", "dismiss_menu"] {
            assert_ne!(input["action"]["const"], window_action);
            if let Some(actions) = input["action"]["enum"].as_array() {
                assert!(!actions.contains(&json!(window_action)));
            }
        }
    }
    let semantic = variant("semantic");
    let native = variant("window_state");
    assert_eq!(native["properties"]["action"]["const"], "minimize_window");
    assert_eq!(native["properties"]["secret_input"]["const"], false);
    assert_eq!(
        native["properties"]["authorization_category"]["const"],
        "window_state"
    );
    assert_eq!(
        semantic["properties"]["action"]["enum"],
        json!([
            "move",
            "scroll",
            "click",
            "double_click",
            "right_click",
            "toggle",
            "drag",
            "type",
            "type_chars",
            "set_text",
            "set_value",
            "set_checked",
            "keypress",
            "press",
            "press_key",
            "keyboard_shortcut",
            "hotkey"
        ])
    );
    let raw_actions = variant("raw_input")["properties"]["action"]["enum"]
        .as_array()
        .unwrap();
    assert_eq!(
        raw_actions.contains(&json!("key_sequence")),
        cfg!(any(windows, target_os = "linux", target_os = "macos"))
    );
    assert_eq!(
        semantic["properties"]["authorization_category"]["enum"],
        json!(TrustedTaskActionScope::SEMANTIC_CATEGORIES)
    );
    assert!(
        !semantic["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("browser_click"))
    );

    let navigation = variants
        .iter()
        .find(|variant| variant["properties"]["action"]["const"] == "browser_extension_navigate")
        .unwrap();
    assert_eq!(navigation["additionalProperties"], false);
    assert_eq!(
        navigation["required"],
        json!([
            "action",
            "input_kind",
            "secret_input",
            "authorization_category",
            "browser_origin"
        ])
    );
    assert_eq!(
        navigation["properties"],
        json!({
            "action":{"const":"browser_extension_navigate"},
            "input_kind":{"const":"browser"}, "secret_input":{"const":false},
            "authorization_category":{"const":"external_effect"},
            "browser_origin":{"type":"string", "minLength":1, "maxLength":2048, "pattern":"^https?://[^/?#@]+$"}
        })
    );
    let browser = variants
        .iter()
        .find(|variant| variant["properties"]["action"]["const"] == "browser_type")
        .unwrap();
    assert_eq!(browser["properties"]["action"]["const"], "browser_type");
    assert_eq!(browser["properties"]["secret_input"]["const"], true);
    assert_eq!(
        browser["properties"]["authorization_category"]["const"],
        "credential"
    );
    assert!(
        browser["required"]
            .as_array()
            .unwrap()
            .contains(&json!("browser_origin"))
    );

    let clipboard = variant("clipboard");
    assert_eq!(
        clipboard["properties"]["action"]["const"],
        "clipboard_capture_secret"
    );
    assert_eq!(clipboard["properties"]["secret_input"]["const"], true);

    let start = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "start_task")
        .unwrap();
    assert_eq!(
        start["inputSchema"]["properties"]["allowed_actions"]["uniqueItems"],
        true
    );
}

#[rstest]
fn pixel_move_scope_matches_native_platform_support() {
    let mut task = pixels_task();
    task["allowed_methods"] = json!(["snapshot", "execute_action"]);
    task["allowed_actions"] = json!([{"action":"move", "input_kind":"raw_input", "secret_input":false, "authorization_category":"raw_input"}]);
    assert_eq!(
        test_server().prepare_task(task).is_ok(),
        cfg!(any(target_os = "linux", target_os = "macos"))
    );
}
