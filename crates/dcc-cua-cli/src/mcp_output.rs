//! Optional MCP `CallToolResult` projection for Host JSONL consumers.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum HostJsonlResponseFormat {
    #[default]
    Host,
    Mcp,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct HostJsonlImageMetrics {
    pub(super) images_total: u64,
    pub(super) pixels_total: u64,
    pub(super) encoded_bytes_total: u64,
    pub(super) unknown_dimensions_total: u64,
}

pub(super) struct JsonlResponseOutput {
    pub(super) value: Value,
    pub(super) image_metrics: HostJsonlImageMetrics,
}

impl HostJsonlResponseFormat {
    pub(super) fn parse(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("host") => Ok(Self::Host),
            Some("mcp") => Ok(Self::Mcp),
            Some(value) => Err(format!(
                "--response-format must be host or mcp, got {value}"
            )),
        }
    }
}

pub(super) fn format_value(
    value: Value,
    response_format: HostJsonlResponseFormat,
) -> Result<Value, String> {
    match response_format {
        HostJsonlResponseFormat::Host => Ok(value),
        HostJsonlResponseFormat::Mcp => call_tool_result(value, None),
    }
}

pub(super) fn call_tool_result(
    structured_content: Value,
    attachment_bytes: Option<&[u8]>,
) -> Result<Value, String> {
    let images = native_images(&structured_content, attachment_bytes)?;
    let is_error = structured_content["type"] == "error"
        || structured_content
            .pointer("/result/isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let text = response_text(&structured_content, !images.is_empty(), is_error);
    let mut content = vec![json!({"type": "text", "text": text})];
    content.extend(images);
    Ok(json!({
        "content": content,
        "structuredContent": structured_content,
        "isError": is_error,
    }))
}

pub(super) fn output_error_value(message: String, request_id: Option<&Value>) -> Value {
    let mut value = json!({
        "type": "error",
        "code": "output_error",
        "message": message,
    });
    if let Some(request_id) = request_id {
        value["request_id"] = request_id.clone();
    }
    value
}

pub(super) fn response_image_metrics(value: &Value, bytes: Option<&[u8]>) -> HostJsonlImageMetrics {
    let Some(bytes) = bytes else {
        return HostJsonlImageMetrics::default();
    };
    let mut metrics = HostJsonlImageMetrics::default();
    let attachments = value.get("attachments").and_then(Value::as_array);
    if let Some(attachments) = attachments.filter(|attachments| !attachments.is_empty()) {
        for attachment in attachments {
            let Some(offset) = attachment
                .get("offset")
                .and_then(Value::as_u64)
                .and_then(|offset| usize::try_from(offset).ok())
            else {
                continue;
            };
            let Some(length) = attachment
                .get("length")
                .and_then(Value::as_u64)
                .and_then(|length| usize::try_from(length).ok())
            else {
                continue;
            };
            let Some(end) = offset.checked_add(length) else {
                continue;
            };
            let Some(image) = bytes.get(offset..end) else {
                continue;
            };
            record_encoded_image(&mut metrics, image);
        }
    } else if value.get("image").is_some() {
        let length = value["image"]
            .get("length")
            .and_then(Value::as_u64)
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(bytes.len());
        if let Some(image) = bytes.get(..length) {
            record_encoded_image(&mut metrics, image);
        }
    }
    metrics
}

fn record_encoded_image(metrics: &mut HostJsonlImageMetrics, image: &[u8]) {
    metrics.images_total = metrics.images_total.saturating_add(1);
    metrics.encoded_bytes_total = metrics
        .encoded_bytes_total
        .saturating_add(image.len().try_into().unwrap_or(u64::MAX));
    if let Some(pixels) = png_pixel_count(image) {
        metrics.pixels_total = metrics.pixels_total.saturating_add(pixels);
    } else {
        metrics.unknown_dimensions_total = metrics.unknown_dimensions_total.saturating_add(1);
    }
}

fn png_pixel_count(image: &[u8]) -> Option<u64> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if image.len() < 24 || image.get(..8)? != PNG_SIGNATURE || image.get(12..16)? != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(image.get(16..20)?.try_into().ok()?);
    let height = u32::from_be_bytes(image.get(20..24)?.try_into().ok()?);
    u64::from(width).checked_mul(u64::from(height))
}

fn native_images(value: &Value, bytes: Option<&[u8]>) -> Result<Vec<Value>, String> {
    let descriptors = image_descriptors(value);
    if descriptors.is_empty() {
        if bytes.is_some() {
            return Err("Host response supplied image bytes without an image descriptor".into());
        }
        return Ok(Vec::new());
    }
    let bytes = bytes.ok_or_else(|| {
        "Host response advertised image content without readable attachment bytes".to_owned()
    })?;
    descriptors
        .into_iter()
        .map(|descriptor| native_image(descriptor, bytes))
        .collect()
}

fn image_descriptors(value: &Value) -> Vec<&Value> {
    if let Some(attachments) = value["attachments"].as_array()
        && !attachments.is_empty()
    {
        return attachments.iter().collect();
    }
    value.get("image").into_iter().collect()
}

fn native_image(descriptor: &Value, bytes: &[u8]) -> Result<Value, String> {
    let offset = descriptor["offset"].as_u64().map_or(Ok(0), |value| {
        usize::try_from(value).map_err(|_| "image offset is too large")
    })?;
    let default_length = bytes.len().saturating_sub(offset);
    let length = descriptor["length"]
        .as_u64()
        .map_or(Ok(default_length), |value| {
            usize::try_from(value).map_err(|_| "image length is too large")
        })?;
    let end = offset
        .checked_add(length)
        .ok_or_else(|| "image attachment range overflowed".to_owned())?;
    let image = bytes.get(offset..end).ok_or_else(|| {
        format!(
            "image attachment range {offset}..{end} exceeds {} bytes",
            bytes.len()
        )
    })?;
    let mime_type = descriptor["mime_type"]
        .as_str()
        .or_else(|| descriptor["mimeType"].as_str())
        .unwrap_or("image/png");
    Ok(json!({
        "type": "image",
        "data": BASE64_STANDARD.encode(image),
        "mimeType": mime_type,
    }))
}

fn response_text(value: &Value, has_images: bool, is_error: bool) -> String {
    if is_error && let Some(message) = value["message"].as_str() {
        return message.to_owned();
    }
    if let Some(text) = nested_text_content(value) {
        return text;
    }
    if let Some(text) = value["text"].as_str().filter(|text| !text.is_empty()) {
        return text.to_owned();
    }
    if has_images {
        return match value["type"].as_str() {
            Some("snapshot") => "Captured exact-window DCC-CUA snapshot.",
            Some("desktop_snapshot" | "desktop_session_snapshot") => {
                "Captured DCC-CUA desktop snapshot."
            }
            Some("action_completed" | "desktop_action_completed") => {
                "DCC-CUA action completed with a fresh post-action snapshot."
            }
            _ => "DCC-CUA returned native image content.",
        }
        .to_owned();
    }
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn nested_text_content(value: &Value) -> Option<String> {
    let content = value.pointer("/result/content")?.as_array()?;
    let texts = content
        .iter()
        .filter(|item| item["type"] == "text")
        .filter_map(|item| item["text"].as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    (!texts.is_empty()).then(|| texts.join("\n"))
}
