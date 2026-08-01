//! Long-lived local host IPC for dcc-mcp-core.
//!
//! The wire format intentionally matches Core's existing UI Control framing:
//! big-endian `u32` length followed by one UTF-8 JSON object. Screenshot bytes
//! are sent as a second framed payload, so the control frame stays bounded and
//! the transport does not base64-encode pixels.

use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
#[cfg(unix)]
use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

use crate::{
    ComputerUseAction, ComputerUseDriver, ComputerUseError, ComputerUseErrorCode, ComputerUsePoint,
    ComputerUseResult, ComputerUseSession, ComputerUseTargetScope,
};

/// Core-compatible control frame limit. Pixel bytes use a separate bounded frame.
pub const MAX_JSON_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_BINARY_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub const HOST_PROTOCOL_VERSION: u32 = 3;

/// Capabilities this implementation actually provides.
pub const HOST_CAPABILITIES: &[&str] = &[
    "exact_window_capabilities",
    "exact_window_state",
    "connection_scoped_sessions",
    "observation_fencing",
    "scoped_raw_input",
    "accessibility_snapshot",
    "uia_snapshot_and_actions",
    "binary_snapshot_frames",
    "cua_cursor_marker",
    "cross_platform_window_control",
];

/// Local transport selected by the CLI or embedding host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostTransport {
    Stdio,
    Endpoint(String),
}

impl HostTransport {
    #[must_use]
    pub fn default_endpoint() -> String {
        if cfg!(windows) {
            r"\\.\pipe\dcc-mcp-computer-use-v1".to_owned()
        } else {
            std::env::temp_dir()
                .join("dcc-mcp-computer-use-v1.sock")
                .to_string_lossy()
                .into_owned()
        }
    }
}

#[derive(Debug, Error)]
pub enum HostError {
    #[error("host transport failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("host protocol failed: {0}")]
    Protocol(String),
    #[error("{0}")]
    ComputerUse(#[from] ComputerUseError),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
enum Request {
    Hello(HelloParams),
    OpenSession {
        session_id: String,
        grant: TaskGrant,
    },
    GetWindowState {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
    },
    ChangeWindowState {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        operation: WindowOperation,
    },
    Snapshot {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        #[allow(dead_code)]
        max_depth: u32,
        #[allow(dead_code)]
        max_nodes: u32,
    },
    AccessibilitySnapshot {
        #[allow(dead_code)]
        session_id: String,
        #[allow(dead_code)]
        task_grant_id: String,
        #[allow(dead_code)]
        window_capability: String,
        #[allow(dead_code)]
        max_depth: u32,
        #[allow(dead_code)]
        max_nodes: u32,
    },
    ExecuteAction {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        observation_id: String,
        accessibility_state_id: String,
        action: HostAction,
    },
    ResumeSession {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
    },
    StopSession {
        session_id: String,
    },
}

#[derive(Debug, Deserialize)]
struct HelloParams {
    protocol_version: u32,
    #[allow(dead_code)]
    client_name: String,
}

#[derive(Debug, Deserialize)]
struct TaskGrant {
    task_grant_id: String,
    dcc_type: String,
    #[serde(default)]
    process_id: Option<u32>,
    #[serde(default)]
    window_handle: Option<u64>,
    #[serde(default)]
    allow_raw_input: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WindowOperation {
    Restore,
    Show,
    Activate,
}

#[derive(Debug, Deserialize)]
struct HostAction {
    action: String,
    #[serde(default)]
    control_id: Option<String>,
    #[serde(default)]
    element_index: Option<u32>,
    input_kind: String,
    intent: String,
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
    #[serde(default)]
    button: Option<String>,
    #[serde(default)]
    scroll_x: Option<i32>,
    #[serde(default)]
    scroll_y: Option<i32>,
    #[serde(default)]
    path: Vec<ComputerUsePoint>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    keys: Vec<String>,
    #[allow(dead_code)]
    #[serde(default)]
    duration_ms: Option<u64>,
}

impl HostAction {
    fn reject_policy(&self) -> Option<(&'static str, &'static str)> {
        const HARD_DENY: [&str; 6] = [
            "terminal_or_run_dialog",
            "credential_or_authentication",
            "windows_security_or_privacy",
            "safety_bypass",
            "password_change",
            "escape_scope",
        ];
        HARD_DENY
            .iter()
            .find(|intent| intent == &&self.intent)
            .map(|_| {
                (
                    "hard_denied",
                    "the host policy denies this Computer Use intent",
                )
            })
    }

    fn requires_approval(&self) -> bool {
        !matches!(
            self.intent.as_str(),
            "observe" | "activate" | "navigate" | "ordinary_edit"
        )
    }

    fn into_computer_use(self, observation_id: String) -> ComputerUseResult<ComputerUseAction> {
        if !matches!(self.input_kind.as_str(), "raw_input" | "semantic") {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "input_kind must be raw_input or semantic",
            ));
        }
        if self.input_kind != "raw_input" && self.element_index.is_none() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "semantic actions require CUA element_index",
            ));
        }
        if self.control_id.is_some() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "control_id requires a semantic UI backend",
            ));
        }
        Ok(ComputerUseAction {
            action: self.action,
            observation_id: Some(observation_id),
            element_index: self.element_index,
            x: self.x,
            y: self.y,
            button: self.button,
            scroll_x: self.scroll_x,
            scroll_y: self.scroll_y,
            path: self.path,
            text: self.text,
            keys: self.keys,
        })
    }
}

struct HostSession {
    task_grant_id: String,
    allow_raw_input: bool,
    capability: String,
    session: ComputerUseSession,
    latest_observation_id: Option<String>,
    latest_accessibility_state_id: Option<String>,
}

/// Run one long-lived host connection over stdio or the platform local endpoint.
pub async fn run(driver: ComputerUseDriver, transport: HostTransport) -> Result<(), HostError> {
    match transport {
        HostTransport::Stdio => {
            process_connection_parts(driver, tokio::io::stdin(), tokio::io::stdout()).await
        }
        HostTransport::Endpoint(endpoint) => serve_endpoint(driver, endpoint).await,
    }
}

async fn process_connection<S>(driver: ComputerUseDriver, stream: S) -> Result<(), HostError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, writer) = tokio::io::split(stream);
    process_connection_parts(driver, reader, writer).await
}

async fn process_connection_parts<R, W>(
    driver: ComputerUseDriver,
    reader: R,
    writer: W,
) -> Result<(), HostError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = reader;
    let mut writer = writer;
    let mut handshaken = false;
    let mut sessions = HashMap::<String, HostSession>::new();

    while let Some(frame) = read_frame(&mut reader, MAX_JSON_FRAME_BYTES).await? {
        let request = match serde_json::from_slice::<Request>(&frame) {
            Ok(request) => request,
            Err(error) => {
                write_json(
                    &mut writer,
                    error_response("invalid_request", error.to_string()),
                )
                .await?;
                continue;
            }
        };
        let (response, attachment) =
            match handle_request(&driver, &mut sessions, &mut handshaken, request).await {
                Ok(result) => result,
                Err(error) => (error_response(error_code(&error), error.to_string()), None),
            };
        write_json(&mut writer, response).await?;
        if let Some(bytes) = attachment {
            write_frame(&mut writer, &bytes, MAX_BINARY_FRAME_BYTES).await?;
        }
    }

    for (_, mut session) in sessions {
        let _ = session.session.stop().await;
    }
    Ok(())
}

async fn handle_request(
    driver: &ComputerUseDriver,
    sessions: &mut HashMap<String, HostSession>,
    handshaken: &mut bool,
    request: Request,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    if let Request::Hello(params) = &request {
        if params.protocol_version != HOST_PROTOCOL_VERSION {
            return Err(HostError::Protocol(format!(
                "protocol version {} is not supported",
                params.protocol_version
            )));
        }
        *handshaken = true;
        return Ok((
            json!({
                "type": "hello",
                "protocol_version": HOST_PROTOCOL_VERSION,
                "capabilities": HOST_CAPABILITIES,
            }),
            None,
        ));
    }
    if !*handshaken {
        return Err(HostError::Protocol(
            "hello is required before stateful requests".into(),
        ));
    }

    match request {
        Request::Hello(_) => unreachable!(),
        Request::OpenSession { session_id, grant } => {
            if sessions.contains_key(&session_id) {
                return Err(HostError::Protocol("session already exists".into()));
            }
            if grant.task_grant_id.trim().is_empty() || grant.dcc_type.trim().is_empty() {
                return Err(HostError::Protocol("task grant is incomplete".into()));
            }
            let scope = ComputerUseTargetScope {
                process_id: grant.process_id,
                window_handle: grant.window_handle,
                window_title: None,
            };
            let mut session = driver.session(scope, grant.dcc_type.clone(), session_id.clone())?;
            session.start().await?;
            let target = session
                .target()
                .ok_or_else(|| HostError::Protocol("CUA did not return a target".into()))?;
            let marker = session.status()["marker"].clone();
            let capability = format!("cua-window-{}", Uuid::new_v4());
            sessions.insert(
                session_id.clone(),
                HostSession {
                    task_grant_id: grant.task_grant_id,
                    allow_raw_input: grant.allow_raw_input,
                    capability: capability.clone(),
                    session,
                    latest_observation_id: None,
                    latest_accessibility_state_id: None,
                },
            );
            Ok((
                json!({
                    "type": "session_opened",
                    "session_id": session_id,
                    "window_capability": capability,
                    "target": target_wire(&target),
                    "marker": marker,
                }),
                None,
            ))
        }
        Request::GetWindowState {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            let state = host.session.window_state().await?;
            Ok((
                json!({"type":"window_state", "session_id":session_id, "state":state}),
                None,
            ))
        }
        Request::ChangeWindowState {
            session_id,
            task_grant_id,
            window_capability,
            operation,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !matches!(operation, WindowOperation::Activate) {
                return Err(HostError::Protocol(
                    "CUA currently supports activate; restore/show are platform-specific".into(),
                ));
            }
            host.session.activate().await?;
            let state = host.session.window_state().await?;
            Ok((
                json!({
                    "type":"window_state_changed",
                    "session_id":session_id,
                    "operation":"activate",
                    "state":state,
                }),
                None,
            ))
        }
        Request::Snapshot {
            session_id,
            task_grant_id,
            window_capability,
            ..
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            let screenshot = host.session.screenshot().await?;
            let observation_id = screenshot.observation.observation_id.clone();
            host.latest_observation_id = Some(observation_id.clone());
            host.latest_accessibility_state_id = Some(observation_id.clone());
            let accessibility = screenshot.accessibility;
            let node_count = accessibility["elements"].as_array().map_or(0, Vec::len);
            let target = json!({
                "process_id": screenshot.observation.process_id,
                "window_handle": screenshot.observation.window_handle,
                "window_title": screenshot.observation.window_title,
            });
            let response = json!({
                "type": "snapshot",
                "observation_id": observation_id,
                "accessibility_state_id": screenshot.observation.observation_id,
                "target": target,
                "observation": screenshot.observation,
                "root": accessibility,
                "node_count": node_count,
                "image": {
                    "name": "",
                    "id": screenshot.observation.observation_id,
                    "length": screenshot.data.len(),
                    "mime_type": "image/png",
                    "encoding": "binary_frame",
                },
            });
            Ok((response, Some(screenshot.data)))
        }
        Request::AccessibilitySnapshot {
            session_id,
            task_grant_id,
            window_capability,
            max_depth,
            max_nodes,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            let root = host
                .session
                .accessibility_snapshot(max_nodes, max_depth)
                .await?;
            let state_id = format!("{}-accessibility-{}", session_id, Uuid::new_v4());
            host.latest_accessibility_state_id = Some(state_id.clone());
            let target = host
                .session
                .target()
                .ok_or_else(|| HostError::Protocol("CUA did not return a target".into()))?;
            Ok((
                json!({
                    "type":"accessibility_snapshot",
                    "accessibility_state_id":state_id,
                    "target":target_wire(&target),
                    "root":root,
                    "node_count":root["elements"].as_array().map_or(0, Vec::len),
                }),
                None,
            ))
        }
        Request::ExecuteAction {
            session_id,
            task_grant_id,
            window_capability,
            observation_id,
            accessibility_state_id,
            action,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if action.input_kind == "raw_input" && !host.allow_raw_input {
                return Err(HostError::Protocol("raw input is not granted".into()));
            }
            if let Some((code, message)) = action.reject_policy() {
                return Ok((
                    json!({
                        "type":"action_completed",
                        "success":false,
                        "policy_tier":code,
                        "message":message,
                        "error":code,
                    }),
                    None,
                ));
            }
            if action.requires_approval() {
                return Ok((
                    json!({
                        "type":"action_completed",
                        "success":false,
                        "policy_tier":"action_confirmation",
                        "message":"trusted action-time confirmation is required",
                        "error":"approval_required",
                    }),
                    None,
                ));
            }
            if host.latest_observation_id.as_deref() != Some(observation_id.as_str()) {
                return Err(HostError::ComputerUse(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "action observation_id does not match the latest host snapshot",
                )));
            }
            if action.element_index.is_some()
                && host.latest_accessibility_state_id.as_deref()
                    != Some(accessibility_state_id.as_str())
            {
                return Err(HostError::ComputerUse(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "semantic action requires the latest accessibility_state_id",
                )));
            }
            let action = action.into_computer_use(observation_id)?;
            let result = host.session.perform_action(&action).await?;
            host.latest_observation_id = None;
            host.latest_accessibility_state_id = None;
            Ok((
                json!({
                    "type":"action_completed",
                    "success":true,
                    "action_id":format!("cua-action-{}", Uuid::new_v4()),
                    "target_closed":false,
                    "policy_tier":"task_grant",
                    "message":"CUA action completed",
                    "result":result,
                }),
                None,
            ))
        }
        Request::ResumeSession {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            host.session.resume_after_user_approval().await?;
            Ok((
                json!({"type":"session_resumed", "session_id":session_id}),
                None,
            ))
        }
        Request::StopSession { session_id } => {
            let mut host = sessions
                .remove(&session_id)
                .ok_or_else(|| HostError::Protocol("session not found".into()))?;
            let result = host.session.stop().await?;
            Ok((
                json!({"type":"session_stopped", "session_id":session_id, "cleanup_pending":result["cleanup_pending"].as_bool().unwrap_or(false)}),
                None,
            ))
        }
    }
}

fn authorized_session<'a>(
    sessions: &'a mut HashMap<String, HostSession>,
    session_id: &str,
    grant_id: &str,
    capability: &str,
) -> Result<&'a mut HostSession, HostError> {
    let session = sessions
        .get_mut(session_id)
        .ok_or_else(|| HostError::Protocol("session not found".into()))?;
    if session.task_grant_id != grant_id || session.capability != capability {
        return Err(HostError::Protocol(
            "session grant or capability mismatch".into(),
        ));
    }
    Ok(session)
}

fn target_wire(target: &Value) -> Value {
    json!({
        "process_id": target["pid"],
        "window_handle": target["window_id"],
        "window_title": target["title"],
    })
}

fn error_code(error: &HostError) -> &'static str {
    match error {
        HostError::ComputerUse(error) => match error.code {
            ComputerUseErrorCode::StaleObservation => "stale_observation",
            ComputerUseErrorCode::UserInterrupted => "user_interrupted",
            ComputerUseErrorCode::InvalidTarget => "invalid_target",
            ComputerUseErrorCode::CaptureFailed => "capture_failed",
            ComputerUseErrorCode::InputFailed => "input_failed",
            ComputerUseErrorCode::InvalidAction => "invalid_request",
            ComputerUseErrorCode::MissingWindow => "invalid_target",
            ComputerUseErrorCode::BackendUnavailable => "backend_unavailable",
        },
        HostError::Io(_) => "backend_unavailable",
        HostError::Protocol(message) if message.contains("version") => "protocol_mismatch",
        HostError::Protocol(message) if message.contains("raw input") => "raw_input_not_granted",
        HostError::Protocol(message) if message.contains("accessibility") => "unsupported",
        HostError::Protocol(_) => "invalid_request",
    }
}

fn error_response(code: &str, message: impl Into<String>) -> Value {
    json!({"type":"error", "code":code, "message":message.into()})
}

async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> Result<Option<Vec<u8>>, HostError> {
    let mut prefix = [0_u8; 4];
    match reader.read_exact(&mut prefix).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > max {
        return Err(HostError::Protocol(format!(
            "frame length {length} exceeds the host limit"
        )));
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).await?;
    Ok(Some(body))
}

async fn write_json<W: AsyncWrite + Unpin>(writer: &mut W, value: Value) -> Result<(), HostError> {
    let body =
        serde_json::to_vec(&value).map_err(|error| HostError::Protocol(error.to_string()))?;
    write_frame(writer, &body, MAX_JSON_FRAME_BYTES).await
}

async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    body: &[u8],
    max: usize,
) -> Result<(), HostError> {
    if body.is_empty() || body.len() > max || body.len() > u32::MAX as usize {
        return Err(HostError::Protocol(
            "frame payload is outside the host limit".into(),
        ));
    }
    writer.write_all(&(body.len() as u32).to_be_bytes()).await?;
    writer.write_all(body).await?;
    writer.flush().await?;
    Ok(())
}

async fn serve_endpoint(driver: ComputerUseDriver, endpoint: String) -> Result<(), HostError> {
    #[cfg(windows)]
    {
        return serve_named_pipe(driver, endpoint).await;
    }
    #[cfg(unix)]
    {
        return serve_unix_socket(driver, endpoint).await;
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = driver;
        let _ = endpoint;
        Err(HostError::Protocol(
            "local endpoint transport is unsupported on this platform".into(),
        ))
    }
}

#[cfg(windows)]
async fn serve_named_pipe(driver: ComputerUseDriver, endpoint: String) -> Result<(), HostError> {
    use tokio::net::windows::named_pipe::ServerOptions;

    loop {
        let server = ServerOptions::new().create(&endpoint)?;
        server.connect().await?;
        let next_driver = driver.clone();
        tokio::spawn(async move {
            let _ = process_connection(next_driver, server).await;
        });
    }
}

#[cfg(unix)]
async fn serve_unix_socket(driver: ComputerUseDriver, endpoint: String) -> Result<(), HostError> {
    use tokio::net::UnixListener;

    let path = Path::new(&endpoint);
    if path.exists() {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_socket() {
            return Err(HostError::Protocol(format!(
                "endpoint exists and is not a socket: {endpoint}"
            )));
        }
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    loop {
        let (stream, _) = listener.accept().await?;
        let next_driver = driver.clone();
        tokio::spawn(async move {
            let _ = process_connection(next_driver, stream).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_prefix_is_big_endian_and_bounded() {
        assert_eq!(u32::from_be_bytes((42_u32).to_be_bytes()), 42);
        assert!(MAX_BINARY_FRAME_BYTES > MAX_JSON_FRAME_BYTES);
    }

    #[test]
    fn hard_denied_intents_do_not_reach_cua() {
        let action = HostAction {
            action: "keypress".into(),
            control_id: None,
            element_index: None,
            input_kind: "raw_input".into(),
            intent: "terminal_or_run_dialog".into(),
            x: None,
            y: None,
            button: None,
            scroll_x: None,
            scroll_y: None,
            path: Vec::new(),
            text: None,
            keys: vec!["ENTER".into()],
            duration_ms: None,
        };
        assert!(action.reject_policy().is_some());
    }
}
