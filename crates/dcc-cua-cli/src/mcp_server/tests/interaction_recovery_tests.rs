use rstest::rstest;

use super::*;

fn input_task(snapshot: bool, activation: bool) -> Value {
    let mut task = pixels_task();
    let mut methods = vec!["get_window_state", "execute_action"];
    if snapshot {
        methods.push("snapshot");
    }
    if activation {
        methods.push("change_window_state");
    }
    task["allowed_methods"] = json!(methods);
    task["allowed_actions"] = json!([{
        "action":"click", "input_kind":"raw_input", "secret_input":false,
        "authorization_category":"raw_input"
    }]);
    task
}

async fn public_response(task: Value, method: &str, response: Value) -> Value {
    public_responses(task, vec![(method, response)])
        .await
        .remove(0)
}

async fn public_responses(task: Value, calls: Vec<(&str, Value)>) -> Vec<Value> {
    // Only the real framed client and MCP projection run; no native UI is opened.
    let (client_stream, mut host_stream) = tokio::io::duplex(32 * 1024);
    let responses = calls
        .iter()
        .map(|(method, value)| (method.to_string(), value.clone()))
        .collect::<Vec<_>>();
    let host = tokio::spawn(async move {
        let handshake = [
            ("hello".into(), json!({"type":"hello", "capabilities":[]})),
            (
                "open_session".into(),
                json!({"type":"session_opened", "session_id":"recovery-session",
                "window_capability":"test-capability", "target":{"process_id":42,"window_handle":7}}),
            ),
        ];
        for (step, (expected_method, mut value)) in
            handshake.into_iter().chain(responses).enumerate()
        {
            let request: Value = serde_json::from_slice(
                &dcc_cua_protocol::read_frame(
                    &mut host_stream,
                    dcc_cua_protocol::MAX_JSON_FRAME_BYTES,
                )
                .await
                .unwrap()
                .unwrap(),
            )
            .unwrap();
            assert_eq!(request["method"], expected_method);
            if step >= 2 {
                assert_eq!(request["params"]["session_id"], "recovery-session");
                assert_eq!(request["params"]["task_grant_id"], "test-grant");
            }
            value["request_id"] = request["request_id"].clone();
            dcc_cua_protocol::write_frame(
                &mut host_stream,
                value.to_string().as_bytes(),
                dcc_cua_protocol::MAX_JSON_FRAME_BYTES,
            )
            .await
            .unwrap();
            host_stream.flush().await.unwrap();
        }
    });
    let mut client = HostClient::from_stream(client_stream);
    client
        .hello("pure-interaction-recovery-test")
        .await
        .unwrap();
    let session = client
        .open_logical_task_session(
            "recovery-session",
            json!({"task_grant_id":"test-grant"}),
            60_000,
        )
        .await
        .unwrap();
    let mut server = test_server();
    let prepared = server.prepare_task(task).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    server.proposals.get_mut(id).unwrap().session = Some(session);
    let mut results = Vec::new();
    for (method, _) in calls {
        let params = if method == "change_window_state" {
            json!({"operation":"activate"})
        } else {
            json!({})
        };
        let result = server
            .task_call(json!({"task_id":id,"method":method,"params":params}))
            .await
            .unwrap();
        results.push(result["structuredContent"].clone());
    }
    host.await.unwrap();
    results
}

fn window_state(foreground: bool) -> Value {
    json!({"type":"window_state", "state":{
        "process_id":42, "window_handle":7, "exists":true,
        "visible":true, "minimized":false, "foreground":foreground
    }})
}

fn activation_refusal() -> Value {
    json!({"type":"error", "code":"foreground_activation_refused", "message":"native refusal",
        "details":{"foreground_activation":foreground_remote_diagnostic_fixture(),
            "background_delivery_viable":true,"suggested_delivery_mode":"background"}})
}

#[rstest]
#[case(false, true, true)]
#[case(true, true, true)]
#[case(true, true, false)]
#[case(false, false, true)]
#[case(false, true, false)]
#[tokio::test]
async fn public_window_state_separates_capture_from_physical_input(
    #[case] foreground: bool,
    #[case] snapshot: bool,
    #[case] activation: bool,
) {
    let response = public_response(
        input_task(snapshot, activation),
        "get_window_state",
        window_state(foreground),
    )
    .await;
    let recovery = &response["interaction_recovery"];
    assert_eq!(recovery["observation"]["snapshot_authorized"], snapshot);
    assert_eq!(recovery["observation"]["foreground_required"], false);
    assert_eq!(
        recovery["observation"]["fresh_capture_proof_required"],
        true
    );
    assert_eq!(recovery["input"]["execute_action_authorized"], true);
    assert_eq!(recovery["input"]["foreground_observed"], foreground);
    assert_eq!(recovery["input"]["foreground_required"], true);
    assert_eq!(recovery["input"]["background_delivery_supported"], false);
    assert_eq!(recovery["input"]["fresh_observation_required"], true);
    assert_eq!(recovery["input"]["readiness_attested"], false);
    assert_eq!(recovery["activation"]["method_authorized"], activation);
    assert_eq!(recovery["activation"]["blind_retry"], false);
    assert_eq!(recovery["activation"]["status"], "history_unknown");
    assert_eq!(recovery["activation"]["prior_refusal_overridden"], false);
    assert_eq!(
        recovery["activation"]["next_step"],
        if foreground {
            "fresh_observation_before_input"
        } else if !activation {
            "activation_scope_required"
        } else {
            "assess_input_handoff"
        }
    );
}

#[rstest]
#[tokio::test]
async fn passive_state_after_refusal_cannot_reopen_activation_guidance() {
    let responses = public_responses(
        input_task(true, true),
        vec![
            ("get_window_state", window_state(false)),
            ("change_window_state", activation_refusal()),
            ("get_window_state", window_state(false)),
        ],
    )
    .await;
    assert_eq!(
        responses[1]["interaction_recovery"]["activation"]["status"],
        "refused"
    );
    assert_eq!(
        responses[1]["interaction_recovery"]["activation"]["next_step"],
        "diagnose_native_refusal"
    );
    let after = &responses[2]["interaction_recovery"];
    assert_eq!(after["activation"]["status"], "history_unknown");
    assert_eq!(after["activation"]["prior_refusal_overridden"], false);
    assert_eq!(after["activation"]["next_step"], "assess_input_handoff");
    assert_eq!(after["activation"]["automatic_retry"], false);
    assert_eq!(after["activation"]["blind_retry"], false);
    assert_eq!(after["input"]["readiness_attested"], false);
    assert_eq!(after["observation"]["snapshot_authorized"], true);
}

#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn refusal_preserves_native_diagnostics_without_granting_background_input(
    #[case] snapshot: bool,
) {
    let response = public_response(
        input_task(snapshot, true),
        "change_window_state",
        activation_refusal(),
    )
    .await;
    assert_eq!(response["ok"], false);
    assert_eq!(
        response["details"]["foreground_activation"],
        foreground_remote_diagnostic_fixture()
    );
    assert_eq!(response["details"]["background_delivery_viable"], true);
    assert_eq!(response["details"]["suggested_delivery_mode"], "background");
    let recovery = &response["interaction_recovery"];
    assert_eq!(recovery["observation"]["snapshot_authorized"], snapshot);
    assert_eq!(recovery["input"]["background_delivery_supported"], false);
    assert_eq!(recovery["input"]["readiness_attested"], false);
    assert_eq!(recovery["activation"]["status"], "refused");
    assert_eq!(recovery["activation"]["blind_retry"], false);
    assert_eq!(recovery["activation"]["automatic_retry"], false);
    assert_eq!(
        recovery["activation"]["next_step"],
        "diagnose_native_refusal"
    );
}

#[rstest]
#[case("missing_details")]
#[case("malformed_diagnostic")]
#[case("wrong_target")]
#[case("other_error")]
#[tokio::test]
async fn unrelated_or_unverified_errors_do_not_invent_recovery(#[case] variant: &str) {
    let mut response = activation_refusal();
    match variant {
        "missing_details" => {
            response.as_object_mut().unwrap().remove("details");
        }
        "malformed_diagnostic" => {
            response["details"]["foreground_activation"]["attempts"][0]["phase"] = json!("unknown")
        }
        "wrong_target" => {
            response["details"]["foreground_activation"]["target"]["process_id"] = json!(99)
        }
        "other_error" => response["code"] = json!("target_unavailable"),
        _ => unreachable!(),
    }
    let result = public_response(input_task(true, true), "change_window_state", response).await;
    assert!(result.get("interaction_recovery").is_none());
}

#[rstest]
#[case("missing_foreground")]
#[case("wrong_target")]
#[case("missing_target")]
#[case("wrong_type")]
#[case("missing_exists")]
#[case("invalid_exists")]
#[case("not_exists")]
#[case("missing_visible")]
#[case("invalid_visible")]
#[case("not_visible")]
#[case("missing_minimized")]
#[case("invalid_minimized")]
#[case("minimized")]
#[case("semantic")]
#[tokio::test]
async fn incomplete_or_other_route_state_is_not_a_capture_or_input_certificate(
    #[case] variant: &str,
) {
    let mut task = input_task(true, true);
    let mut response = window_state(false);
    match variant {
        "missing_foreground" => {
            response["state"]
                .as_object_mut()
                .unwrap()
                .remove("foreground");
        }
        "wrong_target" => response["state"]["window_handle"] = json!(8),
        "missing_target" => {
            response["state"]
                .as_object_mut()
                .unwrap()
                .remove("process_id");
        }
        "wrong_type" => response["type"] = json!("unrelated_response"),
        "missing_exists" => {
            response["state"].as_object_mut().unwrap().remove("exists");
        }
        "invalid_exists" => response["state"]["exists"] = json!("true"),
        "not_exists" => response["state"]["exists"] = json!(false),
        "missing_visible" => {
            response["state"].as_object_mut().unwrap().remove("visible");
        }
        "invalid_visible" => response["state"]["visible"] = json!("true"),
        "not_visible" => response["state"]["visible"] = json!(false),
        "missing_minimized" => {
            response["state"]
                .as_object_mut()
                .unwrap()
                .remove("minimized");
        }
        "invalid_minimized" => response["state"]["minimized"] = json!("false"),
        "minimized" => response["state"]["minimized"] = json!(true),
        "semantic" => {
            task["observation_mode"] = json!("semantic");
        }
        _ => unreachable!(),
    }
    let result = public_response(task, "get_window_state", response).await;
    assert!(result.get("interaction_recovery").is_none());
}

#[rstest]
#[case("missing")]
#[case("malformed")]
#[tokio::test]
async fn native_background_hints_are_not_needed_to_explain_closed_input(#[case] variant: &str) {
    let mut response = activation_refusal();
    if variant == "missing" {
        response["details"]
            .as_object_mut()
            .unwrap()
            .remove("background_delivery_viable");
        response["details"]
            .as_object_mut()
            .unwrap()
            .remove("suggested_delivery_mode");
    } else {
        response["details"]["background_delivery_viable"] = json!("true");
        response["details"]["suggested_delivery_mode"] = json!("unknown");
    }
    let result = public_response(input_task(true, true), "change_window_state", response).await;
    assert!(
        result["details"]
            .get("background_delivery_viable")
            .is_none()
    );
    assert!(result["details"].get("suggested_delivery_mode").is_none());
    assert_eq!(
        result["interaction_recovery"]["input"]["background_delivery_supported"],
        false
    );
    assert_eq!(
        result["interaction_recovery"]["activation"]["next_step"],
        "diagnose_native_refusal"
    );
}

#[rstest]
#[tokio::test]
async fn observation_only_and_menu_only_tasks_do_not_gain_input_or_activation() {
    let mut task = input_task(true, true);
    task["allowed_methods"] = json!(["get_window_state", "snapshot"]);
    task["allowed_actions"] = json!([]);
    let response = public_response(task, "get_window_state", window_state(false)).await;
    assert_eq!(
        response["interaction_recovery"]["input"]["execute_action_authorized"],
        false
    );
    assert_eq!(
        response["interaction_recovery"]["activation"]["method_authorized"],
        false
    );
    let mut task = pixels_task();
    task["allowed_methods"] = json!(["get_window_state", "change_window_state"]);
    task["allowed_actions"] = json!([{"action":"dismiss_menu","input_kind":"window_state","secret_input":false,"authorization_category":"window_state"}]);
    let response = public_response(task, "get_window_state", window_state(false)).await;
    assert_eq!(
        response["interaction_recovery"]["activation"]["method_authorized"],
        false
    );
}
