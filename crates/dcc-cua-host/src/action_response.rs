use super::*;

pub(super) trait TransportImage {
    fn into_transport_parts(self) -> (Vec<u8>, String);
}

impl TransportImage for ComputerUseImage {
    fn into_transport_parts(self) -> (Vec<u8>, String) {
        (self.data, self.mime_type)
    }
}

impl TransportImage for dcc_cua_browser::BrowserImage {
    fn into_transport_parts(self) -> (Vec<u8>, String) {
        (self.data, self.mime_type)
    }
}

#[derive(Debug)]
pub(super) struct PreparedImageTransport {
    pub(super) primary: Option<Value>,
    pub(super) attachments: Vec<Value>,
    pub(super) attachment: Option<Vec<u8>>,
    pub(super) use_shared_memory: bool,
}

impl PreparedImageTransport {
    pub(super) fn annotate_content(&self, value: &mut Value) {
        let Some(content) = value.get_mut("content").and_then(Value::as_array_mut) else {
            return;
        };
        for (index, item) in content
            .iter_mut()
            .filter(|item| item["type"] == "image")
            .enumerate()
        {
            let Some(descriptor) = self.attachments.get(index) else {
                break;
            };
            item["data"] = Value::Null;
            item["encoding"] = descriptor["encoding"].clone();
            item["attachment_index"] = json!(index);
            item["offset"] = descriptor["offset"].clone();
            item["length"] = descriptor["length"].clone();
        }
    }
}

pub(super) fn prepare_image_transport<T: TransportImage>(
    images: Vec<T>,
    mode: SnapshotTransport,
    shared_image: &mut Option<SharedImage>,
) -> Result<PreparedImageTransport, HostError> {
    prepare_image_transport_with_limit(images, mode, shared_image, MAX_BINARY_FRAME_BYTES)
}

pub(super) fn prepare_image_transport_with_limit<T: TransportImage>(
    images: Vec<T>,
    mode: SnapshotTransport,
    shared_image: &mut Option<SharedImage>,
    binary_frame_limit: usize,
) -> Result<PreparedImageTransport, HostError> {
    let mut images = images
        .into_iter()
        .map(TransportImage::into_transport_parts)
        .collect::<Vec<_>>();
    let use_shared_memory = mode == SnapshotTransport::SharedMemory && images.len() == 1;
    let combined_length = if use_shared_memory {
        0
    } else {
        images.iter().try_fold(0_usize, |total, (data, _)| {
            total.checked_add(data.len()).ok_or_else(|| {
                HostError::Protocol("combined image payload length overflowed".into())
            })
        })?
    };
    if combined_length > binary_frame_limit {
        return Err(HostError::Protocol(format!(
            "combined image payload exceeds the {binary_frame_limit}-byte binary frame limit"
        )));
    }

    let mut offset = 0_usize;
    let attachments = images
        .iter()
        .enumerate()
        .map(|(index, (data, mime_type))| {
            let descriptor = json!({
                "index": index,
                "offset": offset,
                "length": data.len(),
                "mime_type": mime_type,
                "encoding": if use_shared_memory { "shared_memory" } else { "binary_frame" },
            });
            offset += data.len();
            descriptor
        })
        .collect::<Vec<_>>();

    let primary = if use_shared_memory {
        let (data, mime_type) = &images[0];
        let shared = SharedImage::from_bytes(data, mime_type)
            .map_err(|error| HostError::Protocol(error.to_string()))?;
        let mut descriptor = serde_json::to_value(shared.descriptor())
            .map_err(|error| HostError::Protocol(error.to_string()))?;
        descriptor["encoding"] = Value::String("shared_memory".into());
        *shared_image = Some(shared);
        Some(descriptor)
    } else {
        attachments.first().cloned()
    };

    let attachment = if use_shared_memory || images.is_empty() {
        None
    } else if images.len() == 1 {
        Some(images.pop().expect("one image was checked").0)
    } else {
        let mut combined = Vec::with_capacity(combined_length);
        for (data, _) in images {
            combined.extend_from_slice(&data);
        }
        Some(combined)
    };

    Ok(PreparedImageTransport {
        primary,
        attachments,
        attachment,
        use_shared_memory,
    })
}

pub(super) fn native_tool_response_with_transport(
    session_id: Option<&str>,
    tool: &str,
    result: ComputerUseToolResult,
    mode: SnapshotTransport,
    shared_image: &mut Option<SharedImage>,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let success = result.status == ComputerUseToolStatus::Succeeded;
    let mut value = result.value;
    value["success"] = Value::Bool(success);
    let prepared = prepare_image_transport(result.images, mode, shared_image)?;
    prepared.annotate_content(&mut value);
    let mut response = json!({
        "type": "tool_result",
        "session_id": session_id,
        "tool": tool,
        "status": result.status,
        "result": value,
        "text": result.text,
        "degraded": result.degraded,
    });
    if let Some(session_id) = session_id {
        response["session_id"] = Value::String(session_id.to_owned());
    }
    if let Some(image) = prepared.primary {
        response["image"] = image;
        if !prepared.use_shared_memory {
            response["attachments"] = Value::Array(prepared.attachments);
        }
    }
    Ok((response, prepared.attachment))
}

pub(super) fn action_completed_response(
    session_id: &str,
    action_id: String,
    message: &str,
    result: ComputerUseToolResult,
    mode: SnapshotTransport,
    shared_image: &mut Option<SharedImage>,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let success = result.status == ComputerUseToolStatus::Succeeded;
    let (tool_response, attachment) = native_tool_response_with_transport(
        Some(session_id),
        "action",
        result,
        mode,
        shared_image,
    )?;
    let mut response = json!({
        "type": "action_completed",
        "success": success,
        "status": tool_response["status"].clone(),
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
