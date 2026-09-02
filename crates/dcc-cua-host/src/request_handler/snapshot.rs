use super::*;

pub(super) async fn snapshot_window(
    host: &mut HostSession,
    mode: SnapshotTransport,
    max_depth: u32,
    max_nodes: u32,
    activate_before: bool,
    pixels_only: bool,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let activation = if activate_before {
        let activation = host.session.activate().await;
        Some(host.finish_observation_sensitive_attempt(activation)?)
    } else {
        None
    };
    let screenshot = if pixels_only {
        host.session.screenshot_pixels_only().await
    } else {
        host.session
            .screenshot_with_bounds(max_nodes, max_depth)
            .await
    };
    let screenshot = host.finish_observation_sensitive_attempt(screenshot)?;
    let observation_id = screenshot.observation.observation_id.clone();
    host.latest_observation_id = Some(observation_id.clone());
    host.latest_accessibility_state_id = Some(observation_id.clone());
    let accessibility = screenshot.accessibility;
    host.latest_accessibility_root = Some(accessibility.clone());
    let node_count = accessibility["elements"].as_array().map_or(0, Vec::len);
    let target = json!({
        "process_id": screenshot.observation.process_id,
        "window_handle": screenshot.observation.window_handle,
        "window_title": screenshot.observation.window_title,
    });
    let (image, attachment) = match mode {
        SnapshotTransport::SharedMemory => {
            let shared = SharedImage::from_bytes(&screenshot.data, "image/png")
                .map_err(|error| HostError::Protocol(error.to_string()))?;
            let mut descriptor = serde_json::to_value(shared.descriptor())
                .map_err(|error| HostError::Protocol(error.to_string()))?;
            descriptor["encoding"] = Value::String("shared_memory".into());
            host.latest_shared_image = Some(shared);
            (descriptor, None)
        }
        SnapshotTransport::BinaryFrame => (
            json!({
                "name": "",
                "id": screenshot.observation.observation_id,
                "length": screenshot.data.len(),
                "mime_type": "image/png",
                "encoding": "binary_frame",
            }),
            Some(screenshot.data),
        ),
    };
    let response = json!({
        "type": "snapshot",
        "observation_id": observation_id,
        "accessibility_state_id": screenshot.observation.observation_id,
        "target": target,
        "observation": screenshot.observation,
        "root": accessibility,
        "node_count": node_count,
        "image": image,
        "activation": activation,
        "observation_mode": if pixels_only { "pixels_only" } else { "accessibility_preferred" },
    });
    Ok((response, attachment))
}
