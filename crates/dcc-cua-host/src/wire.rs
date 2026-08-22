//! Bounded Host framing, request correlation, and wire error mapping.

use std::sync::Arc;

use dcc_cua_core::ComputerUseErrorCode;
use dcc_cua_protocol::{FrameError, RequestEnvelope, validate_request_id};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex as AsyncMutex;

use super::{HostError, MAX_BINARY_FRAME_BYTES, MAX_JSON_FRAME_BYTES, Request};

pub(super) fn target_wire(target: &Value) -> Value {
    json!({
        "process_id": target["pid"],
        "window_handle": target["window_id"],
        "window_title": target["title"],
    })
}

pub(super) fn error_code(error: &HostError) -> &'static str {
    match error {
        HostError::ComputerUse(error) => match error.code {
            ComputerUseErrorCode::StaleObservation => "stale_observation",
            ComputerUseErrorCode::UserInterrupted => "user_interrupted",
            ComputerUseErrorCode::InvalidTarget => "invalid_target",
            ComputerUseErrorCode::TargetMinimized => "target_minimized",
            ComputerUseErrorCode::TargetUnavailable => "target_unavailable",
            ComputerUseErrorCode::TargetModalChanged => "target_modal_changed",
            ComputerUseErrorCode::BrowserRefused => "browser_refused",
            ComputerUseErrorCode::ClipboardRefused => "clipboard_refused",
            ComputerUseErrorCode::RecordingRefused => "recording_refused",
            ComputerUseErrorCode::CaptureFailed => "capture_failed",
            ComputerUseErrorCode::InteractiveDesktopUnavailable => {
                "interactive_desktop_unavailable"
            }
            ComputerUseErrorCode::InputFailed => "input_failed",
            ComputerUseErrorCode::SessionRefreshRequired => "session_refresh_required",
            ComputerUseErrorCode::CompletionUnknown => "completion_unknown",
            ComputerUseErrorCode::InvalidAction => "invalid_request",
            ComputerUseErrorCode::MissingWindow => "target_unavailable",
            ComputerUseErrorCode::BackendUnavailable => "backend_unavailable",
            ComputerUseErrorCode::ForegroundActivationRefused => "foreground_activation_refused",
        },
        HostError::Io(_) | HostError::EndpointHijacked { .. } => "backend_unavailable",
        HostError::CodedProtocol { code, .. } => code.as_wire_code(),
        HostError::Protocol(_) => "invalid_request",
    }
}

pub(super) fn error_response(code: &str, message: impl Into<String>) -> Value {
    json!({"type":"error", "code":code, "message":message.into()})
}

pub(super) fn host_error_response(error: &HostError) -> Value {
    let mut response = error_response(error_code(error), error.to_string());
    if let HostError::ComputerUse(error) = error
        && let Some(details) = &error.details
    {
        response["details"] =
            serde_json::to_value(details).expect("ComputerUseErrorDetails must serialize");
    }
    response
}

pub(super) fn parse_request_frame(
    frame: &[u8],
) -> Result<(Option<String>, Request), (Option<String>, String)> {
    let envelope = match serde_json::from_slice::<Value>(frame) {
        Ok(envelope) => envelope,
        Err(error) => return Err((None, error.to_string())),
    };
    let request_id = match request_id_from(&envelope) {
        Ok(request_id) => request_id,
        Err(error) => return Err((None, error)),
    };
    if let Err(error) = RequestEnvelope::from_value(&envelope) {
        return Err((request_id, error.to_string()));
    }
    serde_json::from_value(envelope)
        .map(|request| (request_id.clone(), request))
        .map_err(|error| (request_id, error.to_string()))
}

pub(super) fn request_id_from(value: &Value) -> Result<Option<String>, String> {
    let Some(request_id) = value.get("request_id") else {
        return Ok(None);
    };
    let request_id = request_id
        .as_str()
        .ok_or_else(|| "request_id must be a string".to_owned())?;
    validate_request_id(request_id).map_err(|error| error.to_string())?;
    Ok(Some(request_id.to_owned()))
}

pub(super) fn with_request_id(mut response: Value, request_id: Option<&str>) -> Value {
    if let Some(request_id) = request_id {
        response["request_id"] = Value::String(request_id.to_owned());
    }
    response
}

pub(super) async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> Result<Option<Vec<u8>>, HostError> {
    dcc_cua_protocol::read_frame(reader, max)
        .await
        .map_err(frame_error)
}

pub(super) async fn write_json_locked<W: AsyncWrite + Unpin>(
    writer: &Arc<AsyncMutex<W>>,
    value: Value,
) -> Result<(), HostError> {
    let mut writer = writer.lock().await;
    write_json(&mut *writer, value).await
}

pub(super) async fn write_response_locked<W: AsyncWrite + Unpin>(
    writer: &Arc<AsyncMutex<W>>,
    value: Value,
    attachment: Option<&[u8]>,
) -> Result<(), HostError> {
    let mut writer = writer.lock().await;
    write_response(&mut *writer, value, attachment).await
}

pub(super) async fn write_json<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: Value,
) -> Result<(), HostError> {
    let body =
        serde_json::to_vec(&value).map_err(|error| HostError::Protocol(error.to_string()))?;
    write_frame(writer, &body, MAX_JSON_FRAME_BYTES).await
}

pub(super) async fn write_response<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: Value,
    attachment: Option<&[u8]>,
) -> Result<(), HostError> {
    let body =
        serde_json::to_vec(&value).map_err(|error| HostError::Protocol(error.to_string()))?;
    write_frame_unflushed(writer, &body, MAX_JSON_FRAME_BYTES).await?;
    if let Some(bytes) = attachment {
        write_frame_unflushed(writer, bytes, MAX_BINARY_FRAME_BYTES).await?;
    }
    writer.flush().await?;
    Ok(())
}

pub(super) async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    body: &[u8],
    max: usize,
) -> Result<(), HostError> {
    dcc_cua_protocol::write_frame(writer, body, max)
        .await
        .map_err(frame_error)
}

pub(super) async fn write_frame_unflushed<W: AsyncWrite + Unpin>(
    writer: &mut W,
    body: &[u8],
    max: usize,
) -> Result<(), HostError> {
    dcc_cua_protocol::write_frame_unflushed(writer, body, max)
        .await
        .map_err(frame_error)
}

fn frame_error(error: FrameError) -> HostError {
    match error {
        FrameError::Io(error) => HostError::Io(error),
        FrameError::Protocol(message) => HostError::Protocol(message),
    }
}
