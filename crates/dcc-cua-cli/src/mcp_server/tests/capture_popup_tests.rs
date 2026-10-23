use super::*;
use rstest::rstest;

fn task() -> Value {
    json!({"application_label":"Popup keyboard", "surface":"window", "observation_mode":"pixels_only",
        "target_process_id":42,"target_window_handle":77,"allow_native_capture_popup_keyboard":true,
        "allowed_methods":["snapshot","get_window_state","execute_action"],
        "allowed_actions":[{"action":"keypress","input_kind":"raw_input","secret_input":false,"authorization_category":"raw_input"}]})
}

#[rstest]
fn capture_popup_ceiling_is_independent_and_never_grants_a_keyboard_action() {
    let mut server = test_server();
    let prepared = server.prepare_task(task()).unwrap();
    assert_eq!(prepared["allow_native_capture_popup_keyboard"], true);
    let proposal = &server.proposals[prepared["task_id"].as_str().unwrap()];
    assert_eq!(proposal.registration.allowed_actions.len(), 2);
    assert!(
        proposal
            .registration
            .allowed_actions
            .iter()
            .any(TrustedTaskActionScope::is_native_capture_popup_keyboard)
    );
    let grant = task_session_grant(proposal, proposal.receipt.as_ref().unwrap());
    assert!(grant.get("allow_native_capture_popup_keyboard").is_none());
    for (field, value) in [
        ("allowed_actions", json!([])),
        (
            "allowed_actions",
            json!([{"action":"click","input_kind":"raw_input","secret_input":false,"authorization_category":"raw_input"}]),
        ),
        ("surface", json!("browser")),
        ("observation_mode", json!("semantic")),
        ("allowed_methods", json!(["snapshot", "get_window_state"])),
    ] {
        let mut invalid = task();
        invalid[field] = value;
        assert!(server.prepare_task(invalid).is_err(), "{field}");
    }
    let mut normal = task();
    normal["allow_native_capture_popup_keyboard"] = json!(false);
    let prepared = server.prepare_task(normal).unwrap();
    assert_eq!(prepared["allowed_actions"].as_array().unwrap().len(), 1);
    assert_eq!(prepared["allow_native_capture_popup_keyboard"], false);
}

#[rstest]
fn capture_popup_snapshot_selection_is_closed_and_numeric() {
    for selected in [
        json!([0]),
        json!([1, 1]),
        json!([-1]),
        json!([1.5]),
        json!(["1"]),
        json!([1, 2, 3, 4, 5, 6, 7, 8, 9]),
        json!(true),
    ] {
        assert!(
            validate_task_method_params(
                "snapshot",
                &json!({"native_capture_popup_window_handles":selected})
            )
            .is_err()
        );
    }
    assert!(
        validate_task_method_params(
            "snapshot",
            &json!({"native_capture_popup_window_handles":[1,2]})
        )
        .is_ok()
    );
    assert!(
        validate_task_method_params(
            "snapshot",
            &json!({"native_capture_popup_window_handles":[]})
        )
        .is_ok()
    );
    assert!(
        validate_task_method_params(
            "execute_action",
            &json!({"native_capture_popup_window_handles":[1]})
        )
        .is_err()
    );
}

#[rstest]
fn capture_popup_owner_flag_is_advertised_as_disabled_by_default() {
    let start = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "start_task")
        .unwrap();
    let field = &start["inputSchema"]["properties"]["allow_native_capture_popup_keyboard"];
    assert_eq!(field["type"], "boolean");
    assert_eq!(field["default"], false);
}
