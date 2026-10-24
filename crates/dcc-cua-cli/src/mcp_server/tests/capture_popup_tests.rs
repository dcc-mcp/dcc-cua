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

#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn capture_popup_started_ack_attests_the_actual_independent_ceiling(#[case] enabled: bool) {
    let mut server = test_server();
    let mut input = task();
    input["target_window_handle"] = json!(7);
    input["allow_native_capture_popup_keyboard"] = json!(enabled);
    let prepared = server.prepare_task(input).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    let (session, fake_host) = cleanup_mock_session(Some(json!({
        "type":"session_stopped", "session_id":"cleanup-session", "success":true,
        "active":false, "cleanup_pending":false
    })))
    .await;
    server.proposals.get_mut(id).unwrap().session = Some(session);

    // Exercise the final started producer and MCP envelope, not the ready/status payload.
    let result = tool_result(server.start_task(json!({"task_id":id})).await.unwrap());
    let started = &result["structuredContent"];
    assert_eq!(started["ok"], true);
    assert_eq!(started["provider"], "dcc-cua");
    assert_eq!(started["runtime_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(started["task_id"], id);
    assert_eq!(started["status"], "started");
    assert_eq!(started["target"]["process_id"], 42);
    assert_eq!(started["target"]["window_handle"], 7);
    assert_eq!(started["allow_native_capture_popup_keyboard"], enabled);
    assert_eq!(
        server.task_status(json!({"task_id":id})).unwrap()["allow_native_capture_popup_keyboard"],
        enabled
    );
    assert_eq!(
        server.revoke_task(json!({"task_id":id})).await.unwrap()["status"],
        "stopped"
    );
    fake_host.await.unwrap();
}

#[rstest]
fn pending_host_cleanup_preserves_the_independent_popup_ceiling() {
    let mut server = test_server();
    let prepared = server.prepare_task(task()).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    let proposal = server.proposals.get_mut(id).unwrap();
    proposal.revoked = true;
    proposal.cleanup = Some(
        json!({"success":true,"active":false,"cleanup_pending":false,
        "host_connection_completion":{"completed":false,"success":false}}),
    );
    let status = server.task_status(json!({"task_id":id})).unwrap();
    assert_eq!(status["allow_native_capture_popup_keyboard"], true);
    assert_eq!(status["allowed_actions"], prepared["allowed_actions"]);
    assert_eq!(status["status"], "cleanup_unknown");
    assert_eq!(status["ok"], false);
    assert_eq!(status["cleanup"]["success"], false);
    assert_eq!(status["cleanup"]["cleanup_pending"], true);
    assert_eq!(status["cleanup"]["session_cleanup"]["success"], true);
}
