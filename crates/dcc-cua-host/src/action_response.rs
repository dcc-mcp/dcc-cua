use super::*;

pub(super) fn native_tool_response_with_transport(
    session_id: Option<&str>,
    tool: &str,
    result: ComputerUseToolResult,
    mode: SnapshotTransport,
    shared_image: &mut Option<SharedImage>,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let mut value = result.value;
    let images = result.images;
    let use_shared_memory = mode == SnapshotTransport::SharedMemory && images.len() == 1;
    let mut attachment_bytes = Vec::new();
    let mut attachments = Vec::with_capacity(images.len());
    for (index, image) in images.iter().enumerate() {
        let offset = attachment_bytes.len();
        if !use_shared_memory {
            attachment_bytes.extend_from_slice(&image.data);
        }
        attachments.push(json!({
            "index": index,
            "offset": offset,
            "length": image.data.len(),
            "mime_type": image.mime_type,
            "encoding": if use_shared_memory { "shared_memory" } else { "binary_frame" },
        }));
    }
    let image_descriptor = if use_shared_memory {
        let image = &images[0];
        let shared = SharedImage::from_bytes(&image.data, &image.mime_type)
            .map_err(|error| HostError::Protocol(error.to_string()))?;
        let mut descriptor = serde_json::to_value(shared.descriptor())
            .map_err(|error| HostError::Protocol(error.to_string()))?;
        descriptor["encoding"] = Value::String("shared_memory".into());
        *shared_image = Some(shared);
        Some(descriptor)
    } else {
        attachments.first().cloned()
    };
    if !images.is_empty()
        && let Some(content) = value.get_mut("content").and_then(Value::as_array_mut)
    {
        for (index, item) in content
            .iter_mut()
            .filter(|item| item["type"] == "image")
            .enumerate()
        {
            let Some(image) = images.get(index) else {
                break;
            };
            item["data"] = Value::Null;
            item["encoding"] = Value::String(
                if use_shared_memory {
                    "shared_memory"
                } else {
                    "binary_frame"
                }
                .into(),
            );
            item["attachment_index"] = json!(index);
            item["offset"] = json!(attachments[index]["offset"]);
            item["length"] = json!(image.data.len());
        }
    }
    let mut response = json!({
        "type": "tool_result",
        "session_id": session_id,
        "tool": tool,
        "result": value,
        "text": result.text,
        "degraded": result.degraded,
    });
    if let Some(session_id) = session_id {
        response["session_id"] = Value::String(session_id.to_owned());
    }
    if let Some(image) = image_descriptor {
        response["image"] = image;
        if !use_shared_memory {
            response["attachments"] = Value::Array(attachments);
        }
    }
    let attachment = (!attachment_bytes.is_empty()).then_some(attachment_bytes);
    Ok((response, attachment))
}

pub(super) fn action_completed_response(
    session_id: &str,
    action_id: String,
    message: &str,
    result: ComputerUseToolResult,
    mode: SnapshotTransport,
    shared_image: &mut Option<SharedImage>,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let (tool_response, attachment) = native_tool_response_with_transport(
        Some(session_id),
        "action",
        result,
        mode,
        shared_image,
    )?;
    let success = tool_response["result"]["success"].as_bool().unwrap_or(true);
    let mut response = json!({
        "type": "action_completed",
        "success": success,
        "action_id": action_id,
        "target_closed": false,
        "policy_tier": "task_grant",
        "message": message,
        "result": tool_response["result"].clone(),
        "text": tool_response["text"].clone(),
        "degraded": tool_response["degraded"].clone(),
    });
    for field in ["image", "attachments"] {
        if !tool_response[field].is_null() {
            response[field] = tool_response[field].clone();
        }
    }
    Ok((response, attachment))
}

fn response_image_descriptor(response: &Value, image_index: usize) -> Option<Value> {
    response["attachments"]
        .as_array()
        .and_then(|attachments| attachments.get(image_index))
        .cloned()
        .or_else(|| {
            (image_index == 0 && !response["image"].is_null()).then(|| response["image"].clone())
        })
}

pub(super) fn action_completed_with_snapshot_response(
    session_id: &str,
    action_id: String,
    mut result: ComputerUseToolResult,
    screenshot: ComputerUseScreenshot,
    mode: SnapshotTransport,
    shared_image: &mut Option<SharedImage>,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let image_index = result.images.len();
    let node_count = screenshot.accessibility["elements"]
        .as_array()
        .map_or(0, Vec::len);
    let observation_id = screenshot.observation.observation_id.clone();
    let mut post_snapshot = json!({
        "success": true,
        "observation_id": observation_id,
        "accessibility_state_id": observation_id,
        "observation": screenshot.observation,
        "root": screenshot.accessibility,
        "node_count": node_count,
    });
    result.images.push(ComputerUseImage {
        data: screenshot.data,
        mime_type: "image/png".into(),
    });
    let (mut response, attachment) = action_completed_response(
        session_id,
        action_id,
        "CUA action completed with a fresh post-action snapshot",
        result,
        mode,
        shared_image,
    )?;
    if let Some(descriptor) = response_image_descriptor(&response, image_index) {
        post_snapshot["image"] = descriptor;
    }
    response["post_snapshot"] = post_snapshot;
    Ok((response, attachment))
}

pub(super) fn desktop_action_completed_with_snapshot_response(
    session_id: &str,
    action_id: String,
    mut result: ComputerUseToolResult,
    snapshot: ComputerUseDesktopSnapshot,
    mode: SnapshotTransport,
    shared_image: &mut Option<SharedImage>,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let image_index = result.images.len();
    let mut post_snapshot = json!({
        "success": true,
        "observation_id": snapshot.observation_id,
        "state": snapshot.state,
    });
    result.images.push(ComputerUseImage {
        data: snapshot.data,
        mime_type: "image/png".into(),
    });
    let (mut response, attachment) = action_completed_response(
        session_id,
        action_id,
        "desktop CUA action completed with a fresh post-action snapshot",
        result,
        mode,
        shared_image,
    )?;
    if let Some(descriptor) = response_image_descriptor(&response, image_index) {
        post_snapshot["image"] = descriptor;
    }
    response["post_snapshot"] = post_snapshot;
    Ok((response, attachment))
}
