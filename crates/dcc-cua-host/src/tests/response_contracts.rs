use rstest::rstest;

use super::*;

#[rstest]
fn only_stateless_discovery_uses_parallel_dispatch() {
    assert!(is_parallel_request(&Request::Ping {}));
    assert!(is_parallel_request(&Request::ListApps {}));
    assert!(is_parallel_request(&Request::ListTools {}));
    assert!(is_parallel_request(&Request::ScreenSize {}));
    assert!(is_parallel_request(&Request::CursorPosition {}));
    assert!(!is_parallel_request(&Request::Doctor {}));
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
            images: vec![dcc_cua_core::ComputerUseImage {
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
                dcc_cua_core::ComputerUseImage {
                    data: vec![1, 2],
                    mime_type: "image/png".into(),
                },
                dcc_cua_core::ComputerUseImage {
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
            value: json!({
                "success": true,
                "cua": {"accepted": true},
                "banner": {"activity": "observing", "live_observation": true},
            }),
            text: "clicked".into(),
            images: vec![dcc_cua_core::ComputerUseImage {
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
    assert_eq!(response["result"]["banner"]["activity"], "observing");
    assert_eq!(response["result"]["banner"]["live_observation"], true);
    assert_eq!(response["text"], "clicked");
    assert_eq!(response["degraded"], true);
    assert_eq!(response["image"]["length"], 2);
    assert_eq!(attachment, Some(vec![7, 8]));
}

#[rstest]
fn session_stopped_response_preserves_typed_cleanup_issues() {
    let response = session_stopped_response(
        "session-1",
        dcc_cua_core::ComputerUseSessionStopResult {
            success: false,
            active: false,
            cleanup_pending: false,
            cleanup_issues: vec![dcc_cua_core::ComputerUseCleanupIssue {
                phase: dcc_cua_core::ComputerUseCleanupPhase::RecordingStop,
                code: ComputerUseErrorCode::CaptureFailed,
                message: "injected showcase rename failure".into(),
            }],
            marker: dcc_cua_core::ComputerUseMarker {
                visible: false,
                label: "Controlled by Codex".into(),
                backend: "cua-driver-sdk",
            },
        },
    );

    assert_eq!(response["type"], "session_stopped");
    assert_eq!(response["session_id"], "session-1");
    assert_eq!(response["success"], false);
    assert_eq!(response["active"], false);
    assert_eq!(response["cleanup_pending"], false);
    assert_eq!(response["cleanup_issues"][0]["phase"], "recording_stop");
    assert_eq!(response["cleanup_issues"][0]["code"], "capture_failed");
    assert_eq!(
        response["cleanup_issues"][0]["message"],
        "injected showcase rename failure"
    );
    assert_eq!(response["marker"]["backend"], "cua-driver-sdk");
}

#[rstest]
fn action_response_preserves_structured_rejected_input_outcome() {
    let mut shared = None;
    let (response, attachment) = action_completed_response(
        "session-1",
        "action-1".into(),
        "CUA action stopped after its delivery probe",
        ComputerUseToolResult {
            value: json!({
                "success": false,
                "cua": {
                    "effect": "unverifiable",
                    "delivery": {
                        "backend_id": "windows.synthetic_touch.v1",
                        "api_accepted": false,
                        "consumer_effect_confirmed": false,
                        "completion_known": false,
                        "delivered": false,
                        "verification_required": true,
                        "target_fence": {"process_id": 42, "window_handle": 7},
                        "input_trace": {"schema_version": 1}
                    }
                }
            }),
            text: "drag path was not sent".into(),
            images: Vec::new(),
            degraded: true,
        },
        SnapshotTransport::BinaryFrame,
        &mut shared,
    )
    .unwrap();

    assert_eq!(response["success"], false);
    assert_eq!(response["degraded"], true);
    assert_eq!(
        response["result"]["cua"]["delivery"]["backend_id"],
        "windows.synthetic_touch.v1"
    );
    assert_eq!(response["result"]["cua"]["delivery"]["api_accepted"], false);
    assert_eq!(response["result"]["cua"]["delivery"]["delivered"], false);
    assert_eq!(
        response["result"]["cua"]["delivery"]["input_trace"]["schema_version"],
        1
    );
    assert!(attachment.is_none());
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
            images: vec![dcc_cua_core::ComputerUseImage {
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
            observation: dcc_cua_core::ComputerUseObservation {
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
fn post_input_focus_loss_keeps_the_fresh_observation_and_no_retry_contract() {
    let mut shared = None;
    let (response, attachment) = action_completed_with_snapshot_response(
        "session-1",
        "action-focus-lost".into(),
        ComputerUseToolResult {
            value: json!({
                "success": true,
                "cua": {
                    "delivery": {
                        "mode": "foreground",
                        "confirmed": false,
                        "input_sent": true,
                        "retry_safe": false,
                        "verification_required": true,
                        "failure_phase": "post_input_focus_lost"
                    },
                    "effect": "unverifiable"
                }
            }),
            text: "input was sent; inspect the post-action observation".into(),
            images: Vec::new(),
            degraded: true,
        },
        ComputerUseScreenshot {
            data: vec![1, 2, 3],
            observation: dcc_cua_core::ComputerUseObservation {
                observation_id: "obs-after-focus-loss".into(),
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
            accessibility: json!({"elements": []}),
        },
        SnapshotTransport::BinaryFrame,
        &mut shared,
    )
    .unwrap();

    assert_eq!(response["success"], true);
    assert_eq!(response["degraded"], true);
    assert_eq!(response["result"]["cua"]["delivery"]["input_sent"], true);
    assert_eq!(response["result"]["cua"]["delivery"]["retry_safe"], false);
    assert_eq!(response["result"]["cua"]["effect"], "unverifiable");
    assert_eq!(
        response["result"]["cua"]["delivery"]["verification_required"],
        true
    );
    assert!(response["result"]["cua"]["verification_required"].is_null());
    assert_eq!(response["post_snapshot"]["success"], true);
    assert_eq!(
        response["post_snapshot"]["observation_id"],
        "obs-after-focus-loss"
    );
    assert_eq!(attachment, Some(vec![1, 2, 3]));
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
        dcc_cua_browser::BrowserResult {
            value: json!({
                "content": [{"type":"image", "data": "base64"}]
            }),
            images: vec![dcc_cua_browser::BrowserImage {
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
fn browser_response_preserves_ancestor_scope_evidence() {
    let mut shared = None;
    let (response, attachment) = browser_response(
        "browser_snapshot",
        "session-1".into(),
        dcc_cua_browser::BrowserResult {
            value: json!({
                "structuredContent": {
                    "status": "ok",
                    "snapshot": {
                        "scope": "ancestor_subtree",
                        "scope_anchor": {
                            "requested_ref": "p1:7",
                            "role": "row",
                            "frame": "main",
                            "distance": 1
                        }
                    }
                }
            }),
            images: Vec::new(),
        },
        SnapshotTransport::BinaryFrame,
        &mut shared,
    )
    .unwrap();

    assert_eq!(
        response["result"]["structuredContent"]["snapshot"]["scope_anchor"]["requested_ref"],
        "p1:7"
    );
    assert_eq!(
        response["result"]["structuredContent"]["snapshot"]["scope"],
        "ancestor_subtree"
    );
    assert!(attachment.is_none());
    assert!(shared.is_none());
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
        dcc_cua_browser::BrowserResult {
            value: json!({
                "content": [
                    {"type":"image", "data": "first"},
                    {"type":"image", "data": "second"}
                ]
            }),
            images: vec![
                dcc_cua_browser::BrowserImage {
                    data: vec![1, 2],
                    mime_type: "image/png".into(),
                },
                dcc_cua_browser::BrowserImage {
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
            {"element_index": 5, "role": "Button", "name": "Cancel"},
            {"element_index": 6, "role": "Edit", "automation_id": "txt-input"}
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
    let automation_match = find_elements(
        &root,
        &FindQuery {
            text: Some("txt-input".into()),
            role: None,
            element_index: None,
            max_results: Some(10),
        },
        10,
    );
    assert_eq!(automation_match[0]["element_index"], 6);
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

#[rstest]
#[tokio::test]
async fn parallel_discovery_tasks_are_reaped_and_bounded() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let mut tasks = JoinSet::new();
    for _ in 0..MAX_PARALLEL_DISCOVERY_REQUESTS {
        let gate = Arc::clone(&gate);
        tasks.spawn(async move {
            gate.notified().await;
            Ok(())
        });
    }
    assert_eq!(tasks.len(), MAX_PARALLEL_DISCOVERY_REQUESTS);
    gate.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        await_parallel_request_capacity(&mut tasks),
    )
    .await
    .unwrap();
    assert_eq!(tasks.len(), MAX_PARALLEL_DISCOVERY_REQUESTS - 1);

    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    for _ in 0..4 {
        tasks.spawn(async { Ok(()) });
    }
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !tasks.is_empty() {
            tokio::task::yield_now().await;
            reap_completed_parallel_requests(&mut tasks);
        }
    })
    .await
    .unwrap();
}

#[rstest]
#[tokio::test]
async fn raw_input_turn_remains_exclusive_through_post_action_capture() {
    let (action_completed_tx, action_completed_rx) = tokio::sync::oneshot::channel();
    let (capture_completed_tx, capture_completed_rx) = tokio::sync::oneshot::channel();
    let first_turn = tokio::spawn(async move {
        let _input_turn = acquire_raw_input_turn(true).await.expect("raw input lock");
        action_completed_tx.send(()).unwrap();
        capture_completed_rx.await.unwrap();
    });
    action_completed_rx.await.unwrap();

    let (second_started_tx, mut second_started_rx) = tokio::sync::oneshot::channel();
    let waiting_turn = tokio::spawn(async move {
        let _input_turn = acquire_raw_input_turn(true).await.expect("raw input lock");
        second_started_tx.send(()).unwrap();
    });

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(25), &mut second_started_rx,)
            .await
            .is_err(),
        "another raw-input turn must stay blocked while capture_after is in flight",
    );
    capture_completed_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), &mut second_started_rx)
        .await
        .unwrap()
        .unwrap();
    first_turn.await.unwrap();
    waiting_turn.await.unwrap();
}
