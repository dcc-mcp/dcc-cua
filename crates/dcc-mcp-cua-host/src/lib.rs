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
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

use dcc_mcp_cua_browser::{
    BrowserClickRequest, BrowserDialogRequest, BrowserDownloadRequest, BrowserNavigateRequest,
    BrowserPointerRequest, BrowserPrepareRequest, BrowserSession, BrowserSetInputFilesRequest,
    BrowserSnapshotRequest, BrowserTypeRequest,
};
use dcc_mcp_cua_core::{
    ComputerUseAction, ComputerUseClipboardWriteRequest, ComputerUseDriver, ComputerUseError,
    ComputerUseErrorCode, ComputerUsePoint, ComputerUseRecordingStartRequest, ComputerUseResult,
    ComputerUseSession, ComputerUseTargetScope,
};
use dcc_mcp_cua_shm::SharedImage;

/// Core-compatible control frame limit. Pixel bytes use a separate bounded frame.
pub const MAX_JSON_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_BINARY_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_REQUEST_ID_CHARS: usize = 128;
pub const HOST_PROTOCOL_VERSION: u32 = 3;
pub const CORE_COMPAT_PROTOCOL_VERSION: u32 = 1;

/// Capabilities this implementation actually provides.
pub const HOST_CAPABILITIES: &[&str] = &[
    "exact_window_capabilities",
    "exact_window_state",
    "connection_scoped_sessions",
    "observation_fencing",
    "scoped_raw_input",
    "accessibility_snapshot",
    "accessibility_find",
    "uia_snapshot_and_actions",
    "semantic_value_actions",
    "bounded_wait_for",
    "binary_snapshot_frames",
    "shared_memory_snapshots",
    "cua_cursor_marker",
    "cross_platform_window_control",
    "scoped_window_restore_show_activate",
    "application_inventory",
    "desktop_snapshot",
    "screen_size",
    "cursor_position",
    "application_launch",
    "application_terminate",
    "clipboard_read",
    "clipboard_write",
    "trajectory_recording",
    "browser_exact_binding",
    "browser_prepare",
    "browser_semantic_snapshot",
    "browser_typed_actions",
    "browser_file_upload",
    "browser_file_download",
    "browser_dialog",
    "request_correlation",
    "request_cancellation",
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
        #[cfg(windows)]
        {
            let mut session_id = 0;
            // The legacy Core client discovers this exact per-Windows-session
            // endpoint before sending its protocol-v1 hello.
            let resolved = unsafe {
                windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(
                    windows_sys::Win32::System::Threading::GetCurrentProcessId(),
                    &mut session_id,
                ) != 0
            };
            if resolved {
                return format!(r"\\.\pipe\dcc-mcp-ui-control-host-v1-session-{session_id}");
            }
            return r"\\.\pipe\dcc-mcp-computer-use-v1".to_owned();
        }
        #[cfg(unix)]
        {
            return std::env::temp_dir()
                .join("dcc-mcp-computer-use-v1.sock")
                .to_string_lossy()
                .into_owned();
        }
        #[cfg(not(any(windows, unix)))]
        {
            "dcc-mcp-computer-use-v1".to_owned()
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
    ListApps {},
    DesktopSnapshot {},
    ScreenSize {},
    CursorPosition {},
    LaunchApp {
        grant: TaskGrant,
        launch: dcc_mcp_cua_core::ComputerUseLaunchRequest,
    },
    TerminateApp {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
    },
    ClipboardRead {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        #[serde(default)]
        include_text: bool,
    },
    ClipboardWrite {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        write: ComputerUseClipboardWriteRequest,
    },
    RecordingStart {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        request: ComputerUseRecordingStartRequest,
    },
    RecordingStop {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
    },
    RecordingState {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
    },
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
    BrowserSnapshot {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        request: BrowserSnapshotRequest,
    },
    BrowserPrepare {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        request: BrowserPrepareRequest,
    },
    BrowserNavigate {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        request: BrowserNavigateRequest,
    },
    BrowserClick {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        request: BrowserClickRequest,
    },
    BrowserType {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        request: BrowserTypeRequest,
    },
    BrowserPointer {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        request: BrowserPointerRequest,
    },
    BrowserSetInputFiles {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        request: BrowserSetInputFilesRequest,
    },
    BrowserDownload {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        request: BrowserDownloadRequest,
    },
    BrowserDialog {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        request: BrowserDialogRequest,
    },
    Find {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        query: FindQuery,
    },
    WaitFor {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        condition: WaitCondition,
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
    Cancel {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
    },
}

#[derive(Debug, Deserialize)]
struct HelloParams {
    protocol_version: u32,
    #[allow(dead_code)]
    client_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtocolMode {
    CoreV1,
    ExtendedV3,
}

impl ProtocolMode {
    fn from_version(version: u32) -> Result<Self, HostError> {
        match version {
            CORE_COMPAT_PROTOCOL_VERSION => Ok(Self::CoreV1),
            HOST_PROTOCOL_VERSION => Ok(Self::ExtendedV3),
            _ => Err(HostError::Protocol(format!(
                "protocol version {version} is not supported"
            ))),
        }
    }
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
    #[serde(default)]
    allow_app_launch: bool,
    #[serde(default)]
    allow_app_terminate: bool,
    #[serde(default)]
    allow_clipboard_read: bool,
    #[serde(default)]
    allow_clipboard_write: bool,
    #[serde(default)]
    allow_recording: bool,
    #[serde(default)]
    allow_browser_input: bool,
    #[serde(default)]
    allow_browser_prepare: bool,
    #[serde(default)]
    allow_browser_download: bool,
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
    checked: Option<bool>,
    #[serde(default)]
    keys: Vec<String>,
    #[allow(dead_code)]
    #[serde(default)]
    duration_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct WaitCondition {
    kind: String,
    #[serde(default)]
    element_index: Option<u32>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    interval_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct FindQuery {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    element_index: Option<u32>,
    #[serde(default)]
    max_results: Option<u32>,
}

impl FindQuery {
    fn validate(&self) -> Result<usize, HostError> {
        const MAX_RESULTS: u32 = 50;
        if self.text.as_deref().is_none_or(str::is_empty)
            && self.role.as_deref().is_none_or(str::is_empty)
            && self.element_index.is_none()
        {
            return Err(HostError::Protocol(
                "find requires text, role, or element_index".into(),
            ));
        }
        Ok(self.max_results.unwrap_or(10).clamp(1, MAX_RESULTS) as usize)
    }
}

impl WaitCondition {
    fn validate(&self) -> Result<(u64, u64), HostError> {
        const MAX_TIMEOUT_MS: u64 = 30_000;
        const MAX_INTERVAL_MS: u64 = 1_000;
        if !matches!(
            self.kind.as_str(),
            "element_present" | "text_contains" | "text_equals" | "value_equals"
        ) {
            return Err(HostError::Protocol(
                "wait condition kind is unsupported".into(),
            ));
        }
        if matches!(self.kind.as_str(), "text_contains" | "text_equals") && self.text.is_none() {
            return Err(HostError::Protocol(
                "text wait conditions require text".into(),
            ));
        }
        if self.kind == "value_equals" && self.value.is_none() {
            return Err(HostError::Protocol(
                "value_equals wait conditions require value".into(),
            ));
        }
        let timeout_ms = self.timeout_ms.unwrap_or(5_000).min(MAX_TIMEOUT_MS);
        let interval_ms = self.interval_ms.unwrap_or(100).clamp(10, MAX_INTERVAL_MS);
        Ok((timeout_ms, interval_ms))
    }
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

    fn into_computer_use(
        self,
        observation_id: String,
        accessibility: Option<&Value>,
    ) -> ComputerUseResult<ComputerUseAction> {
        if !matches!(self.input_kind.as_str(), "raw_input" | "semantic") {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "input_kind must be raw_input or semantic",
            ));
        }
        if self.input_kind == "raw_input" && self.control_id.is_some() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "control_id is only valid for semantic actions",
            ));
        }
        let resolved_element_index = self.element_index.or_else(|| {
            self.control_id.as_deref().and_then(|id| {
                accessibility.and_then(|root| find_element_index_by_control_id(root, id))
            })
        });
        if self.input_kind != "raw_input" && resolved_element_index.is_none() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "semantic actions require a current CUA element_index or resolvable control_id",
            ));
        }
        if let (Some(control_id), Some(accessibility)) = (self.control_id.as_deref(), accessibility)
        {
            let mapped = find_element_index_by_control_id(accessibility, control_id);
            if mapped.is_none() {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::InvalidAction,
                    "control_id is not present in the latest accessibility snapshot",
                ));
            }
            if self.element_index.is_some() && mapped != self.element_index {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "control_id and element_index identify different controls",
                ));
            }
        }
        let action = if self.action == "set_checked" {
            "set_value".to_owned()
        } else {
            self.action
        };
        let text = if action == "set_value" && self.checked.is_some() {
            self.checked.map(|checked| checked.to_string())
        } else {
            self.text
        };
        Ok(ComputerUseAction {
            action,
            observation_id: Some(observation_id),
            element_index: resolved_element_index,
            x: self.x,
            y: self.y,
            button: self.button,
            scroll_x: self.scroll_x,
            scroll_y: self.scroll_y,
            path: self.path,
            text,
            keys: self.keys,
        })
    }
}

struct HostSession {
    task_grant_id: String,
    allow_raw_input: bool,
    allow_app_terminate: bool,
    allow_clipboard_read: bool,
    allow_clipboard_write: bool,
    allow_recording: bool,
    allow_browser_input: bool,
    allow_browser_prepare: bool,
    allow_browser_download: bool,
    capability: String,
    session: ComputerUseSession,
    browser: BrowserSession,
    latest_observation_id: Option<String>,
    latest_accessibility_state_id: Option<String>,
    latest_accessibility_root: Option<Value>,
    latest_shared_image: Option<SharedImage>,
}

type CancellationRegistry = Arc<Mutex<HashMap<String, CancellationHandle>>>;

#[derive(Clone)]
struct CancellationHandle {
    task_grant_id: String,
    window_capability: String,
    cancelled: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

struct CancellationGuard {
    registry: CancellationRegistry,
    session_id: String,
    handle: CancellationHandle,
}

impl CancellationHandle {
    async fn cancelled(&self) {
        let notified = self.notify.notified();
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        notified.await;
    }
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.remove(&self.session_id);
        }
    }
}

fn register_wait(
    registry: &CancellationRegistry,
    session_id: &str,
    task_grant_id: &str,
    window_capability: &str,
) -> Result<CancellationGuard, HostError> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let handle = CancellationHandle {
        task_grant_id: task_grant_id.to_owned(),
        window_capability: window_capability.to_owned(),
        cancelled,
        notify: Arc::new(tokio::sync::Notify::new()),
    };
    let mut waits = registry
        .lock()
        .map_err(|_| HostError::Protocol("cancellation registry is unavailable".into()))?;
    if waits.contains_key(session_id) {
        return Err(HostError::Protocol("wait_for is already running".into()));
    }
    waits.insert(session_id.to_owned(), handle.clone());
    Ok(CancellationGuard {
        registry: Arc::clone(registry),
        session_id: session_id.to_owned(),
        handle,
    })
}

fn cancel_wait(
    registry: &CancellationRegistry,
    session_id: &str,
    task_grant_id: &str,
    window_capability: &str,
) -> Result<Value, HostError> {
    let handle = registry
        .lock()
        .map_err(|_| HostError::Protocol("cancellation registry is unavailable".into()))?
        .get(session_id)
        .cloned()
        .ok_or_else(|| HostError::Protocol("no wait_for is running for this session".into()))?;
    if handle.task_grant_id != task_grant_id || handle.window_capability != window_capability {
        return Err(HostError::Protocol(
            "cancel credentials do not match the running wait_for".into(),
        ));
    }
    handle.cancelled.store(true, Ordering::Release);
    handle.notify.notify_one();
    Ok(json!({"type":"wait_cancel_requested", "session_id":session_id}))
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
    let mut protocol_mode = None;
    let mut sessions = HashMap::<String, HostSession>::new();
    let mut desktop_shared_image = None;
    let cancellation_registry = Arc::new(Mutex::new(HashMap::new()));

    while let Some(frame) = read_frame(&mut reader, MAX_JSON_FRAME_BYTES).await? {
        let (request_id, request) = match parse_request_frame(&frame) {
            Ok(request) => request,
            Err((request_id, error)) => {
                write_json(
                    &mut writer,
                    with_request_id(
                        error_response("invalid_request", error),
                        request_id.as_deref(),
                    ),
                )
                .await?;
                continue;
            }
        };

        if matches!(&request, Request::WaitFor { .. }) {
            let mut operation = Box::pin(handle_request(
                &driver,
                &mut sessions,
                &mut protocol_mode,
                &mut desktop_shared_image,
                &cancellation_registry,
                request,
            ));
            loop {
                tokio::select! {
                    result = &mut operation => {
                        let (response, attachment) = match result {
                            Ok(result) => result,
                            Err(error) => (error_response(error_code(&error), error.to_string()), None),
                        };
                        write_response(
                            &mut writer,
                            with_request_id(response, request_id.as_deref()),
                            attachment.as_deref(),
                        ).await?;
                        break;
                    }
                    frame = read_frame(&mut reader, MAX_JSON_FRAME_BYTES) => {
                        let Some(frame) = frame? else {
                            drop(operation);
                            return cleanup_sessions(sessions).await;
                        };
                        let (cancel_id, next_request) = match parse_request_frame(&frame) {
                            Ok(request) => request,
                            Err((cancel_id, error)) => {
                                write_json(
                                    &mut writer,
                                    with_request_id(error_response("invalid_request", error), cancel_id.as_deref()),
                                ).await?;
                                continue;
                            }
                        };
                        if let Request::Cancel { session_id, task_grant_id, window_capability } = next_request {
                            let response = match cancel_wait(
                                &cancellation_registry,
                                &session_id,
                                &task_grant_id,
                                &window_capability,
                            ) {
                                Ok(response) => response,
                                Err(error) => error_response(error_code(&error), error.to_string()),
                            };
                            write_json(&mut writer, with_request_id(response, cancel_id.as_deref())).await?;
                        } else {
                            write_json(
                                &mut writer,
                                with_request_id(
                                    error_response("request_in_progress", "wait_for is still running; cancel it before sending another request"),
                                    cancel_id.as_deref(),
                                ),
                            ).await?;
                        }
                    }
                }
            }
        } else {
            let (response, attachment) = match handle_request(
                &driver,
                &mut sessions,
                &mut protocol_mode,
                &mut desktop_shared_image,
                &cancellation_registry,
                request,
            )
            .await
            {
                Ok(result) => result,
                Err(error) => (error_response(error_code(&error), error.to_string()), None),
            };
            write_response(
                &mut writer,
                with_request_id(response, request_id.as_deref()),
                attachment.as_deref(),
            )
            .await?;
        }
    }

    cleanup_sessions(sessions).await
}

async fn cleanup_sessions(sessions: HashMap<String, HostSession>) -> Result<(), HostError> {
    for (_, mut session) in sessions {
        let _ = session.session.stop().await;
    }
    Ok(())
}

async fn handle_request(
    driver: &ComputerUseDriver,
    sessions: &mut HashMap<String, HostSession>,
    protocol_mode: &mut Option<ProtocolMode>,
    desktop_shared_image: &mut Option<SharedImage>,
    cancellation_registry: &CancellationRegistry,
    request: Request,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    if let Request::Hello(params) = &request {
        let mode = ProtocolMode::from_version(params.protocol_version)?;
        *protocol_mode = Some(mode);
        return Ok((
            json!({
                "type": "hello",
                "protocol_version": params.protocol_version,
                "capabilities": HOST_CAPABILITIES,
            }),
            None,
        ));
    }
    let mode = protocol_mode
        .ok_or_else(|| HostError::Protocol("hello is required before stateful requests".into()))?;

    match request {
        Request::Hello(_) => unreachable!(),
        Request::Cancel {
            session_id,
            task_grant_id,
            window_capability,
        } => Ok((
            cancel_wait(
                cancellation_registry,
                &session_id,
                &task_grant_id,
                &window_capability,
            )?,
            None,
        )),
        Request::ListApps {} => {
            let apps = driver.list_apps().await?;
            Ok((json!({"type":"apps", "apps":apps}), None))
        }
        Request::DesktopSnapshot {} => {
            let snapshot = driver.desktop_snapshot().await?;
            let (image, attachment) = match mode {
                ProtocolMode::CoreV1 => {
                    let shared = SharedImage::from_bytes(&snapshot.data, "image/png")
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
                    let descriptor = serde_json::to_value(shared.descriptor())
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
                    *desktop_shared_image = Some(shared);
                    (descriptor, None)
                }
                ProtocolMode::ExtendedV3 => (
                    json!({
                        "name": "",
                        "id": format!("desktop-{}", Uuid::new_v4()),
                        "length": snapshot.data.len(),
                        "mime_type": "image/png",
                        "encoding": "binary_frame",
                    }),
                    Some(snapshot.data),
                ),
            };
            Ok((
                json!({"type":"desktop_snapshot", "state":snapshot.state, "image":image}),
                attachment,
            ))
        }
        Request::ScreenSize {} => Ok((
            json!({"type":"screen_size", "result":driver.screen_size().await?}),
            None,
        )),
        Request::CursorPosition {} => Ok((
            json!({"type":"cursor_position", "result":driver.cursor_position().await?}),
            None,
        )),
        Request::LaunchApp { grant, launch } => {
            if grant.task_grant_id.trim().is_empty() || grant.dcc_type.trim().is_empty() {
                return Err(HostError::Protocol("task grant is incomplete".into()));
            }
            if !grant.allow_app_launch {
                return Err(HostError::Protocol(
                    "application launch is not granted".into(),
                ));
            }
            let result = driver.launch_app(&launch).await?;
            Ok((
                json!({
                    "type":"app_launched",
                    "task_grant_id":grant.task_grant_id,
                    "result":result,
                }),
                None,
            ))
        }
        Request::TerminateApp {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let result = {
                let host =
                    authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
                if !host.allow_app_terminate {
                    return Err(HostError::Protocol(
                        "application termination is not granted".into(),
                    ));
                }
                host.session.terminate_app().await?
            };
            sessions.remove(&session_id);
            Ok((
                json!({
                    "type":"app_terminated",
                    "session_id":session_id,
                    "result":result,
                }),
                None,
            ))
        }
        Request::ClipboardRead {
            session_id,
            task_grant_id,
            window_capability,
            include_text,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !host.allow_clipboard_read {
                return Err(HostError::Protocol("clipboard read is not granted".into()));
            }
            let result = host.session.clipboard_read(include_text).await?;
            Ok((
                json!({"type":"clipboard_read", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::ClipboardWrite {
            session_id,
            task_grant_id,
            window_capability,
            write,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !host.allow_clipboard_write {
                return Err(HostError::Protocol("clipboard write is not granted".into()));
            }
            let result = host.session.clipboard_write(&write).await?;
            Ok((
                json!({"type":"clipboard_written", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::RecordingStart {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !host.allow_recording {
                return Err(HostError::Protocol("recording is not granted".into()));
            }
            let result = host.session.recording_start(&request).await?;
            Ok((
                json!({"type":"recording_started", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::RecordingStop {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !host.allow_recording {
                return Err(HostError::Protocol("recording is not granted".into()));
            }
            let result = host.session.recording_stop().await?;
            Ok((
                json!({"type":"recording_stopped", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::RecordingState {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !host.allow_recording {
                return Err(HostError::Protocol("recording is not granted".into()));
            }
            let result = host.session.recording_state().await?;
            Ok((
                json!({"type":"recording_state", "session_id":session_id, "result":result}),
                None,
            ))
        }
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
                    allow_app_terminate: grant.allow_app_terminate,
                    allow_clipboard_read: grant.allow_clipboard_read,
                    allow_clipboard_write: grant.allow_clipboard_write,
                    allow_recording: grant.allow_recording,
                    allow_browser_input: grant.allow_browser_input,
                    allow_browser_prepare: grant.allow_browser_prepare,
                    allow_browser_download: grant.allow_browser_download,
                    capability: capability.clone(),
                    session,
                    browser: BrowserSession::default(),
                    latest_observation_id: None,
                    latest_accessibility_state_id: None,
                    latest_accessibility_root: None,
                    latest_shared_image: None,
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
            let operation_name = match operation {
                WindowOperation::Restore => "restore",
                WindowOperation::Show => "show",
                WindowOperation::Activate => "activate",
            };
            host.session.activate().await?;
            let state = host.session.window_state().await?;
            Ok((
                json!({
                    "type":"window_state_changed",
                    "session_id":session_id,
                    "operation":operation_name,
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
            host.latest_accessibility_root = Some(accessibility.clone());
            let node_count = accessibility["elements"].as_array().map_or(0, Vec::len);
            let target = json!({
                "process_id": screenshot.observation.process_id,
                "window_handle": screenshot.observation.window_handle,
                "window_title": screenshot.observation.window_title,
            });
            let (image, attachment) = match mode {
                ProtocolMode::CoreV1 => {
                    let shared = SharedImage::from_bytes(&screenshot.data, "image/png")
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
                    let descriptor = serde_json::to_value(shared.descriptor())
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
                    host.latest_shared_image = Some(shared);
                    (descriptor, None)
                }
                ProtocolMode::ExtendedV3 => (
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
            });
            Ok((response, attachment))
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
            host.latest_accessibility_root = Some(root.clone());
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
        Request::BrowserSnapshot {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            let result = host.browser.snapshot(&host.session, request).await?;
            Ok(browser_response("browser_snapshot", session_id, result))
        }
        Request::BrowserPrepare {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !host.allow_browser_prepare {
                return Err(HostError::Protocol(
                    "browser preparation is not granted".into(),
                ));
            }
            let result = host.browser.prepare(&host.session, request).await?;
            Ok(browser_response("browser_prepared", session_id, result))
        }
        Request::BrowserNavigate {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !host.allow_browser_input {
                return Err(HostError::Protocol("browser input is not granted".into()));
            }
            let result = host.browser.navigate(&host.session, request).await?;
            Ok(browser_response("browser_navigated", session_id, result))
        }
        Request::BrowserClick {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !host.allow_browser_input {
                return Err(HostError::Protocol("browser input is not granted".into()));
            }
            let result = host.browser.click(&host.session, request).await?;
            Ok(browser_response("browser_clicked", session_id, result))
        }
        Request::BrowserType {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !host.allow_browser_input {
                return Err(HostError::Protocol("browser input is not granted".into()));
            }
            let result = host.browser.type_text(&host.session, request).await?;
            Ok(browser_response("browser_typed", session_id, result))
        }
        Request::BrowserPointer {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !host.allow_browser_input {
                return Err(HostError::Protocol("browser input is not granted".into()));
            }
            let result = host.browser.pointer(&host.session, request).await?;
            Ok(browser_response(
                "browser_pointer_completed",
                session_id,
                result,
            ))
        }
        Request::BrowserSetInputFiles {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !host.allow_browser_input {
                return Err(HostError::Protocol("browser input is not granted".into()));
            }
            let result = host.browser.set_input_files(&host.session, request).await?;
            Ok(browser_response(
                "browser_files_uploaded",
                session_id,
                result,
            ))
        }
        Request::BrowserDownload {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !host.allow_browser_download {
                return Err(HostError::Protocol(
                    "browser download is not granted".into(),
                ));
            }
            let result = host.browser.download(&host.session, request).await?;
            Ok(browser_response("browser_downloaded", session_id, result))
        }
        Request::BrowserDialog {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !host.allow_browser_input {
                return Err(HostError::Protocol("browser input is not granted".into()));
            }
            let result = host.browser.dialog(&host.session, request).await?;
            Ok(browser_response(
                "browser_dialog_completed",
                session_id,
                result,
            ))
        }
        Request::Find {
            session_id,
            task_grant_id,
            window_capability,
            query,
        } => {
            let max_results = query.validate()?;
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            let root = host.latest_accessibility_root.clone().ok_or_else(|| {
                HostError::ComputerUse(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "take a snapshot before finding accessibility elements",
                ))
            })?;
            let state_id = host.latest_accessibility_state_id.clone().ok_or_else(|| {
                HostError::ComputerUse(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "accessibility state is unavailable; take a snapshot first",
                ))
            })?;
            let matches = find_elements(&root, &query, max_results);
            let target = host
                .session
                .target()
                .ok_or_else(|| HostError::Protocol("CUA did not return a target".into()))?;
            Ok((
                json!({
                    "type":"find_results",
                    "accessibility_state_id":state_id,
                    "target":target_wire(&target),
                    "matches":matches,
                    "node_count":root["elements"].as_array().map_or(0, Vec::len),
                }),
                None,
            ))
        }
        Request::WaitFor {
            session_id,
            task_grant_id,
            window_capability,
            condition,
        } => {
            let (timeout_ms, interval_ms) = condition.validate()?;
            let cancellation = register_wait(
                cancellation_registry,
                &session_id,
                &task_grant_id,
                &window_capability,
            )?;
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            let started = Instant::now();
            loop {
                let root = tokio::select! {
                    _ = cancellation.handle.cancelled() => {
                        return Ok((json!({
                            "type":"wait_cancelled",
                            "success":false,
                            "session_id":session_id,
                            "error_code":"cancelled",
                            "elapsed_ms":started.elapsed().as_millis(),
                        }), None));
                    }
                    result = host.session.accessibility_snapshot(5_000, 25) => result?,
                };
                if wait_condition_matches(&root, &condition) {
                    return Ok((
                        json!({
                            "type":"wait_completed",
                            "success":true,
                            "session_id":session_id,
                            "condition":condition.kind,
                            "elapsed_ms":started.elapsed().as_millis(),
                        }),
                        None,
                    ));
                }
                if started.elapsed().as_millis() >= u128::from(timeout_ms) {
                    return Ok((
                        json!({
                            "type":"wait_completed",
                            "success":false,
                            "session_id":session_id,
                            "condition":condition.kind,
                            "error_code":"timeout",
                            "elapsed_ms":started.elapsed().as_millis(),
                        }),
                        None,
                    ));
                }
                tokio::select! {
                    _ = cancellation.handle.cancelled() => {
                        return Ok((json!({
                            "type":"wait_cancelled",
                            "success":false,
                            "session_id":session_id,
                            "error_code":"cancelled",
                            "elapsed_ms":started.elapsed().as_millis(),
                        }), None));
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(interval_ms)) => {}
                }
            }
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
            let action = action
                .into_computer_use(observation_id, host.latest_accessibility_root.as_ref())?;
            let result = host.session.perform_action(&action).await?;
            host.latest_observation_id = None;
            host.latest_accessibility_state_id = None;
            host.latest_accessibility_root = None;
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

fn browser_response(
    response_type: &str,
    session_id: String,
    result: dcc_mcp_cua_browser::BrowserResult,
) -> (Value, Option<Vec<u8>>) {
    let attachment = result.image.as_ref().map(|image| image.data.clone());
    let mut response = json!({
        "type": response_type,
        "session_id": session_id,
        "result": result.value,
    });
    if let Some(image) = result.image {
        response["image"] = json!({
            "encoding": "binary_frame",
            "mime_type": image.mime_type,
            "length": image.data.len(),
        });
    }
    (response, attachment)
}

fn find_elements(root: &Value, query: &FindQuery, max_results: usize) -> Vec<Value> {
    root["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|element| {
            let index_matches = query.element_index.is_none_or(|expected| {
                element["element_index"]
                    .as_u64()
                    .or_else(|| element["index"].as_u64())
                    == Some(u64::from(expected))
            });
            let role_matches = query.role.as_deref().is_none_or(|expected| {
                element["role"]
                    .as_str()
                    .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
            });
            let text_matches = query.text.as_deref().is_none_or(|expected| {
                let expected = expected.to_ascii_lowercase();
                ["name", "label", "title", "text", "value"]
                    .iter()
                    .filter_map(|field| element[*field].as_str())
                    .any(|actual| actual.to_ascii_lowercase().contains(&expected))
            });
            index_matches && role_matches && text_matches
        })
        .take(max_results)
        .cloned()
        .collect()
}

fn find_element_index_by_control_id(root: &Value, control_id: &str) -> Option<u32> {
    let expected = control_id.strip_prefix("uia:").unwrap_or(control_id);
    fn visit(value: &Value, expected: &str) -> Option<u32> {
        match value {
            Value::Object(object) => {
                let matches = ["runtime_id", "control_id", "automation_id"]
                    .into_iter()
                    .filter_map(|key| object.get(key).and_then(Value::as_str))
                    .any(|value| value.strip_prefix("uia:").unwrap_or(value) == expected);
                if matches {
                    return object
                        .get("element_index")
                        .or_else(|| object.get("index"))
                        .and_then(Value::as_u64)
                        .and_then(|index| u32::try_from(index).ok());
                }
                object.values().find_map(|child| visit(child, expected))
            }
            Value::Array(values) => values.iter().find_map(|child| visit(child, expected)),
            _ => None,
        }
    }
    visit(root, expected)
}

fn wait_condition_matches(root: &Value, condition: &WaitCondition) -> bool {
    root["elements"].as_array().is_some_and(|elements| {
        elements.iter().any(|element| {
            if let Some(index) = condition.element_index {
                let actual = element["element_index"]
                    .as_u64()
                    .or_else(|| element["index"].as_u64());
                if actual != Some(u64::from(index)) {
                    return false;
                }
            }
            match condition.kind.as_str() {
                "element_present" => true,
                "text_contains" => condition.text.as_deref().is_some_and(|expected| {
                    ["name", "label", "title", "text", "value"]
                        .iter()
                        .filter_map(|field| element[*field].as_str())
                        .any(|actual| actual.contains(expected))
                }),
                "text_equals" => condition.text.as_deref().is_some_and(|expected| {
                    ["name", "label", "title", "text"]
                        .iter()
                        .filter_map(|field| element[*field].as_str())
                        .any(|actual| actual == expected)
                }),
                "value_equals" => condition
                    .value
                    .as_deref()
                    .is_some_and(|expected| element["value"].as_str() == Some(expected)),
                _ => false,
            }
        })
    })
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
            ComputerUseErrorCode::BrowserRefused => "browser_refused",
            ComputerUseErrorCode::ClipboardRefused => "clipboard_refused",
            ComputerUseErrorCode::RecordingRefused => "recording_refused",
            ComputerUseErrorCode::CaptureFailed => "capture_failed",
            ComputerUseErrorCode::InputFailed => "input_failed",
            ComputerUseErrorCode::InvalidAction => "invalid_request",
            ComputerUseErrorCode::MissingWindow => "invalid_target",
            ComputerUseErrorCode::BackendUnavailable => "backend_unavailable",
        },
        HostError::Io(_) => "backend_unavailable",
        HostError::Protocol(message) if message.contains("version") => "protocol_mismatch",
        HostError::Protocol(message) if message.contains("raw input") => "raw_input_not_granted",
        HostError::Protocol(message) if message.contains("application launch") => {
            "app_launch_not_granted"
        }
        HostError::Protocol(message) if message.contains("application termination") => {
            "app_terminate_not_granted"
        }
        HostError::Protocol(message) if message.contains("browser input") => {
            "browser_input_not_granted"
        }
        HostError::Protocol(message) if message.contains("browser preparation") => {
            "browser_prepare_not_granted"
        }
        HostError::Protocol(message) if message.contains("browser download") => {
            "browser_download_not_granted"
        }
        HostError::Protocol(message) if message.contains("clipboard read") => {
            "clipboard_read_not_granted"
        }
        HostError::Protocol(message) if message.contains("clipboard write") => {
            "clipboard_write_not_granted"
        }
        HostError::Protocol(message) if message.contains("recording") => "recording_not_granted",
        HostError::Protocol(message) if message.contains("already running") => {
            "request_in_progress"
        }
        HostError::Protocol(message) if message.contains("no wait_for") => "request_not_found",
        HostError::Protocol(message) if message.contains("cancel credentials") => "forbidden",
        HostError::Protocol(message) if message.contains("accessibility") => "unsupported",
        HostError::Protocol(_) => "invalid_request",
    }
}

fn error_response(code: &str, message: impl Into<String>) -> Value {
    json!({"type":"error", "code":code, "message":message.into()})
}

fn parse_request_frame(
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
    serde_json::from_value(envelope)
        .map(|request| (request_id.clone(), request))
        .map_err(|error| (request_id, error.to_string()))
}

fn request_id_from(value: &Value) -> Result<Option<String>, String> {
    let Some(request_id) = value.get("request_id") else {
        return Ok(None);
    };
    let request_id = request_id
        .as_str()
        .ok_or_else(|| "request_id must be a string".to_owned())?;
    if request_id.is_empty() || request_id.chars().count() > MAX_REQUEST_ID_CHARS {
        return Err(format!(
            "request_id must contain 1..{MAX_REQUEST_ID_CHARS} characters"
        ));
    }
    Ok(Some(request_id.to_owned()))
}

fn with_request_id(mut response: Value, request_id: Option<&str>) -> Value {
    if let Some(request_id) = request_id {
        response["request_id"] = Value::String(request_id.to_owned());
    }
    response
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

async fn write_response<W: AsyncWrite + Unpin>(
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

async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    body: &[u8],
    max: usize,
) -> Result<(), HostError> {
    write_frame_unflushed(writer, body, max).await?;
    writer.flush().await?;
    Ok(())
}

async fn write_frame_unflushed<W: AsyncWrite + Unpin>(
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
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
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

    #[test]
    fn wait_cancellation_requires_exact_credentials() {
        let registry = Arc::new(Mutex::new(HashMap::new()));
        let guard = register_wait(&registry, "session-1", "grant-1", "cap-1").unwrap();
        assert!(cancel_wait(&registry, "session-1", "grant-1", "wrong-cap").is_err());
        let response = cancel_wait(&registry, "session-1", "grant-1", "cap-1").unwrap();
        assert_eq!(response["type"], "wait_cancel_requested");
        assert!(guard.handle.cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn request_frame_preserves_correlation_on_deserialization_errors() {
        let parsed =
            parse_request_frame(br#"{"request_id":"req-7","method":"unknown","params":{}}"#);
        assert_eq!(parsed.unwrap_err().0, Some("req-7".into()));
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
            checked: None,
            keys: vec!["ENTER".into()],
            duration_ms: None,
        };
        assert!(action.reject_policy().is_some());
    }

    #[test]
    fn legacy_control_ids_resolve_to_current_cua_elements() {
        let root = json!({
            "elements": [{"runtime_id": "42.1", "element_index": 7}]
        });
        let action = HostAction {
            action: "set_checked".into(),
            control_id: Some("uia:42.1".into()),
            element_index: None,
            input_kind: "semantic".into(),
            intent: "ordinary_edit".into(),
            x: None,
            y: None,
            button: None,
            scroll_x: None,
            scroll_y: None,
            path: Vec::new(),
            text: None,
            checked: Some(true),
            keys: Vec::new(),
            duration_ms: None,
        };
        let mapped = action
            .into_computer_use("obs-1".into(), Some(&root))
            .unwrap();
        assert_eq!(mapped.action, "set_value");
        assert_eq!(mapped.element_index, Some(7));
        assert_eq!(mapped.text.as_deref(), Some("true"));
    }

    #[test]
    fn hello_versions_select_shared_memory_or_binary_mode() {
        assert_eq!(
            ProtocolMode::from_version(CORE_COMPAT_PROTOCOL_VERSION).unwrap(),
            ProtocolMode::CoreV1
        );
        assert_eq!(
            ProtocolMode::from_version(HOST_PROTOCOL_VERSION).unwrap(),
            ProtocolMode::ExtendedV3
        );
    }

    #[test]
    fn app_launch_grant_defaults_to_denied() {
        let grant: TaskGrant = serde_json::from_value(json!({
            "task_grant_id": "task-1",
            "dcc_type": "unreal"
        }))
        .expect("legacy grants should remain readable");
        assert!(!grant.allow_app_launch);
        assert!(!grant.allow_app_terminate);
        assert!(!grant.allow_clipboard_read);
        assert!(!grant.allow_clipboard_write);
        assert!(!grant.allow_recording);
        assert!(!grant.allow_browser_input);
        assert!(!grant.allow_browser_prepare);
        assert!(!grant.allow_browser_download);
        assert_eq!(
            error_code(&HostError::Protocol(
                "browser download is not granted".into()
            )),
            "browser_download_not_granted"
        );
    }

    #[test]
    fn app_requests_parse_with_core_params_frames() {
        assert!(matches!(
            serde_json::from_value::<Request>(json!({
                "method": "list_apps",
                "params": {}
            })),
            Ok(Request::ListApps {})
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
                "method": "screen_size",
                "params": {}
            })),
            Ok(Request::ScreenSize {})
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

    #[test]
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

    #[test]
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
}
