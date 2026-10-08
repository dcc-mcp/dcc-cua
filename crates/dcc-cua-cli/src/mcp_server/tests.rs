use rstest::rstest;
use serde_json::{Value, json};

use super::*;

fn test_server() -> TaskAuthorizationServer {
    let mut server = TaskAuthorizationServer::automatic();
    server.recording_output_root = None;
    server
}

fn native_recording_task() -> Value {
    let mut task = pixels_task();
    task["allowed_methods"] = json!(["recording_start", "recording_state", "recording_stop"]);
    task["allowed_actions"] = json!([]);
    task["allow_recording"] = json!(true);
    task
}

#[rstest]
fn native_recording_public_scope_requires_operator_owned_output_and_complete_lifecycle() {
    assert!(
        test_server()
            .prepare_task(native_recording_task())
            .unwrap_err()
            .contains("operator configuration")
    );
    let root = tempfile::tempdir().unwrap();
    let mut server = test_server();
    server.recording_output_root = Some(root.path().to_owned());
    for (field, value) in [
        ("allow_recording", json!(false)),
        ("allowed_methods", json!(["recording_state"])),
        (
            "allowed_methods",
            json!(["recording_start", "recording_state"]),
        ),
        ("observation_mode", json!("semantic")),
        ("recording_output_dir", json!("caller-owned")),
        ("recording_output_root", json!("caller-owned")),
    ] {
        let mut task = native_recording_task();
        task[field] = value;
        assert!(server.prepare_task(task).is_err(), "{field}");
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    let prepared = server.prepare_task(native_recording_task()).unwrap();
    let proposal = &server.proposals[prepared["task_id"].as_str().unwrap()];
    let directory = proposal.registration.recording_output_dir.as_ref().unwrap();
    assert_eq!(std::path::Path::new(directory).parent(), Some(root.path()));
    assert!(std::path::Path::new(directory).is_dir());
    let grant = task_session_grant(proposal, proposal.receipt.as_ref().unwrap());
    assert_eq!(grant["allow_recording"], true);
    assert_eq!(grant["recording_output_dir"], *directory);
    assert!(grant["showcase_output_dir"].is_null());
    assert_eq!(grant["allow_raw_input"], false);
    let mut params = json!({});
    bind_recording_request(&mut params, Some(directory)).unwrap();
    assert_eq!(
        params,
        json!({"request":{"record_video":true,"output_dir":directory}})
    );
    assert!(
        bind_recording_request(
            &mut json!({"request":{"output_dir":"elsewhere"}}),
            Some(directory)
        )
        .is_err()
    );
    assert!(bind_recording_request(&mut json!({}), None).is_err());
}

#[rstest]
#[case("recording_start", json!({}), true)]
#[case("recording_start", json!({"request":{"record_video":true}}), true)]
#[case("recording_start", json!({"request":{"record_video":false}}), false)]
#[case("recording_start", json!({"request":{"record_video":1}}), false)]
#[case("recording_start", json!({"request":{"trajectory":true}}), false)]
#[case("recording_start", json!({"output_dir":"outside"}), false)]
#[case("recording_state", json!({"output_dir":"outside"}), false)]
#[case("recording_stop", json!({}), true)]
#[case("live_observation_start", json!({"request":{"fps":30,"max_dimension":256}}), true)]
#[case("live_observation_start", json!({"request":{"fps":31}}), false)]
#[case("live_observation_start", json!({"request":{"fps":1.5}}), false)]
#[case("live_observation_start", json!({"request":{"max_dimension":4097}}), false)]
#[case("live_observation_start", json!({"request":{"capture_backend":"unguarded"}}), false)]
#[case("live_observation_stop", json!({"session_id":"other"}), false)]
fn native_recording_requests_are_bounded(
    #[case] method: &str,
    #[case] params: Value,
    #[case] allowed: bool,
) {
    assert_eq!(
        validate_task_method_params(method, &params).is_ok(),
        allowed
    );
}

// A real framed client transport with no driver or native application.
async fn cleanup_mock_session(
    response: Option<Value>,
) -> (LogicalTaskSession, tokio::task::JoinHandle<()>) {
    cleanup_mock_session_with_calls(response, Vec::new()).await
}

async fn cleanup_mock_session_with_calls(
    response: Option<Value>,
    calls: Vec<(&'static str, Value, Value)>,
) -> (LogicalTaskSession, tokio::task::JoinHandle<()>) {
    use dcc_cua_protocol::{MAX_JSON_FRAME_BYTES, read_frame, write_frame};
    let (client_stream, mut host_stream) = tokio::io::duplex(16 * 1024);
    let server = tokio::spawn(async move {
        for mut value in [
            json!({"type":"hello","capabilities":[]}),
            json!({"type":"session_opened","session_id":"cleanup-session","window_capability":"cleanup-cap","target":{"process_id":42,"window_handle":7}}),
        ] {
            let request: Value = serde_json::from_slice(
                &read_frame(&mut host_stream, MAX_JSON_FRAME_BYTES)
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            value["request_id"] = request["request_id"].clone();
            write_frame(
                &mut host_stream,
                &serde_json::to_vec(&value).unwrap(),
                MAX_JSON_FRAME_BYTES,
            )
            .await
            .unwrap();
        }
        for (method, request, mut response) in calls {
            let actual: Value = serde_json::from_slice(
                &read_frame(&mut host_stream, MAX_JSON_FRAME_BYTES)
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(actual["method"], method);
            assert_eq!(actual["params"]["session_id"], "cleanup-session");
            assert_eq!(actual["params"]["task_grant_id"], "cleanup-grant");
            assert_eq!(actual["params"]["window_capability"], "cleanup-cap");
            assert_eq!(actual["params"]["request"], request);
            response["request_id"] = actual["request_id"].clone();
            write_frame(
                &mut host_stream,
                &serde_json::to_vec(&response).unwrap(),
                MAX_JSON_FRAME_BYTES,
            )
            .await
            .unwrap();
        }
        let stop: Value = serde_json::from_slice(
            &read_frame(&mut host_stream, MAX_JSON_FRAME_BYTES)
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(stop["method"], "stop_session");
        assert_eq!(stop["params"]["session_id"], "cleanup-session");
        if let Some(mut response) = response {
            response["request_id"] = stop["request_id"].clone();
            write_frame(
                &mut host_stream,
                &serde_json::to_vec(&response).unwrap(),
                MAX_JSON_FRAME_BYTES,
            )
            .await
            .unwrap();
        }
    });
    let mut client =
        HostClient::from_stream_with_transport(client_stream, SnapshotTransport::BinaryFrame);
    client.hello("pure-cleanup-test").await.unwrap();
    let session = client
        .open_logical_task_session(
            "cleanup-session",
            json!({"task_grant_id":"cleanup-grant"}),
            60_000,
        )
        .await
        .unwrap();
    (session, server)
}

#[rstest]
#[tokio::test]
async fn native_recording_public_calls_keep_the_same_session_and_bind_output() {
    let root = tempfile::tempdir().unwrap();
    let mut server = test_server();
    server.recording_output_root = Some(root.path().to_owned());
    let mut task = native_recording_task();
    task["allowed_methods"] = json!([
        "recording_start",
        "recording_state",
        "recording_stop",
        "live_observation_start",
        "live_observation_state",
        "live_observation_stop"
    ]);
    let prepared = server.prepare_task(task).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    let directory = prepared["recording_output_dir"].as_str().unwrap();
    let pairs = [
        (
            "live_observation_start",
            json!({}),
            "live_observation_started",
        ),
        (
            "live_observation_state",
            Value::Null,
            "live_observation_state",
        ),
        (
            "recording_start",
            json!({"record_video":true,"output_dir":directory}),
            "recording_started",
        ),
        ("recording_state", Value::Null, "recording_state"),
        ("recording_stop", Value::Null, "recording_stopped"),
        (
            "live_observation_stop",
            Value::Null,
            "live_observation_stopped",
        ),
    ];
    let (session, fake_host) = cleanup_mock_session_with_calls(
        Some(json!({"type":"session_stopped","session_id":"cleanup-session","success":true,"active":false,"cleanup_pending":false})),
        pairs.iter().map(|(method,request,response)| (*method,request.clone(),json!({"type":response,"session_id":"cleanup-session","result":{"backend":"pure_mock","trajectory_available":false}}))).collect()
    ).await;
    server.proposals.get_mut(id).unwrap().session = Some(session);
    for method in [
        "live_observation_start",
        "live_observation_state",
        "recording_start",
        "recording_state",
        "recording_stop",
        "live_observation_stop",
    ] {
        let result = server
            .task_call(json!({"task_id":id,"method":method,"params":{}}))
            .await
            .unwrap();
        assert_eq!(result["structuredContent"]["session_id"], "cleanup-session");
        assert_eq!(result["structuredContent"]["task_context"]["task_id"], id);
        assert_eq!(
            result["structuredContent"]["result"]["backend"],
            "pure_mock"
        );
    }
    let stopped = server.revoke_task(json!({"task_id":id})).await.unwrap();
    assert_eq!(stopped["status"], "stopped");
    fake_host.await.unwrap();
}

#[rstest]
#[case(Some(json!({"type":"session_stopped","session_id":"cleanup-session","success":true,"active":false,"cleanup_pending":false})), "stopped")]
#[case(Some(json!({"type":"session_stopped","session_id":"cleanup-session","success":false,"active":false,"cleanup_pending":false,"cleanup_issues":[{"component":"recording","message":"encoder failed"}]})), "cleanup_failed")]
#[case(Some(json!({"type":"session_stopped","session_id":"wrong","success":true,"active":false,"cleanup_pending":false})), "cleanup_unknown")]
#[case(Some(json!({"type":"session_stopped","session_id":"cleanup-session","success":true,"active":false})), "cleanup_unknown")]
#[case(None, "cleanup_unknown")]
#[rstest]
#[tokio::test]
async fn native_recording_task_stop_preserves_authoritative_cleanup(
    #[case] response: Option<Value>,
    #[case] expected: &str,
) {
    let (session, fake_host) = cleanup_mock_session(response).await;
    let mut server = test_server();
    let mut task = pixels_task();
    task["allowed_actions"] = json!([]);
    task["allowed_methods"] = json!(["snapshot"]);
    let prepared = server.prepare_task(task).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    server.proposals.get_mut(id).unwrap().session = Some(session);
    let stopped = server.revoke_task(json!({"task_id":id})).await.unwrap();
    assert_eq!(stopped["status"], expected);
    assert_eq!(stopped["ok"], expected == "stopped");
    if expected == "cleanup_failed" {
        assert_eq!(
            stopped["cleanup"]["cleanup_issues"][0]["component"],
            "recording"
        );
    }
    assert_eq!(
        server.revoke_task(json!({"task_id":id})).await.unwrap(),
        stopped
    );
    assert_eq!(
        server.task_status(json!({"task_id":id})).unwrap()["status"],
        expected
    );
    assert!(
        server
            .task_call(json!({"task_id":id,"method":"snapshot","params":{}}))
            .await
            .unwrap_err()
            .contains("stopped")
    );
    fake_host.await.unwrap();
}

#[rstest]
#[tokio::test]
async fn native_recording_mcp_shutdown_awaits_cleanup_and_reports_failures() {
    let (session, fake_host) = cleanup_mock_session(Some(json!({"type":"session_stopped","session_id":"cleanup-session","success":false,"active":false,"cleanup_pending":false,"cleanup_issues":["partial video"]}))).await;
    let mut server = test_server();
    let prepared = server.prepare_task(pixels_task()).unwrap();
    server
        .proposals
        .get_mut(prepared["task_id"].as_str().unwrap())
        .unwrap()
        .session = Some(session);
    let failures = server.shutdown_tasks().await;
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0]["status"], "cleanup_failed");
    assert_eq!(failures[0]["cleanup"]["cleanup_issues"][0], "partial video");
    fake_host.await.unwrap();
}

#[rstest]
#[tokio::test]
async fn native_recording_start_without_an_owned_session_ack_cannot_claim_cleanup() {
    let mut server = test_server();
    let prepared = server.prepare_task(pixels_task()).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    server.proposals.get_mut(id).unwrap().session_open_attempted = true;
    assert_eq!(
        server.revoke_task(json!({"task_id":id})).await.unwrap()["status"],
        "cleanup_unknown"
    );
    assert_eq!(server.shutdown_tasks().await.len(), 1);
    let mut server = test_server();
    let prepared = server.prepare_task(pixels_task()).unwrap();
    assert!(
        server
            .task_call(json!({"task_id":prepared["task_id"],"method":"snapshot","params":[]}))
            .await
            .unwrap_err()
            .contains("must be an object")
    );
}

fn root_bounds_capture_fixture() -> Value {
    json!({
        "stage":"visible_desktop_proof", "reason":"root_bounds_invalid",
        "target_process_id":42, "target_window_handle":7,
        "blocker_window_handle":91, "blocker_process_id":100,
        "PRIVATE_CAPTURE_FIELD":"PRIVATE_CAPTURE_VALUE",
        "root_bounds_failure":{
            "root_role":"above_target_root", "proof_target_root_window_handle":77,
            "dwm_raw_rect_edges":[10,20,10,30], "dwm_classification":"zero_area",
            "visible":true, "cloaked":0, "win32_read_after_dwm_rejection":true,
            "win32_raw_rect_edges":[10,20,30,40], "win32_classification":"positive",
            "win32_os_error":null, "zero_area_status_mismatch":true,
            "PRIVATE_ROOT_FIELD":"PRIVATE_ROOT_VALUE"
        }
    })
}

#[rstest]
#[case("valid")]
#[case("missing")]
#[case("invalid_class")]
#[case("invalid_rect")]
#[case("invalid_reason")]
#[rstest]
#[tokio::test]
async fn root_bounds_failure_survives_actual_public_task_remote_error_adapter(
    #[case] variant: &str,
) {
    // Exercise the real framed Host client and task_call adapter. This fake
    // server performs no driver, native capture, Host startup, or window call.
    let mut capture = root_bounds_capture_fixture();
    match variant {
        "missing" => capture = Value::Null,
        "invalid_class" => {
            capture["root_bounds_failure"]["dwm_classification"] = json!("PRIVATE_ERROR")
        }
        "invalid_rect" => capture["root_bounds_failure"]["dwm_raw_rect_edges"] = json!([0, 0, 0]),
        "invalid_reason" => capture["reason"] = json!("PRIVATE_ERROR"),
        "valid" => (),
        _ => unreachable!(),
    }
    let (client_stream, mut host_stream) = tokio::io::duplex(32 * 1024);
    let host_task = tokio::spawn(async move {
        for step in 0..3 {
            let bytes = dcc_cua_protocol::read_frame(
                &mut host_stream,
                dcc_cua_protocol::MAX_JSON_FRAME_BYTES,
            )
            .await
            .unwrap()
            .unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            let mut response = match step {
                0 => {
                    assert_eq!(request["method"], "hello");
                    json!({"type":"hello", "capabilities":[]})
                }
                1 => {
                    assert_eq!(request["method"], "open_session");
                    json!({
                        "type":"session_opened", "session_id":"fixture-session",
                        "window_capability":"PRIVATE_CAPABILITY",
                        "target":{"process_id":42,"window_handle":7,"window_title":"PRIVATE_TITLE"}
                    })
                }
                2 => {
                    assert_eq!(request["method"], "snapshot");
                    assert_eq!(request["params"]["session_id"], "fixture-session");
                    assert_eq!(request["params"]["task_grant_id"], "fixture-grant");
                    assert_eq!(request["params"]["window_capability"], "PRIVATE_CAPABILITY");
                    json!({
                        "type":"error", "code":"invalid_target", "message":"original refusal",
                        "details":{"capture":capture, "PRIVATE_DETAILS":"PRIVATE_DETAIL_VALUE"},
                        "task_context":{"target":{"process_id":999,"window_handle":888}},
                        "PRIVATE_RESPONSE":"PRIVATE_RESPONSE_VALUE"
                    })
                }
                _ => unreachable!(),
            };
            response["request_id"] = request["request_id"].clone();
            dcc_cua_protocol::write_frame(
                &mut host_stream,
                response.to_string().as_bytes(),
                dcc_cua_protocol::MAX_JSON_FRAME_BYTES,
            )
            .await
            .unwrap();
            host_stream.flush().await.unwrap();
        }
    });
    let mut client = HostClient::from_stream(client_stream);
    client.hello("pure-capture-error-fixture").await.unwrap();
    let session = client
        .open_logical_task_session(
            "fixture-session",
            json!({"task_grant_id":"fixture-grant"}),
            DEFAULT_IDLE_TIMEOUT_MS,
        )
        .await
        .unwrap();
    let mut server = test_server();
    let mut task = pixels_task();
    task["allowed_methods"] = json!(["snapshot"]);
    task["allowed_actions"] = json!([]);
    let prepared = server.prepare_task(task).unwrap();
    let task_id = prepared["task_id"].as_str().unwrap().to_owned();
    server.proposals.get_mut(&task_id).unwrap().session = Some(session);
    let result = server
        .call_tool(json!({
            "name":"dcc_cua_task_call",
            "arguments":{"task_id":task_id,"method":"snapshot","params":{}}
        }))
        .await
        .unwrap();
    host_task.await.unwrap();
    assert_eq!(result["isError"], true);
    let payload = &result["structuredContent"];
    assert_eq!(payload["ok"], false);
    assert_eq!(
        payload["error"],
        "host returned invalid_target: original refusal"
    );
    assert_eq!(payload["task_context"]["provider"], "dcc-cua");
    assert_eq!(
        payload["task_context"]["runtime_version"],
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(payload["task_context"]["task_id"], task_id);
    assert_eq!(
        payload["task_context"]["target"],
        json!({"process_id":42,"window_handle":7})
    );
    if variant == "valid" {
        let expected: dcc_cua_core::ComputerUseCaptureDiagnostic =
            serde_json::from_value(root_bounds_capture_fixture()).unwrap();
        assert_eq!(
            payload["details"]["capture"],
            serde_json::to_value(expected).unwrap()
        );
    } else {
        assert!(payload.get("details").is_none());
    }
    assert!(!result.to_string().contains("PRIVATE_"));
    assert!(payload.get("observation_id").is_none());
    assert!(payload.get("image").is_none());
    assert!(payload.get("window_capability").is_none());
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(&text, payload);
}

#[rstest]
#[case(TaskSurface::Window)]
#[case(TaskSurface::Browser)]
fn exact_window_restore_is_available_in_both_task_surfaces(#[case] surface: TaskSurface) {
    assert!(method_allowed(surface, "change_window_state"));
    validate_allowed_methods(surface, &["change_window_state".into()]).unwrap();

    let start = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "start_task")
        .unwrap();
    let methods = start["inputSchema"]["properties"]["allowed_methods"]["items"]["enum"]
        .as_array()
        .unwrap();
    assert!(methods.iter().any(|method| method == "change_window_state"));
}

#[rstest]
#[case(json!({"operation": "activate"}), true)]
#[case(json!({"operation": "restore_activate"}), true)]
#[case(json!({"operation": "close"}), false)]
#[case(json!({"operation": "minimize"}), false)]
#[case(json!({"operation": "RESTORE_ACTIVATE"}), false)]
#[case(json!({"operation": 1}), false)]
#[case(json!({}), false)]
fn task_window_state_operations_remain_bounded(#[case] params: Value, #[case] allowed: bool) {
    assert_eq!(
        validate_task_method_params("change_window_state", &params).is_ok(),
        allowed
    );
}

fn pixels_task() -> Value {
    json!({
        "application_label":"Exact DCC pixels", "target_process_id":42,
        "target_window_handle":7, "surface":"window", "observation_mode":"pixels_only",
        "allowed_methods":["snapshot", "minimize_window"],
        "allowed_actions":[{"action":"minimize_window", "input_kind":"window_state",
            "secret_input":false, "authorization_category":"window_state"}]
    })
}

fn native_frame_task() -> Value {
    let mut task = pixels_task();
    task["allowed_methods"] = json!(["get_window_state", "set_window_frame"]);
    task["allowed_actions"] = json!([{"action":"set_window_frame","input_kind":"window_state","secret_input":false,"authorization_category":"window_state"}]);
    task
}

fn native_frame_params() -> Value {
    json!({"window_state_id":"native-state-1","frame":{"x":50,"y":800,"width":926,"height":680}})
}

#[rstest]
fn native_frame_public_task_derives_only_closed_window_state_authority() {
    let mut server = test_server();
    let prepared = server.prepare_task(native_frame_task()).unwrap();
    let proposal = &server.proposals[prepared["task_id"].as_str().unwrap()];
    let grant = task_session_grant(proposal, proposal.receipt.as_ref().unwrap());
    assert_eq!(grant["allow_raw_input"], false);
    assert_eq!(grant["allow_recording"], false);
    assert_eq!(grant["allow_browser_input"], false);
    assert!(proposal.registration.allowed_actions[0].is_window_frame());
    assert_eq!(
        proposal.registration.allowed_host_methods,
        vec!["get_window_state", "set_window_frame"]
    );
    for (field, value) in [
        ("observation_mode", json!("semantic")),
        ("surface", json!("browser")),
        ("allowed_actions", json!([])),
        ("allowed_methods", json!(["set_window_frame"])),
    ] {
        let mut invalid = native_frame_task();
        invalid[field] = value;
        assert!(test_server().prepare_task(invalid).is_err(), "{field}");
    }
    for (field, value) in [
        ("input_kind", json!("raw_input")),
        ("secret_input", json!(true)),
        ("authorization_category", json!("credential")),
        ("browser_origin", json!("https://example.com")),
    ] {
        let mut invalid = native_frame_task();
        invalid["allowed_actions"][0][field] = value;
        assert!(test_server().prepare_task(invalid).is_err(), "{field}");
    }
}

#[rstest]
fn native_frame_public_parameters_require_closed_physical_i32_extents() {
    assert!(validate_task_method_params("set_window_frame", &native_frame_params()).is_ok());
    for change in 0..13 {
        let mut invalid = native_frame_params();
        match change {
            0 => invalid["window_state_id"] = json!(""),
            1 => invalid["window_state_id"] = json!(7),
            2 => invalid["window_state_id"] = json!(" id "),
            3 => invalid["window_state_id"] = json!("x".repeat(129)),
            4 => invalid["observation_id"] = json!("snapshot-1"),
            5 => invalid["frame"]["x"] = json!(50.5),
            6 => invalid["frame"]["x"] = json!(2147483647_i64),
            7 => invalid["frame"]["y"] = json!(2147483647_i64),
            8 => invalid["frame"]["width"] = json!(0),
            9 => invalid["frame"]["height"] = json!(-1),
            10 => invalid["frame"]["activate"] = json!(true),
            11 => invalid["frame"]["x"] = json!(-2147483649_i64),
            _ => {
                invalid["frame"].as_object_mut().unwrap().remove("height");
            }
        }
        assert!(
            validate_task_method_params("set_window_frame", &invalid).is_err(),
            "change {change}"
        );
    }
}

#[rstest]
#[tokio::test]
async fn native_frame_public_task_retains_start_expiry_stop_and_scope_fences() {
    let mut server = test_server();
    let prepared = server.prepare_task(native_frame_task()).unwrap();
    let id = prepared["task_id"].as_str().unwrap().to_owned();
    let args = |task_id: &str| json!({"task_id":task_id,"method":"set_window_frame","params":native_frame_params()});
    assert!(
        server
            .task_call(args("foreign-task"))
            .await
            .unwrap_err()
            .contains("not found")
    );
    assert!(
        server
            .task_call(args(&id))
            .await
            .unwrap_err()
            .contains("call start_task")
    );
    server.proposals.get_mut(&id).unwrap().allowed_methods = vec!["get_window_state".into()];
    assert!(
        server
            .task_call(args(&id))
            .await
            .unwrap_err()
            .contains("configured task method scope")
    );
    server
        .proposals
        .get_mut(&id)
        .unwrap()
        .registration
        .expires_at_unix_ms = 0;
    assert!(
        server
            .task_call(args(&id))
            .await
            .unwrap_err()
            .contains("expired")
    );
    server.proposals.get_mut(&id).unwrap().revoked = true;
    assert!(
        server
            .task_call(args(&id))
            .await
            .unwrap_err()
            .contains("stopped")
    );
}

#[rstest]
fn native_frame_public_schema_advertises_distinct_metadata_token_and_exact_frame() {
    let tools = tool_definitions();
    let start = tools
        .iter()
        .find(|tool| tool["name"] == "start_task")
        .unwrap();
    let conditional = start["inputSchema"]["allOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| {
            entry["if"]["properties"]["allowed_methods"]["contains"]["const"] == "set_window_frame"
        })
        .unwrap();
    assert_eq!(
        conditional["then"]["properties"]["observation_mode"]["const"],
        "pixels_only"
    );
    assert_eq!(
        conditional["then"]["properties"]["allowed_actions"]["contains"]["properties"]["action"]["const"],
        "set_window_frame"
    );
    let call = tools
        .iter()
        .find(|tool| tool["name"] == "dcc_cua_task_call")
        .unwrap();
    let frame = call["inputSchema"]["allOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["if"]["properties"]["method"]["const"] == "set_window_frame")
        .unwrap();
    let params = &frame["then"]["properties"]["params"];
    assert_eq!(params["required"], json!(["window_state_id", "frame"]));
    assert_eq!(params["additionalProperties"], false);
    assert_eq!(
        params["properties"]["frame"]["properties"]["x"]["type"],
        "integer"
    );
    assert!(!method_allowed(TaskSurface::Browser, "set_window_frame"));
}

#[rstest]
fn native_frame_failure_projection_is_finite_coherent_and_method_specific() {
    let mut details = json!({"phase":"local_mutation_dispatch","action_attempted":true,"input_sent":"not_sent",
        "completion":"unknown","effect_unknown":true,"automatic_input":false,"blind_retry":false,"fresh_observation_required":true,
        "PRIVATE_PATH":"PRIVATE_VALUE","window_state_id":"PRIVATE_TOKEN"});
    let projected = native_frame_failure_projection(&details).unwrap();
    assert_eq!(projected.as_object().unwrap().len(), 8);
    assert!(!projected.to_string().contains("PRIVATE"));
    details["completion"] = json!("known");
    assert!(native_frame_failure_projection(&details).is_none());
    details["phase"] = json!("PRIVATE_PHASE");
    assert!(native_frame_failure_projection(&details).is_none());
}

#[rstest]
#[tokio::test]
async fn native_frame_actual_public_error_adapter_preserves_only_typed_completion() {
    for attempted in [false, true] {
        let (client_stream, mut host_stream) = tokio::io::duplex(32 * 1024);
        let worker = tokio::spawn(async move {
            for step in 0..3 {
                let bytes = dcc_cua_protocol::read_frame(
                    &mut host_stream,
                    dcc_cua_protocol::MAX_JSON_FRAME_BYTES,
                )
                .await
                .unwrap()
                .unwrap();
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                let mut response = match step {
                    0 => json!({"type":"hello","capabilities":[]}),
                    1 => {
                        json!({"type":"session_opened","session_id":"fixture-session","window_capability":"PRIVATE_CAPABILITY","target":{"process_id":42,"window_handle":7}})
                    }
                    _ => {
                        assert_eq!(request["method"], "set_window_frame");
                        assert_eq!(request["params"]["window_state_id"], "native-state-1");
                        assert_eq!(request["params"]["frame"], native_frame_params()["frame"]);
                        assert!(request["params"].get("observation_id").is_none());
                        json!({"type":"error","code":"invalid_target","message":"frame refused",
                            "details":{"phase":if attempted{"local_mutation_dispatch"}else{"pre_dispatch"},
                                "action_attempted":attempted,"input_sent":"not_sent",
                                "completion":if attempted{"unknown"}else{"known"},"effect_unknown":attempted,
                                "automatic_input":false,"blind_retry":false,"fresh_observation_required":true,
                                "PRIVATE_PATH":"PRIVATE_VALUE","window_state_id":"PRIVATE_TOKEN"}})
                    }
                };
                response["request_id"] = request["request_id"].clone();
                dcc_cua_protocol::write_frame(
                    &mut host_stream,
                    &serde_json::to_vec(&response).unwrap(),
                    dcc_cua_protocol::MAX_JSON_FRAME_BYTES,
                )
                .await
                .unwrap();
                host_stream.flush().await.unwrap();
            }
        });
        let mut client = HostClient::from_stream(client_stream);
        client.hello("pure-frame-contract").await.unwrap();
        let session = client
            .open_logical_task_session(
                "fixture-session",
                json!({"task_grant_id":"fixture-grant"}),
                DEFAULT_IDLE_TIMEOUT_MS,
            )
            .await
            .unwrap();
        let mut server = test_server();
        let prepared = server.prepare_task(native_frame_task()).unwrap();
        let id = prepared["task_id"].as_str().unwrap().to_owned();
        server.proposals.get_mut(&id).unwrap().session = Some(session);
        let result = server
            .task_call(
                json!({"task_id":id,"method":"set_window_frame","params":native_frame_params()}),
            )
            .await
            .unwrap();
        worker.await.unwrap();
        assert_eq!(result["isError"], true);
        assert_eq!(
            result["structuredContent"]["details"]["action_attempted"],
            attempted
        );
        assert_eq!(
            result["structuredContent"]["details"]["effect_unknown"],
            attempted
        );
        assert_eq!(
            result["structuredContent"]["details"]
                .as_object()
                .unwrap()
                .len(),
            8
        );
        assert_eq!(
            result["structuredContent"]["task_context"]["target"],
            json!({"process_id":42,"window_handle":7})
        );
        assert!(!result.to_string().contains("PRIVATE"));
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, result["structuredContent"]);
    }
}

#[rstest]
fn explicit_pixel_mode_survives_public_proposal_and_host_grant() {
    let mut server = test_server();
    let prepared = server.prepare_task(pixels_task()).unwrap();
    assert_eq!(prepared["observation_mode"], "pixels_only");
    let proposal = server
        .proposals
        .get(prepared["task_id"].as_str().unwrap())
        .unwrap();
    let grant = task_session_grant(proposal, proposal.receipt.as_ref().unwrap());
    assert_eq!(grant["observation_mode"], "pixels_only");
    assert_eq!(grant["process_id"], 42);
    assert_eq!(grant["window_handle"], 7);
    assert_eq!(grant["allow_browser_input"], false);
    assert_eq!(grant["allow_raw_input"], false);
    assert_eq!(
        proposal.registration.allowed_host_methods,
        vec!["snapshot", "minimize_window"]
    );
    let semantic = server.prepare_task(browser_task()).unwrap();
    assert_eq!(semantic["observation_mode"], "semantic");
}

#[rstest]
#[case("observation_mode", json!("fallback"))]
#[case("surface", json!("browser"))]
#[case("target_window_handle", Value::Null)]
#[case("target_process_id", json!(0))]
#[case("allowed_methods", json!(["accessibility_snapshot"]))]
#[case("allowed_methods", json!(["find"]))]
#[case("allowed_methods", json!(["execute_action"]))]
#[case("allowed_actions", json!([{"action":"drag", "input_kind":"raw_input", "secret_input":false, "authorization_category":"raw_input"}]))]
#[case("allowed_actions", json!([{"action":"click", "input_kind":"semantic", "secret_input":false, "authorization_category":"content_change"}]))]
fn public_pixels_mode_refuses_ambiguous_or_semantic_grants(
    #[case] field: &str,
    #[case] value: Value,
) {
    let mut task = pixels_task();
    task[field] = value;
    assert!(test_server().prepare_task(task).is_err());
}

#[rstest]
#[case("click")]
#[case("double_click")]
#[case("right_click")]
#[case("toggle")]
#[case("keypress")]
#[case("keyboard_shortcut")]
#[case("type")]
#[case("type_chars")]
fn pixel_raw_input_permission_is_derived_from_a_real_supported_scope(#[case] action: &str) {
    let mut server = test_server();
    let mut task = pixels_task();
    task["allowed_methods"] = json!(["snapshot", "execute_action"]);
    task["allowed_actions"] = json!([{"action":action, "input_kind":"raw_input", "secret_input":false, "authorization_category":"raw_input"}]);
    let prepared = server.prepare_task(task).unwrap();
    let proposal = server
        .proposals
        .get(prepared["task_id"].as_str().unwrap())
        .unwrap();
    let grant = task_session_grant(proposal, proposal.receipt.as_ref().unwrap());
    assert_eq!(grant["allow_raw_input"], true);
    assert_eq!(grant["allow_browser_input"], false);
    assert_eq!(proposal.registration.allowed_actions[0].action, action);
}

#[rstest]
#[case("action", json!("drag"))]
#[case("action", json!("scroll"))]
#[case("action", json!("move"))]
#[case("action", json!("press"))]
#[case("input_kind", json!("semantic"))]
#[case("secret_input", json!(true))]
#[case("authorization_category", json!("credential"))]
#[case("browser_origin", json!("https://example.com"))]
fn pixel_task_refuses_uncovered_or_sensitive_raw_scopes(#[case] field: &str, #[case] value: Value) {
    let mut task = pixels_task();
    task["allowed_methods"] = json!(["snapshot", "execute_action"]);
    task["allowed_actions"] = json!([{"action":"click", "input_kind":"raw_input", "secret_input":false, "authorization_category":"raw_input"}]);
    task["allowed_actions"][0][field] = value;
    assert!(test_server().prepare_task(task).is_err());
}

#[rstest]
#[case(json!({}), false)]
#[case(json!({"observation_id":""}), false)]
#[case(json!({"observation_id":7}), false)]
#[case(json!({"observation_id":"snapshot-1"}), true)]
fn public_minimize_requires_observation_id(#[case] params: Value, #[case] allowed: bool) {
    assert_eq!(
        validate_task_method_params("minimize_window", &params).is_ok(),
        allowed
    );
    assert!(method_allowed(TaskSurface::Window, "minimize_window"));
}

#[rstest]
#[tokio::test]
async fn minimize_cannot_escape_declared_method_scope_or_start_fence() {
    let mut server = test_server();
    let mut task = pixels_task();
    task["allowed_methods"] = json!(["snapshot"]);
    let prepared = server.prepare_task(task).unwrap();
    let error = server.task_call(json!({"task_id":prepared["task_id"], "method":"minimize_window", "params":{"observation_id":"obs-1"}})).await.unwrap_err();
    assert!(error.contains("configured task method scope"), "{error}");
    let prepared = server.prepare_task(pixels_task()).unwrap();
    let error = server.task_call(json!({"task_id":prepared["task_id"], "method":"minimize_window", "params":{"observation_id":"obs-1"}})).await.unwrap_err();
    assert!(error.contains("call start_task"), "{error}");
}

#[rstest]
fn public_tool_schema_advertises_closed_pixel_actions_and_exact_window_only() {
    let tools = tool_definitions();
    let start = tools
        .iter()
        .find(|tool| tool["name"] == "start_task")
        .unwrap();
    assert_eq!(
        start["inputSchema"]["properties"]["observation_mode"]["default"],
        "semantic"
    );
    assert!(
        start["inputSchema"]["properties"]["observation_mode"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("pixels_only"))
    );
    assert!(
        start["inputSchema"]["properties"]["allowed_methods"]["items"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("minimize_window"))
    );
    let pixel_condition = &start["inputSchema"]["allOf"][0];
    assert_eq!(
        pixel_condition["if"]["properties"]["observation_mode"]["const"],
        "pixels_only"
    );
    let pixel = &pixel_condition["then"];
    assert_eq!(
        pixel["required"],
        json!(["target_process_id", "target_window_handle"])
    );
    assert_eq!(pixel["properties"]["surface"]["const"], "window");
    assert_eq!(
        pixel["properties"]["allowed_browser_origins"]["maxItems"],
        0
    );
    let methods = pixel["properties"]["allowed_methods"]["items"]["enum"]
        .as_array()
        .unwrap();
    assert!(methods.contains(&json!("execute_action")));
    assert!(!methods.contains(&json!("accessibility_snapshot")));
    let scopes = pixel["properties"]["allowed_actions"]["items"]["oneOf"]
        .as_array()
        .unwrap();
    assert_eq!(scopes.len(), 3);
    let input = &scopes[2]["properties"];
    assert_eq!(
        input["action"]["enum"],
        json!([
            "click",
            "double_click",
            "right_click",
            "toggle",
            "keypress",
            "keyboard_shortcut",
            "type",
            "type_chars"
        ])
    );
    assert_eq!(input["input_kind"]["const"], "raw_input");
    assert_eq!(input["secret_input"]["const"], false);
    assert_eq!(input["authorization_category"]["const"], "raw_input");
}

#[rstest]
#[tokio::test]
async fn task_call_rejects_window_close_before_host_dispatch() {
    let error = test_server()
        .task_call(json!({
            "task_id": "unstarted-task",
            "method": "change_window_state",
            "params": {"operation": "close"}
        }))
        .await
        .unwrap_err();
    assert!(error.contains("activate or restore_activate"), "{error}");
}

#[rstest]
#[tokio::test]
async fn restore_keeps_the_declared_method_scope_and_session_start_fence() {
    let mut server = test_server();
    let prepared = server.prepare_task(browser_task()).unwrap();
    let error = server
        .task_call(json!({
            "task_id": prepared["task_id"],
            "method": "change_window_state",
            "params": {"operation": "restore_activate"}
        }))
        .await
        .unwrap_err();
    assert!(error.contains("configured task method scope"), "{error}");

    let mut task = browser_task();
    task["allowed_methods"] = json!(["change_window_state"]);
    let prepared = server.prepare_task(task).unwrap();
    let task_id = prepared["task_id"].as_str().unwrap();
    let proposal = server.proposals.get(task_id).unwrap();
    assert_eq!(
        proposal.registration.allowed_host_methods,
        vec!["change_window_state"]
    );
    let grant = task_session_grant(proposal, proposal.receipt.as_ref().unwrap());
    assert_eq!(grant["process_id"], 42);
    assert_eq!(grant["window_handle"], 7);
    let error = server
        .task_call(json!({
            "task_id": task_id,
            "method": "change_window_state",
            "params": {"operation": "restore_activate"}
        }))
        .await
        .unwrap_err();
    assert!(error.contains("call start_task"), "{error}");
}

fn browser_task() -> Value {
    json!({
        "application_label": "Chrome Web Store credentials",
        "target_process_id": 42,
        "target_window_handle": 7,
        "surface": "browser",
        "allowed_methods": ["browser_snapshot", "browser_type"],
        "allowed_actions": [{
            "action": "browser_type",
            "input_kind": "browser",
            "secret_input": true,
            "authorization_category": "credential",
            "browser_origin": "https://chromewebstore.google.com"
        }],
        "ttl_minutes": 15
    })
}

#[rstest]
fn connected_agent_host_creates_a_task_lease_without_secondary_confirmation() {
    let mut server = test_server();

    let created = server.prepare_task(browser_task()).unwrap();
    let task_id = created["task_id"].as_str().unwrap();
    let task = server.proposals.get(task_id).unwrap();

    assert_eq!(created["status"], "ready");
    assert!(task.receipt.is_some());
    assert!(created.get("confirmation_method").is_none());
}

#[rstest]
#[tokio::test]
async fn mcp_surface_has_no_secondary_authorization_ui_or_tools() {
    let mut server = test_server();
    let tools = tool_definitions();
    let tool_names = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();

    assert!(tool_names.contains(&"start_task"));
    assert!(tool_names.contains(&"task_status"));
    assert!(tool_names.contains(&"stop_task"));
    assert!(tool_names.contains(&"dcc_cua_task_call"));
    assert!(
        tool_names
            .iter()
            .all(|name| !name.contains("authorization") && *name != "authorize_task")
    );

    let resources = server
        .handle_rpc(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "resources/list",
            "params": {}
        }))
        .await
        .unwrap();
    assert_eq!(
        resources["result"]["resources"][0]["uri"],
        CONNECTION_RESOURCE
    );
}

fn owned_browser_task() -> Value {
    json!({
        "application_label": "Firefox add-on upload",
        "owned_browser_launch": {
            "browser": "chromium",
            "profile": "isolated_new"
        },
        "surface": "browser",
        "allowed_methods": ["browser_snapshot", "browser_navigate", "browser_set_input_files"],
        "allowed_actions": [{
            "action": "click",
            "input_kind": "semantic",
            "secret_input": false,
            "authorization_category": "publishing"
        }],
        "allowed_browser_origins": ["https://addons.mozilla.org"],
        "ttl_minutes": 15
    })
}

fn driver_authorization_request(public_session: &str) -> DriverAuthorizationRequest {
    DriverAuthorizationRequest {
        schema: "cua-driver-authorization-request-v1".into(),
        nonce: "nonce-1".into(),
        generation: 1,
        daemon_instance: "daemon-1".into(),
        permission_mode: "standard".into(),
        managed_policy_sha256: None,
        user_policy_sha256: None,
        adapter_id: "browser_prepare.existing_profile".into(),
        risk_class: "r2".into(),
        public_session: public_session.into(),
        transport_session: "transport-1".into(),
        resource_json: "{}".into(),
        human_summary: "Attach to the authorized browser profile".into(),
        expires_unix_ms: u64::MAX,
        request_digest: "digest-1".into(),
    }
}

#[rstest]
fn task_calls_cannot_mint_or_widen_the_internal_runtime_lease() {
    let tools = tool_definitions();
    let task_call = tools
        .iter()
        .find(|tool| tool["name"] == "dcc_cua_task_call")
        .unwrap();
    assert!(
        task_call["inputSchema"]["properties"]
            .get("authorization_id")
            .is_none()
    );
    assert!(
        task_call["inputSchema"]["properties"]
            .get("window_capability")
            .is_none()
    );
}

#[rstest]
fn owned_browser_task_exposes_only_a_closed_launch_spec_until_start() {
    let mut server = test_server();
    let prepared = server.prepare_task(owned_browser_task()).unwrap();

    assert_eq!(prepared["target"]["kind"], "owned_browser");
    assert_eq!(prepared["target"]["browser"], "chromium");
    assert_eq!(prepared["target"]["profile"], "isolated_new");
    assert!(prepared["target"]["process_id"].is_null());
    assert!(prepared["target"]["window_handle"].is_null());

    let proposal_id = prepared["task_id"].as_str().unwrap();
    let proposal = server.proposals.get(proposal_id).unwrap();
    let grant = task_session_grant(proposal, proposal.receipt.as_ref().unwrap());
    assert!(grant["process_id"].is_null());
    assert!(grant["window_handle"].is_null());
    assert_eq!(grant["allow_browser_prepare"], false);
    assert_eq!(
        grant["allowed_browser_origins"],
        json!(["https://addons.mozilla.org"])
    );
}

#[rstest]
fn owned_browser_proposal_rejects_target_substitution_and_prepare_reentry() {
    let mut server = test_server();
    let mut substituted = owned_browser_task();
    substituted["target_process_id"] = json!(42);
    substituted["target_window_handle"] = json!(7);
    assert!(server.prepare_task(substituted).is_err());

    let mut reentry = owned_browser_task();
    reentry["allowed_methods"] = json!(["browser_prepare"]);
    assert!(
        server
            .prepare_task(reentry)
            .unwrap_err()
            .contains("cannot grant browser_prepare")
    );
}

#[rstest]
#[tokio::test]
async fn task_call_requires_start_attestation_before_observation() {
    let mut server = test_server();
    let prepared = server.prepare_task(browser_task()).unwrap();
    let proposal_id = prepared["task_id"].as_str().unwrap();

    let error = server
        .task_call(json!({
            "task_id": proposal_id,
            "method": "browser_snapshot",
            "params": {}
        }))
        .await
        .unwrap_err();

    assert!(error.contains("provider/runtime/PID/HWND"));
}

#[rstest]
fn clipboard_capture_grants_read_and_clear_as_one_authorized_capability() {
    let mut server = test_server();
    let mut task = browser_task();
    task["allowed_methods"] = json!(["clipboard_capture_secret"]);
    task["allowed_actions"] = json!([{
        "action": "clipboard_capture_secret",
        "input_kind": "clipboard",
        "secret_input": true,
        "authorization_category": "credential"
    }]);
    let prepared = server.prepare_task(task).unwrap();
    let proposal_id = prepared["task_id"].as_str().unwrap();
    let proposal = server.proposals.get(proposal_id).unwrap();
    let receipt = proposal.receipt.as_ref().unwrap();

    let grant = task_session_grant(proposal, receipt);

    assert_eq!(grant["allow_clipboard_read"], true);
    assert_eq!(grant["allow_clipboard_write"], true);
}

#[rstest]
fn browser_prepare_is_granted_only_when_the_task_includes_the_method() {
    let mut server = test_server();
    let prepared = server.prepare_task(browser_task()).unwrap();
    let proposal_id = prepared["task_id"].as_str().unwrap();
    let proposal = server.proposals.get(proposal_id).unwrap();
    let receipt = proposal.receipt.as_ref().unwrap();

    let grant = task_session_grant(proposal, receipt);

    assert_eq!(grant["allow_browser_prepare"], false);
}

#[rstest]
fn browser_download_is_closed_to_browser_tasks_and_granted_only_when_requested() {
    assert!(method_allowed(TaskSurface::Browser, "browser_download"));
    assert!(!method_allowed(TaskSurface::Window, "browser_download"));

    let start = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "start_task")
        .unwrap();
    let methods = start["inputSchema"]["properties"]["allowed_methods"]["items"]["enum"]
        .as_array()
        .unwrap();
    assert!(methods.iter().any(|method| method == "browser_download"));

    let mut server = test_server();
    let mut task = browser_task();
    task["allowed_methods"] = json!(["browser_snapshot", "browser_download"]);
    let prepared = server.prepare_task(task).unwrap();
    let proposal_id = prepared["task_id"].as_str().unwrap();
    let proposal = server.proposals.get(proposal_id).unwrap();
    let grant = task_session_grant(proposal, proposal.receipt.as_ref().unwrap());

    assert_eq!(grant["allow_browser_download"], true);

    let prepared_without_download = server.prepare_task(browser_task()).unwrap();
    let proposal_id = prepared_without_download["task_id"].as_str().unwrap();
    let proposal = server.proposals.get(proposal_id).unwrap();
    let grant = task_session_grant(proposal, proposal.receipt.as_ref().unwrap());

    assert_eq!(grant["allow_browser_download"], false);
}

#[rstest]
#[tokio::test]
async fn browser_prepare_accepts_the_exact_logical_task_session() {
    let mut server = test_server();
    let mut task = browser_task();
    task["allowed_methods"] = json!(["browser_snapshot", "browser_prepare"]);
    let prepared = server.prepare_task(task).unwrap();
    let proposal_id = prepared["task_id"].as_str().unwrap();
    let proposal = server.proposals.get(proposal_id).unwrap();
    let host = browser_prepare_authorization_host(proposal_id, proposal).unwrap();
    let public_session = task_session_id(proposal_id);

    let decision = host
        .authorize(driver_authorization_request(&public_session))
        .await
        .unwrap();

    assert_eq!(decision.action, DriverAuthorizationAction::Allow);
    assert_eq!(decision.request_digest, "digest-1");
}

#[rstest]
#[tokio::test]
async fn authorized_browser_prepare_accepts_the_exact_namespaced_logical_task_session() {
    let host = TaskBrowserPrepareAuthorizationHost::new("mcp-task-exact");
    let namespaced = "__cua_runtime_00112233445566778899aabbccddeeff:mcp-task-exact";

    let decision = host
        .authorize(driver_authorization_request(namespaced))
        .await
        .unwrap();

    assert_eq!(decision.action, DriverAuthorizationAction::Allow);
    assert_eq!(decision.request_digest, "digest-1");
}

#[rstest]
#[tokio::test]
async fn authorized_browser_prepare_accepts_and_binds_the_real_runtime_window_session() {
    let host = TaskBrowserPrepareAuthorizationHost::new("mcp-task-exact");
    let runtime_session = concat!(
        "__cua_runtime_00112233445566778899aabbccddeeff:",
        "dcc-cua-window-123e4567-e89b-42d3-a456-426614174000"
    );
    let mut first = driver_authorization_request(runtime_session);
    first.transport_session = "transport-exact".into();

    let decision = host.authorize(first).await.unwrap();

    assert_eq!(decision.action, DriverAuthorizationAction::Allow);
    let mut repeated = driver_authorization_request(runtime_session);
    repeated.transport_session = "transport-exact".into();
    assert_eq!(
        host.authorize(repeated).await.unwrap().action,
        DriverAuthorizationAction::Allow
    );

    let mut different_public = driver_authorization_request(concat!(
        "__cua_runtime_00112233445566778899aabbccddeeff:",
        "dcc-cua-window-223e4567-e89b-42d3-a456-426614174000"
    ));
    different_public.transport_session = "transport-exact".into();
    assert_eq!(
        host.authorize(different_public).await.unwrap().action,
        DriverAuthorizationAction::Deny
    );

    let mut different_transport = driver_authorization_request(runtime_session);
    different_transport.transport_session = "transport-other".into();
    assert_eq!(
        host.authorize(different_transport).await.unwrap().action,
        DriverAuthorizationAction::Deny
    );
}

#[rstest]
#[tokio::test]
async fn authorized_browser_prepare_rejects_a_different_logical_task_session() {
    let host = TaskBrowserPrepareAuthorizationHost::new("mcp-task-exact");

    let decision = host
        .authorize(driver_authorization_request("mcp-task-other"))
        .await
        .unwrap();

    assert_eq!(decision.action, DriverAuthorizationAction::Deny);
    assert_eq!(decision.request_digest, "digest-1");
}

#[rstest]
#[tokio::test]
async fn authorized_browser_prepare_rejects_namespaced_session_lookalikes() {
    let host = TaskBrowserPrepareAuthorizationHost::new("mcp-task-exact");
    for observed_session in [
        "__cua_runtime_00112233445566778899aabbccddee:mcp-task-exact",
        "__cua_runtime_00112233445566778899AABBCCDDEEFF:mcp-task-exact",
        "__cua_runtime_00112233445566778899aabbccddeeff:mcp-task-other",
        "__cua_runtime_00112233445566778899aabbccddeeff:other:mcp-task-exact",
        "__cua_runtime_00112233445566778899aabbccddeeff:dcc-cua-window-not-a-uuid",
        "__cua_runtime_00112233445566778899aabbccddeeff:dcc-cua-window-123e4567-e89b-12d3-a456-426614174000",
        "dcc-cua-window-123e4567-e89b-42d3-a456-426614174000",
        "prefix:mcp-task-exact",
    ] {
        let decision = host
            .authorize(driver_authorization_request(observed_session))
            .await
            .unwrap();

        assert_eq!(decision.action, DriverAuthorizationAction::Deny);
        assert_eq!(decision.request_digest, "digest-1");
    }
}

#[rstest]
#[tokio::test]
async fn authorized_browser_prepare_rejects_every_non_exact_driver_request() {
    let host = TaskBrowserPrepareAuthorizationHost::new("mcp-task-exact");
    let mut requests = Vec::new();
    let mut wrong_schema = driver_authorization_request("mcp-task-exact");
    wrong_schema.schema = "cua-driver-authorization-request-v2".into();
    requests.push(wrong_schema);
    let mut wrong_mode = driver_authorization_request("mcp-task-exact");
    wrong_mode.permission_mode = "unrestricted".into();
    requests.push(wrong_mode);
    let mut wrong_adapter = driver_authorization_request("mcp-task-exact");
    wrong_adapter.adapter_id = "browser_prepare.isolated_profile".into();
    requests.push(wrong_adapter);
    let mut wrong_risk = driver_authorization_request("mcp-task-exact");
    wrong_risk.risk_class = "r1".into();
    requests.push(wrong_risk);

    for request in requests {
        let decision = host.authorize(request).await.unwrap();
        assert_eq!(decision.action, DriverAuthorizationAction::Deny);
        assert_eq!(decision.request_digest, "digest-1");
    }
}

#[rstest]
fn browser_prepare_driver_authorization_requires_the_declared_task_method() {
    let mut server = test_server();
    let mut task = browser_task();
    task["allowed_methods"] = json!(["browser_snapshot", "browser_prepare"]);
    let prepared = server.prepare_task(task).unwrap();
    let proposal_id = prepared["task_id"].as_str().unwrap();
    let allowed = server.proposals.get(proposal_id).unwrap();
    assert!(browser_prepare_authorization_host(proposal_id, allowed).is_some());

    let prepared = server.prepare_task(browser_task()).unwrap();
    let proposal_id = prepared["task_id"].as_str().unwrap();
    let method_omitted = server.proposals.get(proposal_id).unwrap();
    assert!(browser_prepare_authorization_host(proposal_id, method_omitted).is_none());
}

#[rstest]
fn start_task_has_no_confirmation_ui_or_secret_value_fields() {
    let start = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "start_task")
        .unwrap();
    let properties = start["inputSchema"]["properties"].as_object().unwrap();
    for forbidden in ["text", "secret", "password", "token", "credential"] {
        assert!(!properties.contains_key(forbidden));
    }
    assert!(start.get("_meta").is_none());
}

#[rstest]
fn task_action_schema_matches_the_closed_runtime_contract() {
    let schema = task_action_scope_schema();
    let variants = schema["oneOf"].as_array().unwrap();
    assert_eq!(variants.len(), 6);

    let variant = |input_kind: &str| {
        variants
            .iter()
            .find(|variant| {
                variant["properties"]["input_kind"]["const"].as_str() == Some(input_kind)
            })
            .unwrap()
    };
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
        json!(TrustedTaskActionScope::NATIVE_ACTIONS)
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

    let browser = variant("browser");
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
fn method_style_action_name_is_rejected_before_a_task_is_created() {
    let mut server = test_server();
    let mut task = browser_task();
    task["allowed_actions"][0]["action"] = json!("browser_click");

    assert!(server.prepare_task(task).is_err());
    assert!(server.proposals.is_empty());
}

#[rstest]
#[tokio::test]
async fn task_call_scope_rejects_methods_not_declared_by_the_agent() {
    let mut server = test_server();
    let prepared = server.prepare_task(browser_task()).unwrap();
    let proposal_id = prepared["task_id"].as_str().unwrap();
    let error = server
        .task_call(json!({
            "task_id": proposal_id,
            "method": "browser_navigate",
            "params": {}
        }))
        .await
        .unwrap_err();
    assert!(error.contains("configured task method scope"));
}

#[rstest]
fn task_request_rejects_surface_mismatches_and_duplicate_methods() {
    let mut server = test_server();
    let mut mismatch = browser_task();
    mismatch["surface"] = json!("window");
    assert!(
        server
            .prepare_task(mismatch)
            .unwrap_err()
            .contains("closed window")
    );

    let mut duplicate = browser_task();
    duplicate["allowed_methods"] = json!(["browser_snapshot", "browser_snapshot"]);
    assert!(
        server
            .prepare_task(duplicate)
            .unwrap_err()
            .contains("duplicates")
    );
}

#[rstest]
#[case("snapshot", json!({}), true)]
#[case("snapshot", json!({"capture_diagnostics":false}), true)]
#[case("snapshot", json!({"capture_diagnostics":true}), true)]
#[case("snapshot", json!({"capture_diagnostics":"true"}), false)]
#[case("snapshot", json!({"capture_diagnostics":null}), false)]
#[case("get_window_state", json!({"capture_diagnostics":true}), false)]
fn capture_diagnostics_public_task_params_are_explicit_and_typed(
    #[case] method: &str,
    #[case] params: Value,
    #[case] allowed: bool,
) {
    assert_eq!(
        validate_task_method_params(method, &params).is_ok(),
        allowed
    );
}

#[rstest]
fn capture_diagnostics_public_schema_defaults_false_without_an_extra_tool_or_grant() {
    let start = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "start_task")
        .unwrap();
    assert_eq!(
        start["inputSchema"]["properties"]["allowed_actions"]["minItems"],
        0
    );
    let call = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "dcc_cua_task_call")
        .unwrap();
    let condition = call["inputSchema"]["allOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|condition| condition["if"]["properties"]["method"]["const"] == "snapshot")
        .unwrap();
    assert_eq!(condition["if"]["properties"]["method"]["const"], "snapshot");
    let flag = &condition["then"]["properties"]["params"]["properties"]["capture_diagnostics"];
    assert_eq!(flag["type"], "boolean");
    assert_eq!(flag["default"], false);
    let mut task = pixels_task();
    task["allowed_methods"] = json!([
        "snapshot",
        "get_window_state",
        "change_window_state",
        "session_health"
    ]);
    task["allowed_actions"] = json!([]);
    let mut server = test_server();
    let prepared = server.prepare_task(task).unwrap();
    let proposal = server
        .proposals
        .get(prepared["task_id"].as_str().unwrap())
        .unwrap();
    let grant = task_session_grant(proposal, proposal.receipt.as_ref().unwrap());
    assert_eq!(grant["allow_raw_input"], false);
    assert_eq!(grant["allow_browser_input"], false);
}

#[rstest]
#[tokio::test]
async fn capture_diagnostics_observation_only_stop_and_expiry_remain_closed() {
    let mut server = test_server();
    let mut task = pixels_task();
    task["allowed_methods"] = json!(["snapshot"]);
    task["allowed_actions"] = json!([]);
    let prepared = server.prepare_task(task.clone()).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    server.revoke_task(json!({"task_id": id})).await.unwrap();
    let error = server
        .task_call(
            json!({"task_id":id, "method":"snapshot", "params":{"capture_diagnostics":true}}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("stopped"), "{error}");
    let prepared = server.prepare_task(task).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    server
        .proposals
        .get_mut(id)
        .unwrap()
        .registration
        .expires_at_unix_ms = unix_time_millis().saturating_sub(1);
    let error = server
        .task_call(
            json!({"task_id":id, "method":"snapshot", "params":{"capture_diagnostics":true}}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("expired"), "{error}");
}
