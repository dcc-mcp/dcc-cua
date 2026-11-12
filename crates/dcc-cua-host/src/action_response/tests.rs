use super::*;
use rstest::rstest;

fn observation(handle: u64) -> dcc_cua_core::ComputerUseObservation {
    dcc_cua_core::ComputerUseObservation {
        observation_id: format!("obs-{handle}"),
        window_handle: handle,
        process_id: 77,
        window_title: String::new(),
        width: 10,
        height: 10,
        source_rect: [0, 0, 10, 10],
        capture_backend: "dcc-cua-visible-exact-window".into(),
        capture_provenance: json!({}),
        session_id: "session".into(),
    }
}

#[rstest]
fn composite_snapshot_transport_keeps_root_and_selected_image_order_distinct() {
    let overlays = vec![
        ComputerUseScreenshot {
            data: b"a".to_vec(),
            observation: observation(200),
            accessibility: json!({}),
        },
        ComputerUseScreenshot {
            data: b"bc".to_vec(),
            observation: observation(300),
            accessibility: json!({}),
        },
    ];
    let mut public = json!({"overlays":[{"window_handle":200},{"window_handle":300}]});
    let prepared = snapshot_image_transport(
        b"root".to_vec(),
        &observation(100),
        overlays,
        &mut public,
        SnapshotTransport::BinaryFrame,
        &mut None,
    )
    .unwrap();
    assert_eq!(prepared.attachment.unwrap(), b"rootabc");
    assert_eq!(prepared.primary.unwrap()["offset"], 0);
    assert_eq!(public["overlays"][0]["image"]["offset"], 4);
    assert_eq!(public["overlays"][0]["image"]["length"], 1);
    assert_eq!(public["overlays"][1]["image"]["offset"], 5);
    assert_eq!(public["overlays"][1]["image"]["length"], 2);
    assert_eq!(prepared.attachments.len(), 3);
}

#[rstest]
fn ordinary_snapshot_descriptor_remains_unchanged_without_secondary_images() {
    let prepared = snapshot_image_transport(
        b"root".to_vec(),
        &observation(100),
        Vec::new(),
        &mut Value::Null,
        SnapshotTransport::BinaryFrame,
        &mut None,
    )
    .unwrap();
    assert!(prepared.attachments.is_empty());
    assert_eq!(prepared.primary.unwrap()["id"], "obs-100");
}

fn owned_payload_result(status: ComputerUseToolStatus) -> ComputerUseToolResult {
    ComputerUseToolResult {
        status,
        value: json!({
            "success": status != ComputerUseToolStatus::Succeeded,
            "payload": {
                "text": "synthetic payload ".repeat(1024),
                "items": [1, true, null, {"label": "nested value"}],
            },
        }),
        text: "synthetic result ".repeat(512),
        images: Vec::new(),
        degraded: status == ComputerUseToolStatus::Rejected,
    }
}

#[rstest]
#[case::succeeded(ComputerUseToolStatus::Succeeded, "succeeded", Some("session-1"))]
#[case::rejected(ComputerUseToolStatus::Rejected, "rejected", None)]
fn native_response_preserves_owned_payload_and_wire_contract(
    #[case] status: ComputerUseToolStatus,
    #[case] status_name: &str,
    #[case] session_id: Option<&str>,
) {
    let result = owned_payload_result(status);
    let payload_pointer = result.value["payload"]["text"].as_str().unwrap().as_ptr();
    let text_pointer = result.text.as_ptr();
    let mut expected = json!({
        "type": "tool_result",
        "session_id": session_id,
        "tool": "synthetic_tool",
        "status": status_name,
        "result": result.value,
        "text": result.text,
        "degraded": result.degraded,
    });
    expected["result"]["success"] = Value::Bool(status == ComputerUseToolStatus::Succeeded);
    let mut shared = None;

    let (response, attachment) = native_tool_response_with_transport(
        session_id,
        "synthetic_tool",
        result,
        SnapshotTransport::BinaryFrame,
        &mut shared,
    )
    .unwrap();

    assert_eq!(response, expected);
    assert!(attachment.is_none());
    assert!(shared.is_none());
    assert_eq!(
        response["result"]["payload"]["text"]
            .as_str()
            .unwrap()
            .as_ptr(),
        payload_pointer,
        "the response must retain the owned payload allocation"
    );
    assert_eq!(
        response["text"].as_str().unwrap().as_ptr(),
        text_pointer,
        "the response must retain the owned text allocation"
    );
}

#[rstest]
#[case::succeeded(ComputerUseToolStatus::Succeeded, "succeeded")]
#[case::rejected(ComputerUseToolStatus::Rejected, "rejected")]
fn action_response_preserves_owned_payload_and_wire_contract(
    #[case] status: ComputerUseToolStatus,
    #[case] status_name: &str,
    #[values(false, true)] with_image: bool,
) {
    let mut result = owned_payload_result(status);
    let payload_pointer = result.value["payload"]["text"].as_str().unwrap().as_ptr();
    let text_pointer = result.text.as_ptr();
    let mut expected = json!({
        "type": "action_completed",
        "success": status == ComputerUseToolStatus::Succeeded,
        "status": status_name,
        "action_id": "action-1",
        "target_closed": false,
        "policy_tier": "task_grant",
        "message": "Synthetic action completed",
        "result": result.value,
        "text": result.text,
        "degraded": result.degraded,
    });
    expected["result"]["success"] = Value::Bool(status == ComputerUseToolStatus::Succeeded);
    if with_image {
        result.images.push(ComputerUseImage {
            data: vec![7, 8],
            mime_type: "image/png".into(),
        });
        let descriptor = json!({
            "index": 0,
            "offset": 0,
            "length": 2,
            "mime_type": "image/png",
            "encoding": "binary_frame",
        });
        expected["image"] = descriptor.clone();
        expected["attachments"] = json!([descriptor]);
    }
    let mut shared = None;

    let (response, attachment) = action_completed_response(
        "session-1",
        "action-1".into(),
        "Synthetic action completed",
        result,
        SnapshotTransport::BinaryFrame,
        &mut shared,
    )
    .unwrap();

    assert_eq!(response, expected);
    assert_eq!(attachment, with_image.then(|| vec![7, 8]));
    assert!(shared.is_none());
    assert_eq!(
        response["result"]["payload"]["text"]
            .as_str()
            .unwrap()
            .as_ptr(),
        payload_pointer,
        "the action response must retain the owned payload allocation"
    );
    assert_eq!(
        response["text"].as_str().unwrap().as_ptr(),
        text_pointer,
        "the action response must retain the owned text allocation"
    );
}
