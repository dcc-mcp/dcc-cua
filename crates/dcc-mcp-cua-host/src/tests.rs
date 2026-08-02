use rstest::rstest;
use serde_json::Value;
use tokio::io::{AsyncWrite, DuplexStream};

use super::*;

async fn write_json_request(
    writer: &mut (impl AsyncWrite + Unpin),
    value: Value,
) -> Result<(), HostError> {
    write_frame(
        writer,
        &serde_json::to_vec(&value).unwrap(),
        MAX_JSON_FRAME_BYTES,
    )
    .await
}

#[rstest]
#[tokio::test]
async fn process_connection_negotiates_and_rejects_duplicate_hello() {
    let (mut client, server_stream): (DuplexStream, DuplexStream) = tokio::io::duplex(16 * 1024);
    let server = tokio::spawn(process_connection(
        ComputerUseDriver::create().unwrap(),
        server_stream,
    ));

    let hello = json!({
        "request_id": "hello-1",
        "method": "hello",
        "params": {
            "protocol_version": HOST_PROTOCOL_VERSION,
            "client_name": "host-integration-test",
            "snapshot_transport": "binary_frame"
        }
    });
    write_json_request(&mut client, hello).await.unwrap();
    let response = read_frame(&mut client, MAX_JSON_FRAME_BYTES)
        .await
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["type"], "hello");
    assert_eq!(response["request_id"], "hello-1");
    assert_eq!(response["protocol_version"], HOST_PROTOCOL_VERSION);
    assert!(
        response["capabilities"]
            .as_array()
            .is_some_and(|items| { items.iter().any(|item| item == "pipelined_read_requests") })
    );
    assert!(
        response["capabilities"]
            .as_array()
            .is_some_and(|items| { items.iter().any(|item| item == "window_inventory_filters") })
    );

    write_json_request(
        &mut client,
        json!({
            "request_id": "hello-2",
            "method": "hello",
            "params": {
                "protocol_version": HOST_PROTOCOL_VERSION,
                "client_name": "host-integration-test",
                "snapshot_transport": "shared_memory"
            }
        }),
    )
    .await
    .unwrap();
    let response = read_frame(&mut client, MAX_JSON_FRAME_BYTES)
        .await
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["type"], "error");
    assert_eq!(response["request_id"], "hello-2");
    assert_eq!(response["code"], "invalid_request");

    drop(client);
    assert!(server.await.unwrap().is_ok());
}

#[rstest]
fn frame_prefix_is_big_endian_and_bounded() {
    assert_eq!(u32::from_be_bytes((42_u32).to_be_bytes()), 42);
    const { assert!(MAX_BINARY_FRAME_BYTES > MAX_JSON_FRAME_BYTES) };
}

#[cfg(unix)]
#[rstest]
fn only_refused_or_missing_unix_sockets_are_replaceable() {
    assert!(stale_unix_socket_error(&std::io::Error::from(
        std::io::ErrorKind::ConnectionRefused,
    )));
    assert!(stale_unix_socket_error(&std::io::Error::from(
        std::io::ErrorKind::NotFound,
    )));
    assert!(!stale_unix_socket_error(&std::io::Error::from(
        std::io::ErrorKind::PermissionDenied,
    )));
}

#[rstest]
fn request_ids_are_optional_bounded_and_echoable() {
    assert_eq!(request_id_from(&json!({})).unwrap(), None);
    assert_eq!(
        request_id_from(&json!({"request_id":"req-1"})).unwrap(),
        Some("req-1".into())
    );
    assert!(request_id_from(&json!({"request_id":""})).is_err());
    assert!(
        request_id_from(&json!({
            "request_id": "x".repeat(MAX_REQUEST_ID_CHARS + 1)
        }))
        .is_err()
    );
    assert_eq!(
        with_request_id(json!({"type":"ok"}), Some("req-1")),
        json!({"type":"ok", "request_id":"req-1"})
    );
}

#[rstest]
fn wait_cancellation_requires_exact_credentials() {
    let registry = Arc::new(Mutex::new(HashMap::new()));
    let guard = register_wait(&registry, "session-1", "grant-1", "cap-1").unwrap();
    assert!(cancel_wait(&registry, "session-1", "grant-1", "wrong-cap").is_err());
    let response = cancel_wait(&registry, "session-1", "grant-1", "cap-1").unwrap();
    assert_eq!(response["type"], "wait_cancel_requested");
    assert!(guard.handle.cancelled.load(Ordering::Acquire));
}

#[rstest]
fn window_wait_cancellation_uses_the_request_id_handle() {
    let registry = Arc::new(Mutex::new(HashMap::new()));
    let guard = register_window_wait(&registry, "window-wait-1").unwrap();
    let response = cancel_window_wait(&registry, "window-wait-1").unwrap();
    assert_eq!(response["type"], "window_wait_cancel_requested");
    assert_eq!(response["wait_id"], "window-wait-1");
    assert!(guard.handle.cancelled.load(Ordering::Acquire));
    assert!(cancel_window_wait(&registry, "missing").is_err());
}

#[rstest]
fn request_frame_preserves_correlation_on_deserialization_errors() {
    let parsed = parse_request_frame(br#"{"request_id":"req-7","method":"unknown","params":{}}"#);
    assert_eq!(parsed.unwrap_err().0, Some("req-7".into()));
}

#[rstest]
fn window_state_wire_surface_matches_cua_capability() {
    assert!(serde_json::from_value::<WindowOperation>(json!("activate")).is_ok());
    assert!(serde_json::from_value::<WindowOperation>(json!("restore")).is_err());
    assert!(serde_json::from_value::<WindowOperation>(json!("show")).is_err());
}

#[rstest]
fn hard_denied_intents_do_not_reach_cua() {
    let action = HostAction {
        action: "keypress".into(),
        element_index: None,
        element_token: None,
        delivery_mode: None,
        input_kind: "raw_input".into(),
        intent: "terminal_or_run_dialog".into(),
        x: None,
        y: None,
        button: None,
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: Vec::new(),
        text: None,
        delay_ms: None,
        type_chars_only: false,
        checked: None,
        keys: vec!["ENTER".into()],
        modifiers: Vec::new(),
        duration_ms: None,
        steps: None,
    };
    assert!(action.reject_policy().is_some());
}

#[rstest]
fn semantic_actions_require_element_locator() {
    let action = HostAction {
        action: "set_checked".into(),
        element_index: None,
        element_token: None,
        delivery_mode: None,
        input_kind: "semantic".into(),
        intent: "ordinary_edit".into(),
        x: None,
        y: None,
        button: None,
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: Vec::new(),
        text: None,
        delay_ms: None,
        type_chars_only: false,
        checked: Some(true),
        keys: Vec::new(),
        modifiers: Vec::new(),
        duration_ms: None,
        steps: None,
    };
    let error = action.into_computer_use("obs-1".into()).unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::InvalidAction);
}

#[rstest]
fn semantic_actions_forward_element_tokens_and_delivery_mode() {
    let action = HostAction {
        action: "click".into(),
        element_index: None,
        element_token: Some("element-token".into()),
        delivery_mode: Some("background".into()),
        input_kind: "semantic".into(),
        intent: "ordinary_edit".into(),
        x: None,
        y: None,
        button: None,
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: Vec::new(),
        text: None,
        delay_ms: None,
        type_chars_only: false,
        checked: None,
        keys: Vec::new(),
        modifiers: Vec::new(),
        duration_ms: None,
        steps: None,
    };
    let action = action.into_computer_use("obs-1".into()).unwrap();
    assert_eq!(action.element_token.as_deref(), Some("element-token"));
    assert_eq!(action.delivery_mode.as_deref(), Some("background"));
}

#[rstest]
fn hello_selects_snapshot_transport() {
    let shared_memory = HelloParams {
        protocol_version: HOST_PROTOCOL_VERSION,
        client_name: "test-client".into(),
        snapshot_transport: Some("shared_memory".into()),
    };
    assert_eq!(
        SnapshotTransport::from_hello(&shared_memory).unwrap(),
        SnapshotTransport::SharedMemory
    );
    let binary_frame = HelloParams {
        protocol_version: HOST_PROTOCOL_VERSION,
        client_name: "test-client".into(),
        snapshot_transport: None,
    };
    assert_eq!(
        SnapshotTransport::from_hello(&binary_frame).unwrap(),
        SnapshotTransport::BinaryFrame
    );
}

#[rstest]
fn app_launch_grant_defaults_to_denied() {
    let grant: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": "task-1",
        "dcc_type": "unreal"
    }))
    .expect("minimal grants should be readable");
    assert!(!grant.allow_app_launch);
    assert!(!grant.allow_app_terminate);
    assert!(!grant.allow_clipboard_read);
    assert!(!grant.allow_clipboard_write);
    assert!(!grant.allow_recording);
    assert!(!grant.allow_browser_input);
    assert!(!grant.allow_browser_prepare);
    assert!(!grant.allow_browser_download);
    assert!(!grant.allow_native_tool);
    assert!(!grant.allow_session_escalation);
    assert_eq!(
        error_code(&HostError::Protocol(
            "browser download is not granted".into()
        )),
        "browser_download_not_granted"
    );
}

#[rstest]
fn app_requests_parse_with_host_params_frames() {
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "list_apps",
            "params": {}
        })),
        Ok(Request::ListApps {})
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "list_tools",
            "params": {}
        })),
        Ok(Request::ListTools {})
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "list_windows",
            "params": {
                "app": "chrome.exe",
                "window_id": 42,
                "window_title": "PCG Fab"
            }
        })),
        Ok(Request::ListWindows {
            window_id: Some(42),
            window_title: Some(title),
            ..
        }) if title == "PCG Fab"
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "wait_for_window",
            "params": {"query": {"app": "UE5Editor.exe"}, "timeout_ms": 1000}
        })),
        Ok(Request::WaitForWindow(..))
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "cancel_window_wait",
            "params": {"wait_id": "window-wait-1"}
        })),
        Ok(Request::CancelWindowWait { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "desktop_snapshot",
            "params": {}
        })),
        Ok(Request::DesktopSnapshot {})
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "snapshot",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "activate_before": true
            }
        })),
        Ok(Request::Snapshot {
            max_depth: 0,
            max_nodes: 0,
            activate_before: true,
            ..
        })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "zoom",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {
                    "observation_id": "session-1-obs-1",
                    "x1": 10,
                    "y1": 20,
                    "x2": 400,
                    "y2": 200
                }
            }
        })),
        Ok(Request::Zoom { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "verify_state",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "expect": [{"window": {"exists": true}}],
                "stable_samples": 2
            }
        })),
        Ok(Request::VerifyState { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "execute_action",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "observation_id": "obs-1",
                "accessibility_state_id": "obs-1",
                "action": {
                    "action": "click",
                    "input_kind": "semantic",
                    "intent": "ordinary_edit",
                    "element_token": "token-1"
                },
                "capture_after": true,
                "post_snapshot_max_nodes": 256,
                "post_snapshot_max_depth": 12
            }
        })),
        Ok(Request::ExecuteAction {
            capture_after: true,
            post_snapshot_max_nodes: 256,
            post_snapshot_max_depth: 12,
            ..
        })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "call_tool",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "tool": "debug_window_info",
                "arguments": {}
            }
        })),
        Ok(Request::CallTool { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "call_global_tool",
            "params": {
                "grant": {
                    "task_grant_id": "task-1",
                    "dcc_type": "desktop",
                    "allow_native_tool": true
                },
                "tool": "health_report",
                "arguments": {}
            }
        })),
        Ok(Request::CallGlobalTool { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "screen_size",
            "params": {}
        })),
        Ok(Request::ScreenSize {})
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "get_session_state",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1"
            }
        })),
        Ok(Request::GetSessionState { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "escalate_session",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "reason": "foreground_ineffective",
                "detail": "window route exhausted"
            }
        })),
        Ok(Request::EscalateSession { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "cursor_tool",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "tool": "set_agent_cursor_enabled",
                "arguments": {"enabled": true}
            }
        })),
        Ok(Request::CursorTool { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "cursor_position",
            "params": {}
        })),
        Ok(Request::CursorPosition {})
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "open_desktop_session",
            "params": {
                "session_id": "desktop-1",
                "grant": {
                    "task_grant_id": "task-1",
                    "dcc_type": "desktop",
                    "allow_raw_input": true
                }
            }
        })),
        Ok(Request::OpenDesktopSession { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "execute_desktop_action",
            "params": {
                "session_id": "desktop-1",
                "task_grant_id": "task-1",
                "desktop_capability": "cap-1",
                "observation_id": "desktop-obs-1",
                "capture_after": true,
                "action": {
                    "action": "click",
                    "input_kind": "raw_input",
                    "intent": "ordinary_edit",
                    "x": 10,
                    "y": 20
                }
            }
        })),
        Ok(Request::ExecuteDesktopAction {
            capture_after: true,
            ..
        })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "launch_app",
            "params": {
                "grant": {
                    "task_grant_id": "task-1",
                    "dcc_type": "unreal",
                    "allow_app_launch": true
                },
                "launch": {"name": "Calculator"}
            }
        })),
        Ok(Request::LaunchApp { .. })
    ));
    let request = serde_json::from_value::<Request>(json!({
        "method": "open_session",
        "params": {
            "session_id": "session-title",
            "grant": {
                "task_grant_id": "task-1",
                "dcc_type": "unreal",
                "window_title": "PCG Fab"
            }
        }
    }))
    .unwrap();
    let Request::OpenSession { grant, .. } = request else {
        panic!("expected open_session request");
    };
    assert_eq!(grant.window_title.as_deref(), Some("PCG Fab"));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "terminate_app",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1"
            }
        })),
        Ok(Request::TerminateApp { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "wait_for",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "condition": {
                    "kind": "text_contains",
                    "element_index": 3,
                    "text": "Ready"
                }
            }
        })),
        Ok(Request::WaitFor { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "find",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "query": {"text": "Ready", "max_results": 3}
            }
        })),
        Ok(Request::Find { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "browser_snapshot",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {"snapshot_format": "semantic_v2"}
            }
        })),
        Ok(Request::BrowserSnapshot { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "clipboard_write",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "write": {"text": "hello"}
            }
        })),
        Ok(Request::ClipboardWrite { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "recording_start",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {"output_dir": "C:/tmp/cua"}
            }
        })),
        Ok(Request::RecordingStart { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "browser_prepare",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {"allow_launch": false}
            }
        })),
        Ok(Request::BrowserPrepare { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "browser_type",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {
                    "target_id": "target-1",
                    "tab_id": "tab-1",
                    "snapshot_id": "p1",
                    "ref": "p1:2",
                    "text": "Fab"
                }
            }
        })),
        Ok(Request::BrowserType { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "browser_set_input_files",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {
                    "target_id": "target-1",
                    "tab_id": "tab-1",
                    "snapshot_id": "p1",
                    "ref": "p1:4",
                    "files": ["C:/tmp/input.fbx"]
                }
            }
        })),
        Ok(Request::BrowserSetInputFiles { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "browser_download",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {
                    "target_id": "target-1",
                    "tab_id": "tab-1",
                    "snapshot_id": "p1",
                    "ref": "p1:5",
                    "destination_root": "C:/tmp/downloads"
                }
            }
        })),
        Ok(Request::BrowserDownload { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "browser_dialog",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {
                    "target_id": "target-1",
                    "tab_id": "tab-1",
                    "action": "inspect"
                }
            }
        })),
        Ok(Request::BrowserDialog { .. })
    ));
}

#[rstest]
fn only_stateless_discovery_uses_parallel_dispatch() {
    assert!(is_parallel_request(&Request::ListApps {}));
    assert!(is_parallel_request(&Request::ListTools {}));
    assert!(is_parallel_request(&Request::ScreenSize {}));
    assert!(is_parallel_request(&Request::CursorPosition {}));
    assert!(!is_parallel_request(&Request::DesktopSnapshot {}));
}

#[rstest]
fn native_tool_response_moves_image_pixels_to_binary_attachment() {
    let mut shared = None;
    let (response, attachment) = native_tool_response_with_transport(
        Some("session-1"),
        "debug_window_info",
        ComputerUseToolResult {
            value: json!({
                "content": [{"type": "image", "mimeType": "image/png", "data": "base64"}]
            }),
            text: String::new(),
            images: vec![dcc_mcp_cua_core::ComputerUseImage {
                data: vec![1, 2, 3],
                mime_type: "image/png".into(),
            }],
            degraded: false,
        },
        SnapshotTransport::BinaryFrame,
        &mut shared,
    )
    .unwrap();
    assert_eq!(response["type"], "tool_result");
    assert_eq!(response["result"]["content"][0]["data"], Value::Null);
    assert_eq!(response["image"]["length"], 3);
    assert_eq!(attachment, Some(vec![1, 2, 3]));
}

#[rstest]
fn native_tool_response_concatenates_all_image_attachments() {
    let mut shared = None;
    let (response, attachment) = native_tool_response_with_transport(
        None,
        "page",
        ComputerUseToolResult {
            value: json!({
                "content": [
                    {"type": "image", "data": "first"},
                    {"type": "image", "data": "second"}
                ]
            }),
            text: String::new(),
            images: vec![
                dcc_mcp_cua_core::ComputerUseImage {
                    data: vec![1, 2],
                    mime_type: "image/png".into(),
                },
                dcc_mcp_cua_core::ComputerUseImage {
                    data: vec![3, 4, 5],
                    mime_type: "image/jpeg".into(),
                },
            ],
            degraded: false,
        },
        SnapshotTransport::BinaryFrame,
        &mut shared,
    )
    .unwrap();
    assert_eq!(response["attachments"].as_array().map(Vec::len), Some(2));
    assert_eq!(response["attachments"][1]["offset"], 2);
    assert_eq!(response["result"]["content"][0]["data"], Value::Null);
    assert_eq!(response["result"]["content"][1]["data"], Value::Null);
    assert_eq!(response["result"]["content"][0]["attachment_index"], 0);
    assert_eq!(response["result"]["content"][1]["length"], 3);
    assert_eq!(attachment, Some(vec![1, 2, 3, 4, 5]));
}

#[rstest]
fn action_response_preserves_tool_metadata_and_images() {
    let mut shared = None;
    let (response, attachment) = action_completed_response(
        "session-1",
        "action-1".into(),
        "CUA action completed",
        ComputerUseToolResult {
            value: json!({"success": true, "cua": {"accepted": true}}),
            text: "clicked".into(),
            images: vec![dcc_mcp_cua_core::ComputerUseImage {
                data: vec![7, 8],
                mime_type: "image/png".into(),
            }],
            degraded: true,
        },
        SnapshotTransport::BinaryFrame,
        &mut shared,
    )
    .unwrap();
    assert_eq!(response["type"], "action_completed");
    assert_eq!(response["action_id"], "action-1");
    assert_eq!(response["result"]["cua"]["accepted"], true);
    assert_eq!(response["text"], "clicked");
    assert_eq!(response["degraded"], true);
    assert_eq!(response["image"]["length"], 2);
    assert_eq!(attachment, Some(vec![7, 8]));
}

#[rstest]
fn action_response_uses_shared_memory_for_one_image() {
    let mut shared = None;
    let (response, attachment) = action_completed_response(
        "session-1",
        "action-1".into(),
        "CUA action completed",
        ComputerUseToolResult {
            value: json!({
                "content": [{"type": "image", "data": "base64"}]
            }),
            text: "clicked".into(),
            images: vec![dcc_mcp_cua_core::ComputerUseImage {
                data: vec![7, 8],
                mime_type: "image/png".into(),
            }],
            degraded: false,
        },
        SnapshotTransport::SharedMemory,
        &mut shared,
    )
    .unwrap();
    assert_eq!(response["image"]["encoding"], "shared_memory");
    assert_eq!(
        response["result"]["content"][0]["encoding"],
        "shared_memory"
    );
    assert_eq!(response["result"]["content"][0]["data"], Value::Null);
    assert!(attachment.is_none());
    assert!(shared.is_some_and(|image| image.is_alive()));
}

#[rstest]
fn action_post_snapshot_reuses_the_single_attachment_frame() {
    let mut shared = None;
    let (response, attachment) = action_completed_with_snapshot_response(
        "session-1",
        "action-1".into(),
        ComputerUseToolResult {
            value: json!({"accepted": true}),
            text: "clicked".into(),
            images: vec![ComputerUseImage {
                data: vec![7, 8],
                mime_type: "image/png".into(),
            }],
            degraded: false,
        },
        ComputerUseScreenshot {
            data: vec![1, 2, 3],
            observation: dcc_mcp_cua_core::ComputerUseObservation {
                observation_id: "obs-after".into(),
                window_handle: 7,
                process_id: 42,
                window_title: "DCC".into(),
                width: 1280,
                height: 720,
                source_rect: [0, 0, 1280, 720],
                capture_backend: "test".into(),
                capture_provenance: json!({}),
                session_id: "session-1".into(),
            },
            accessibility: json!({"elements": [{"element_token": "next"}]}),
        },
        SnapshotTransport::BinaryFrame,
        &mut shared,
    )
    .unwrap();
    assert_eq!(response["post_snapshot"]["observation_id"], "obs-after");
    assert_eq!(response["post_snapshot"]["node_count"], 1);
    assert_eq!(response["post_snapshot"]["image"]["index"], 1);
    assert_eq!(response["post_snapshot"]["image"]["offset"], 2);
    assert_eq!(attachment, Some(vec![7, 8, 1, 2, 3]));
}

#[rstest]
fn desktop_action_post_snapshot_reuses_the_single_attachment_frame() {
    let mut shared = None;
    let (response, attachment) = desktop_action_completed_with_snapshot_response(
        "desktop-1",
        "action-1".into(),
        ComputerUseToolResult {
            value: json!({"accepted": true}),
            text: "clicked".into(),
            images: vec![ComputerUseImage {
                data: vec![7, 8],
                mime_type: "image/png".into(),
            }],
            degraded: false,
        },
        ComputerUseDesktopSnapshot {
            data: vec![1, 2, 3],
            state: json!({"screen_size": {"width": 1920, "height": 1080}}),
            observation_id: "desktop-obs-after".into(),
        },
        SnapshotTransport::BinaryFrame,
        &mut shared,
    )
    .unwrap();
    assert_eq!(
        response["post_snapshot"]["observation_id"],
        "desktop-obs-after"
    );
    assert_eq!(
        response["post_snapshot"]["state"]["screen_size"]["width"],
        1920
    );
    assert_eq!(response["post_snapshot"]["image"]["index"], 1);
    assert_eq!(response["post_snapshot"]["image"]["offset"], 2);
    assert_eq!(attachment, Some(vec![7, 8, 1, 2, 3]));
}

#[rstest]
fn browser_response_uses_shared_memory_for_one_image() {
    let mut shared = None;
    let (response, attachment) = browser_response(
        "browser_snapshot",
        "session-1".into(),
        dcc_mcp_cua_browser::BrowserResult {
            value: json!({
                "content": [{"type":"image", "data": "base64"}]
            }),
            images: vec![dcc_mcp_cua_browser::BrowserImage {
                data: vec![1, 2, 3],
                mime_type: "image/png".into(),
            }],
        },
        SnapshotTransport::SharedMemory,
        &mut shared,
    )
    .unwrap();

    assert_eq!(response["image"]["encoding"], "shared_memory");
    assert_eq!(response["image"]["length"], 3);
    assert_eq!(response["result"]["content"][0]["data"], Value::Null);
    assert!(attachment.is_none());
    assert!(shared.is_some_and(|image| image.is_alive()));
}

#[rstest]
fn verification_images_follow_the_negotiated_transport() {
    let mut shared = None;
    let (shared_response, shared_attachment) = image_response(
        ComputerUseImage {
            data: vec![1, 2, 3],
            mime_type: "image/png".into(),
        },
        SnapshotTransport::SharedMemory,
        &mut shared,
    )
    .unwrap();
    assert_eq!(shared_response["encoding"], "shared_memory");
    assert!(shared_attachment.is_none());
    assert!(shared.is_some_and(|image| image.is_alive()));

    let mut no_shared_image = None;
    let (binary_response, binary_attachment) = image_response(
        ComputerUseImage {
            data: vec![4, 5],
            mime_type: "image/jpeg".into(),
        },
        SnapshotTransport::BinaryFrame,
        &mut no_shared_image,
    )
    .unwrap();
    assert_eq!(binary_response["encoding"], "binary_frame");
    assert_eq!(binary_response["length"], 2);
    assert_eq!(binary_attachment, Some(vec![4, 5]));
    assert!(no_shared_image.is_none());
}

#[rstest]
fn browser_response_concatenates_multiple_binary_images() {
    let mut shared = None;
    let (response, attachment) = browser_response(
        "browser_snapshot",
        "session-1".into(),
        dcc_mcp_cua_browser::BrowserResult {
            value: json!({
                "content": [
                    {"type":"image", "data": "first"},
                    {"type":"image", "data": "second"}
                ]
            }),
            images: vec![
                dcc_mcp_cua_browser::BrowserImage {
                    data: vec![1, 2],
                    mime_type: "image/png".into(),
                },
                dcc_mcp_cua_browser::BrowserImage {
                    data: vec![3, 4, 5],
                    mime_type: "image/jpeg".into(),
                },
            ],
        },
        SnapshotTransport::BinaryFrame,
        &mut shared,
    )
    .unwrap();

    assert_eq!(response["attachments"].as_array().map(Vec::len), Some(2));
    assert_eq!(response["attachments"][1]["offset"], 2);
    assert_eq!(response["result"]["content"][1]["data"], Value::Null);
    assert_eq!(attachment, Some(vec![1, 2, 3, 4, 5]));
    assert!(shared.is_none());
}

#[rstest]
fn find_queries_filter_semantic_elements() {
    let root = json!({
        "elements": [
            {"element_index": 3, "role": "Button", "name": "Ready"},
            {"element_index": 4, "role": "Text", "name": "Ready"},
            {"element_index": 5, "role": "Button", "name": "Cancel"}
        ]
    });
    let query = FindQuery {
        text: Some("ready".into()),
        role: Some("button".into()),
        element_index: None,
        max_results: Some(10),
    };
    let matches = find_elements(&root, &query, query.validate().unwrap());
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["element_index"], 3);
    assert!(
        FindQuery {
            text: None,
            role: None,
            element_index: None,
            max_results: None,
        }
        .validate()
        .is_err()
    );
}

#[rstest]
fn wait_conditions_match_bounded_accessibility_elements() {
    let root = json!({
        "elements": [
            {"element_index": 3, "role": "text", "name": "Ready to render", "value": "idle"}
        ]
    });
    let condition = WaitCondition {
        kind: "text_contains".into(),
        element_index: Some(3),
        text: Some("Ready".into()),
        value: None,
        timeout_ms: None,
        interval_ms: None,
    };
    assert!(wait_condition_matches(&root, &condition));
    assert!(!wait_condition_matches(
        &root,
        &WaitCondition {
            kind: "value_equals".into(),
            element_index: Some(3),
            text: None,
            value: Some("done".into()),
            timeout_ms: None,
            interval_ms: None,
        }
    ));
    assert!(
        WaitCondition {
            kind: "text_equals".into(),
            element_index: None,
            text: Some("Ready to render".into()),
            value: None,
            timeout_ms: Some(60_000),
            interval_ms: Some(1),
        }
        .validate()
        .is_ok()
    );
}
