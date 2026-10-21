use super::*;
use rstest::rstest;

fn task() -> Value {
    let mut task = pixels_task();
    task["allowed_methods"] = json!(["get_window_state", "change_window_state"]);
    task["allowed_actions"] = json!([{"action":"dismiss_menu","input_kind":"window_state","secret_input":false,"authorization_category":"window_state"}]);
    task
}

#[rstest]
fn native_menu_task_requires_independent_scope_and_exact_pixels_binding() {
    let mut server = test_server();
    let prepared = server.prepare_task(task()).unwrap();
    let proposal = &server.proposals[prepared["task_id"].as_str().unwrap()];
    assert!(proposal.registration.allowed_actions[0].is_native_menu_dismiss());
    let mut state = json!({"state":{"maximized":true}});
    frame_recovery::annotate(&mut state, "menu-task", "get_window_state", proposal);
    assert!(state["frame_recovery"].get("next_call").is_none());
    assert_eq!(state["frame_recovery"]["requires_new_task"], true);
    assert_eq!(
        state["frame_recovery"]["blocking_action_scope"],
        "dismiss_menu"
    );
    assert_eq!(state["frame_recovery"]["omit_action_scope"], "dismiss_menu");
    assert!(state["frame_recovery"].get("required_method").is_none());
    let grant = task_session_grant(proposal, proposal.receipt.as_ref().unwrap());
    assert_eq!(grant["allow_raw_input"], false);
    assert_eq!(grant["allow_recording"], false);
    assert_eq!(grant["allow_browser_input"], false);
    for (field, value) in [
        ("observation_mode", json!("semantic")),
        ("surface", json!("browser")),
        ("allowed_methods", json!(["change_window_state"])),
        ("target_process_id", json!(0)),
        ("target_window_handle", json!(0)),
    ] {
        let mut invalid = task();
        invalid[field] = value;
        assert!(test_server().prepare_task(invalid).is_err(), "{field}");
    }
    for (field, value) in [
        ("input_kind", json!("raw_input")),
        ("secret_input", json!(true)),
        ("authorization_category", json!("raw_input")),
        ("browser_origin", json!("https://example.com")),
    ] {
        let mut invalid = task();
        invalid["allowed_actions"][0][field] = value;
        assert!(test_server().prepare_task(invalid).is_err(), "{field}");
    }
}

#[rstest]
fn native_menu_parameters_refuse_frame_pixel_input_and_foreign_fields() {
    let params = json!({"operation":"dismiss_menu","window_state_id":"native-menu-state-1"});
    validate_task_method_params("change_window_state", &params).unwrap();
    for change in 0..10 {
        let mut invalid = params.clone();
        match change {
            0 => invalid["window_state_id"] = json!("native-state-1"),
            1 => invalid["window_state_id"] = json!("snapshot-1"),
            2 => invalid["window_state_id"] = json!("native-menu-state-1\n"),
            3 => invalid["window_state_id"] = json!(7),
            4 => {
                invalid["window_state_id"] =
                    json!("native-menu-state-".to_owned() + &"x".repeat(129))
            }
            5 => invalid["observation_id"] = json!("snapshot-1"),
            6 => invalid["activate"] = json!(true),
            7 => invalid["operation"] = json!("close"),
            8 => invalid["input_kind"] = json!("raw_input"),
            _ => {
                invalid.as_object_mut().unwrap().remove("window_state_id");
            }
        }
        assert!(
            validate_task_method_params("change_window_state", &invalid).is_err(),
            "change {change}"
        );
    }
    assert!(
        validate_task_method_params(
            "change_window_state",
            &json!({"operation":"activate","window_state_id":"native-menu-state-1"})
        )
        .is_err()
    );
}

#[rstest]
fn native_menu_schema_exposes_purpose_and_closed_confirmation_contract() {
    let tools = tool_definitions();
    let call = tools
        .iter()
        .find(|tool| tool["name"] == "dcc_cua_task_call")
        .unwrap();
    let text = call["description"].as_str().unwrap();
    for expected in [
        "menu_dismiss_window_state_id",
        "owner_alive",
        "unconfirmed",
        "Frame and snapshot tokens",
    ] {
        assert!(text.contains(expected), "{expected}");
    }
    assert!(
        call["inputSchema"]["allOf"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["if"]["properties"]["method"]["const"] == "change_window_state")
    );
}

#[rstest]
#[case::windows(true)]
#[case::other_platform(false)]
fn native_menu_manifest_preserves_independent_context_diagnostics(#[case] windows: bool) {
    let manifest = crate::manifest::document_for_platform(windows);
    let runtime = &manifest["runtime"];
    let diagnostics = &runtime["local_diagnostics_only"];
    assert_eq!(
        diagnostics["cli_command"],
        json!(["doctor", "--diagnostics-only"])
    );
    assert_eq!(diagnostics["scope"], "local_process_desktop_metadata");
    assert_eq!(diagnostics["read_only"], true);
    assert_eq!(diagnostics["additional_arguments_accepted"], false);
    assert_eq!(diagnostics["sdk_initialized"], false);
    for field in [
        "sdk_inventory",
        "permissions",
        "health",
        "window_inventory",
        "native_acceptance",
    ] {
        assert_eq!(diagnostics[field], "not_run", "{field}");
    }
    assert_eq!(diagnostics["ready"], false);
    assert_eq!(diagnostics["input_ready"], false);
    if !windows {
        assert!(runtime.get("native_menu_dismiss").is_none());
        return;
    }
    let menu = &runtime["native_menu_dismiss"];
    assert_eq!(
        menu["action_scope"],
        json!({"action":"dismiss_menu","input_kind":"window_state","secret_input":false,"authorization_category":"window_state"})
    );
    assert_eq!(menu["token_field"], "menu_dismiss_window_state_id");
    assert_eq!(menu["token_max_age_ms"], 5000);
    assert_eq!(menu["one_use"], true);
    assert_eq!(menu["frame_token_accepted"], false);
    assert_eq!(menu["dispatch"], "single_bounded_WM_CANCELMODE");
    assert_eq!(menu["activation"], false);
    assert_eq!(menu["input_sent"], false);
    assert_eq!(
        menu["confirmation"],
        json!(["popup_hidden_or_gone", "menu_inactive", "owner_alive"])
    );
    assert_eq!(
        menu["unconfirmed_after_dispatch"],
        "completion_unknown_no_blind_retry"
    );
}

#[rstest]
#[tokio::test]
async fn native_menu_scope_blocks_activation_without_changing_legacy_method_scope() {
    let mut server = test_server();
    let prepared = server.prepare_task(task()).unwrap();
    let task_id = prepared["task_id"].as_str().unwrap();
    for operation in ["activate", "restore_activate"] {
        let error = server.task_call(json!({"task_id":task_id,"method":"change_window_state","params":{"operation":operation}})).await.unwrap_err();
        assert!(error.contains("grants only dismiss_menu"), "{error}");
    }
    let mut ordinary = pixels_task();
    ordinary["allowed_methods"] = json!(["change_window_state"]);
    ordinary["allowed_actions"] = json!([]);
    let prepared = server.prepare_task(ordinary).unwrap();
    let error = server.task_call(json!({"task_id":prepared["task_id"],"method":"change_window_state","params":{"operation":"activate"}})).await.unwrap_err();
    assert!(
        error.contains("call start_task"),
        "legacy activation must pass its unchanged scope fence: {error}"
    );
}
