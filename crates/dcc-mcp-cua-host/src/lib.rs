//! Long-lived local host IPC for dcc-mcp Computer Use.
//!
//! The wire format is a versioned length-prefixed protocol: a big-endian `u32`
//! followed by one UTF-8 JSON object. Screenshot bytes are sent as a second
//! framed payload when binary transport is selected, so control frames stay
//! bounded and the transport does not base64-encode pixels.

mod request_handler;
use request_handler::handle_request;

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
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinSet;
use uuid::Uuid;

use dcc_mcp_cua_browser::{
    BrowserClickRequest, BrowserDialogRequest, BrowserDownloadRequest, BrowserNavigateRequest,
    BrowserPointerRequest, BrowserPrepareRequest, BrowserSession, BrowserSetInputFilesRequest,
    BrowserSnapshotRequest, BrowserTypeRequest,
};
use dcc_mcp_cua_core::{
    ComputerUseAction, ComputerUseClipboardWriteRequest, ComputerUseDesktopSession,
    ComputerUseDesktopSnapshot, ComputerUseDriver, ComputerUseError, ComputerUseErrorCode,
    ComputerUseImage, ComputerUsePoint, ComputerUseRecordingStartRequest, ComputerUseResult,
    ComputerUseScreenshot, ComputerUseSession, ComputerUseTargetScope, ComputerUseToolResult,
    ComputerUseWindowQuery, ComputerUseWindowWaitRequest, ComputerUseZoomRequest,
};
use dcc_mcp_cua_indicator::{BannerTarget, ControlBanner};
use dcc_mcp_cua_shm::SharedImage;

// ponytail: one OS input stream is process-global; shard only if platforms gain isolated seats.
static RAW_INPUT_QUEUE: AsyncMutex<()> = AsyncMutex::const_new(());

/// Control frame limit. Pixel bytes use a separate bounded frame.
pub const MAX_JSON_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_BINARY_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_REQUEST_ID_CHARS: usize = 128;
pub const HOST_PROTOCOL_VERSION: u32 = 1;

/// Capabilities this implementation actually provides.
pub const HOST_CAPABILITIES: &[&str] = &[
    "exact_window_capabilities",
    "exact_window_state",
    "connection_scoped_sessions",
    "observation_fencing",
    "action_post_snapshot",
    "semantic_element_tokens",
    "background_first_input_delivery",
    "scoped_raw_input",
    "serialized_raw_input",
    "accessibility_snapshot",
    "accessibility_find",
    "state_verification",
    "session_state",
    "session_escalation",
    "cursor_controls",
    "uia_snapshot_and_actions",
    "windows_background_uia_fallback",
    "zoomed_window_observation",
    "semantic_value_actions",
    "two_axis_scroll",
    "bounded_wait_for",
    "binary_snapshot_frames",
    "shared_memory_snapshots",
    "shared_memory_verification_images",
    "shared_memory_browser_images",
    "shared_memory_native_images",
    "cua_cursor_marker",
    "cross_platform_window_control",
    "scoped_window_activate",
    "degraded_window_visual_fallback",
    "application_inventory",
    "window_inventory",
    "window_inventory_filters",
    "window_wait",
    "window_wait_cancellation",
    "tool_inventory",
    "authorized_native_tool_calls",
    "authorized_global_native_tool_calls",
    "desktop_accessibility_tree",
    "desktop_snapshot",
    "screen_size",
    "cursor_position",
    "scoped_desktop_sessions",
    "scoped_desktop_raw_input",
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
    "host_ping",
    "host_diagnostics",
    "pipelined_read_requests",
    "parallel_discovery_requests",
];

/// Return only capabilities backed by the current platform runtime.
#[must_use]
pub fn host_capabilities() -> Vec<&'static str> {
    HOST_CAPABILITIES
        .iter()
        .copied()
        .filter(|capability| {
            (!matches!(*capability, "cursor_controls" | "cua_cursor_marker")
                || cfg!(any(windows, target_os = "linux")))
                && (*capability != "windows_background_uia_fallback" || cfg!(windows))
        })
        .collect()
}

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
            let resolved = unsafe {
                windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(
                    windows_sys::Win32::System::Threading::GetCurrentProcessId(),
                    &mut session_id,
                ) != 0
            };
            if resolved {
                return format!(r"\\.\pipe\dcc-mcp-cua-v1-session-{session_id}");
            }
            r"\\.\pipe\dcc-mcp-cua-v1".to_owned()
        }
        #[cfg(unix)]
        {
            std::env::temp_dir()
                .join("dcc-mcp-cua-v1.sock")
                .to_string_lossy()
                .into_owned()
        }
        #[cfg(not(any(windows, unix)))]
        {
            "dcc-mcp-cua-v1".to_owned()
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
    Ping {},
    Doctor {},
    ListApps {},
    ListTools {},
    ListWindows {
        #[serde(default)]
        app: Option<String>,
        #[serde(default)]
        pid: Option<u32>,
        #[serde(default)]
        window_id: Option<u64>,
        #[serde(default)]
        window_title: Option<String>,
        #[serde(default)]
        on_screen_only: bool,
    },
    WaitForWindow(ComputerUseWindowWaitRequest),
    DesktopSnapshot {},
    ScreenSize {},
    CursorPosition {},
    OpenDesktopSession {
        session_id: String,
        grant: TaskGrant,
    },
    DesktopSessionSnapshot {
        session_id: String,
        task_grant_id: String,
        desktop_capability: String,
    },
    ExecuteDesktopAction {
        session_id: String,
        task_grant_id: String,
        desktop_capability: String,
        observation_id: String,
        action: HostAction,
        #[serde(default)]
        capture_after: bool,
    },
    StopDesktopSession {
        session_id: String,
    },
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
        #[serde(default)]
        max_depth: u32,
        #[serde(default)]
        max_nodes: u32,
        #[serde(default)]
        activate_before: bool,
    },
    Zoom {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        request: ComputerUseZoomRequest,
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
    VerifyState {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        expect: Value,
        #[serde(default)]
        timeout_ms: Option<u64>,
        #[serde(default)]
        stable_samples: Option<u64>,
        #[serde(default)]
        include_screenshot: bool,
    },
    CallTool {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        tool: String,
        arguments: Value,
    },
    CallGlobalTool {
        grant: TaskGrant,
        tool: String,
        arguments: Value,
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
        #[serde(default)]
        capture_after: bool,
        #[serde(default)]
        post_snapshot_max_depth: u32,
        #[serde(default)]
        post_snapshot_max_nodes: u32,
    },
    ResumeSession {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
    },
    GetSessionState {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
    },
    CursorTool {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        tool: String,
        arguments: Value,
    },
    EscalateSession {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        reason: String,
        #[serde(default)]
        detail: Option<String>,
    },
    StopSession {
        session_id: String,
    },
    Cancel {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
    },
    CancelWindowWait {
        wait_id: String,
    },
}

#[derive(Debug, Deserialize)]
struct HelloParams {
    protocol_version: u32,
    client_name: String,
    #[serde(default)]
    snapshot_transport: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotTransport {
    SharedMemory,
    BinaryFrame,
}

impl SnapshotTransport {
    fn from_hello(params: &HelloParams) -> Result<Self, HostError> {
        if params.protocol_version != HOST_PROTOCOL_VERSION {
            return Err(HostError::Protocol(format!(
                "protocol version {} is not supported",
                params.protocol_version
            )));
        }
        match params
            .snapshot_transport
            .as_deref()
            .unwrap_or("binary_frame")
        {
            "binary_frame" => Ok(Self::BinaryFrame),
            "shared_memory" => Ok(Self::SharedMemory),
            value => Err(HostError::Protocol(format!(
                "snapshot transport {value} is not supported"
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
    window_title: Option<String>,
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
    #[serde(default)]
    allow_native_tool: bool,
    #[serde(default)]
    allow_session_escalation: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WindowOperation {
    Activate,
}

#[derive(Debug, Deserialize)]
struct HostAction {
    action: String,
    #[serde(default)]
    element_index: Option<u32>,
    #[serde(default)]
    element_token: Option<String>,
    #[serde(default)]
    delivery_mode: Option<String>,
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
    scroll_by: Option<String>,
    #[serde(default)]
    path: Vec<ComputerUsePoint>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    delay_ms: Option<u64>,
    #[serde(default)]
    type_chars_only: bool,
    #[serde(default)]
    checked: Option<bool>,
    #[serde(default)]
    keys: Vec<String>,
    #[serde(default)]
    modifiers: Vec<String>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    steps: Option<u32>,
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

    fn into_computer_use(self, observation_id: String) -> ComputerUseResult<ComputerUseAction> {
        if !matches!(self.input_kind.as_str(), "raw_input" | "semantic") {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "input_kind must be raw_input or semantic",
            ));
        }
        if self.input_kind != "raw_input"
            && self.element_index.is_none()
            && self.element_token.is_none()
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "semantic actions require a current CUA element_index or element_token",
            ));
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
            element_index: self.element_index,
            element_token: self.element_token,
            delivery_mode: self.delivery_mode,
            x: self.x,
            y: self.y,
            button: self.button,
            scroll_x: self.scroll_x,
            scroll_y: self.scroll_y,
            scroll_by: self.scroll_by,
            path: self.path,
            text,
            delay_ms: self.delay_ms,
            duration_ms: self.duration_ms,
            steps: self.steps,
            type_chars_only: self.type_chars_only,
            keys: self.keys,
            modifiers: self.modifiers,
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
    allow_native_tool: bool,
    allow_session_escalation: bool,
    capability: String,
    session: ComputerUseSession,
    banner: ControlBanner,
    browser: BrowserSession,
    latest_observation_id: Option<String>,
    latest_accessibility_state_id: Option<String>,
    latest_accessibility_root: Option<Value>,
    latest_shared_image: Option<SharedImage>,
}

struct HostDesktopSession {
    task_grant_id: String,
    allow_raw_input: bool,
    capability: String,
    session: ComputerUseDesktopSession,
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

fn register_window_wait(
    registry: &CancellationRegistry,
    wait_id: &str,
) -> Result<CancellationGuard, HostError> {
    validate_window_wait_id(wait_id)?;
    let key = format!("window_wait:{wait_id}");
    let handle = CancellationHandle {
        task_grant_id: String::new(),
        window_capability: String::new(),
        cancelled: Arc::new(AtomicBool::new(false)),
        notify: Arc::new(tokio::sync::Notify::new()),
    };
    let mut waits = registry
        .lock()
        .map_err(|_| HostError::Protocol("cancellation registry is unavailable".into()))?;
    if waits.contains_key(&key) {
        return Err(HostError::Protocol(
            "window_wait is already running for this wait_id".into(),
        ));
    }
    waits.insert(key.clone(), handle.clone());
    Ok(CancellationGuard {
        registry: Arc::clone(registry),
        session_id: key,
        handle,
    })
}

fn cancel_window_wait(registry: &CancellationRegistry, wait_id: &str) -> Result<Value, HostError> {
    validate_window_wait_id(wait_id)?;
    let key = format!("window_wait:{wait_id}");
    let handle = registry
        .lock()
        .map_err(|_| HostError::Protocol("cancellation registry is unavailable".into()))?
        .get(&key)
        .cloned()
        .ok_or_else(|| HostError::Protocol("no window_wait is running for this wait_id".into()))?;
    handle.cancelled.store(true, Ordering::Release);
    handle.notify.notify_one();
    Ok(json!({
        "type": "window_wait_cancel_requested",
        "wait_id": wait_id,
    }))
}

fn validate_window_wait_id(wait_id: &str) -> Result<(), HostError> {
    if wait_id.is_empty() || wait_id.chars().count() > MAX_REQUEST_ID_CHARS {
        return Err(HostError::Protocol(format!(
            "wait_id must contain 1..{MAX_REQUEST_ID_CHARS} characters"
        )));
    }
    Ok(())
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
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
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
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut reader = reader;
    let writer = Arc::new(AsyncMutex::new(writer));
    let mut parallel_tasks = JoinSet::new();
    let mut snapshot_transport = None;
    let mut sessions = HashMap::<String, HostSession>::new();
    let mut desktop_sessions = HashMap::<String, HostDesktopSession>::new();
    let mut desktop_shared_image = None;
    let cancellation_registry = Arc::new(Mutex::new(HashMap::new()));

    while let Some(frame) = read_frame(&mut reader, MAX_JSON_FRAME_BYTES).await? {
        let (request_id, request) = match parse_request_frame(&frame) {
            Ok(request) => request,
            Err((request_id, error)) => {
                write_json_locked(
                    &writer,
                    with_request_id(
                        error_response("invalid_request", error),
                        request_id.as_deref(),
                    ),
                )
                .await?;
                continue;
            }
        };

        let is_window_wait = matches!(&request, Request::WaitForWindow(_));
        if snapshot_transport.is_none() && !matches!(&request, Request::Hello(_)) {
            write_json_locked(
                &writer,
                with_request_id(
                    error_response("protocol_error", "hello is required before Host requests"),
                    request_id.as_deref(),
                ),
            )
            .await?;
            continue;
        }
        let window_wait_guard = if is_window_wait {
            request_id
                .as_deref()
                .map(|wait_id| register_window_wait(&cancellation_registry, wait_id))
                .transpose()?
        } else {
            None
        };

        if matches!(
            &request,
            Request::WaitFor { .. } | Request::WaitForWindow(_)
        ) {
            let mut operation = Box::pin(handle_request(
                &driver,
                &mut sessions,
                &mut desktop_sessions,
                &mut snapshot_transport,
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
                        write_response_locked(
                            &writer,
                            with_request_id(response, request_id.as_deref()),
                            attachment.as_deref(),
                        ).await?;
                        break;
                    }
                    frame = read_frame(&mut reader, MAX_JSON_FRAME_BYTES) => {
                        let Some(frame) = frame? else {
                            drop(operation);
                            parallel_tasks.abort_all();
                            while parallel_tasks.join_next().await.is_some() {}
                            return cleanup_sessions(sessions, desktop_sessions).await;
                        };
                        let (cancel_id, next_request) = match parse_request_frame(&frame) {
                            Ok(request) => request,
                            Err((cancel_id, error)) => {
                                write_json_locked(
                                    &writer,
                                    with_request_id(error_response("invalid_request", error), cancel_id.as_deref()),
                                ).await?;
                                continue;
                            }
                        };
                        if let Request::CancelWindowWait { wait_id } = next_request {
                            let response = match cancel_window_wait(&cancellation_registry, &wait_id) {
                                Ok(response) => response,
                                Err(error) => error_response(error_code(&error), error.to_string()),
                            };
                            let cancelled = response["type"] == "window_wait_cancel_requested";
                            write_json_locked(&writer, with_request_id(response, cancel_id.as_deref())).await?;
                            if is_window_wait && cancelled {
                                drop(operation);
                                write_json_locked(
                                    &writer,
                                    with_request_id(
                                        json!({
                                            "type": "window_wait_cancelled",
                                            "success": false,
                                            "wait_id": wait_id,
                                            "error_code": "cancelled",
                                        }),
                                        request_id.as_deref(),
                                    ),
                                )
                                .await?;
                                break;
                            }
                        } else if let Request::Cancel { session_id, task_grant_id, window_capability } = next_request {
                            let response = match cancel_wait(
                                &cancellation_registry,
                                &session_id,
                                &task_grant_id,
                                &window_capability,
                            ) {
                                Ok(response) => response,
                                Err(error) => error_response(error_code(&error), error.to_string()),
                            };
                            write_json_locked(&writer, with_request_id(response, cancel_id.as_deref())).await?;
                        } else {
                            write_json_locked(
                                &writer,
                                with_request_id(
                                    error_response("request_in_progress", "wait_for is still running; cancel it before sending another request"),
                                    cancel_id.as_deref(),
                                ),
                            ).await?;
                        }
                    }
                }
            }
            drop(window_wait_guard);
        } else if is_parallel_request(&request) {
            let task_driver = driver.clone();
            let task_writer = writer.clone();
            parallel_tasks.spawn(async move {
                let (response, attachment) =
                    match handle_parallel_request(&task_driver, request).await {
                        Ok(result) => result,
                        Err(error) => (error_response(error_code(&error), error.to_string()), None),
                    };
                write_response_locked(
                    &task_writer,
                    with_request_id(response, request_id.as_deref()),
                    attachment.as_deref(),
                )
                .await
            });
        } else {
            let (response, attachment) = match handle_request(
                &driver,
                &mut sessions,
                &mut desktop_sessions,
                &mut snapshot_transport,
                &mut desktop_shared_image,
                &cancellation_registry,
                request,
            )
            .await
            {
                Ok(result) => result,
                Err(error) => (error_response(error_code(&error), error.to_string()), None),
            };
            write_response_locked(
                &writer,
                with_request_id(response, request_id.as_deref()),
                attachment.as_deref(),
            )
            .await?;
        }
    }

    while parallel_tasks.join_next().await.is_some() {}

    cleanup_sessions(sessions, desktop_sessions).await
}

async fn cleanup_sessions(
    sessions: HashMap<String, HostSession>,
    desktop_sessions: HashMap<String, HostDesktopSession>,
) -> Result<(), HostError> {
    for (_, mut session) in sessions {
        let _ = session.session.stop().await;
    }
    for (_, mut session) in desktop_sessions {
        let _ = session.session.stop().await;
    }
    Ok(())
}

fn ping_response() -> Value {
    json!({
        "type": "pong",
        "protocol_version": HOST_PROTOCOL_VERSION,
        "host_version": env!("CARGO_PKG_VERSION"),
    })
}

fn is_parallel_request(request: &Request) -> bool {
    matches!(
        request,
        Request::Ping {}
            | Request::ListApps {}
            | Request::ListTools {}
            | Request::ListWindows { .. }
            | Request::ScreenSize {}
            | Request::CursorPosition {}
    )
}

async fn handle_parallel_request(
    driver: &ComputerUseDriver,
    request: Request,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    match request {
        Request::Ping {} => Ok((ping_response(), None)),
        Request::ListApps {} => {
            let apps = driver.list_apps().await?;
            Ok((json!({"type":"apps", "apps":apps}), None))
        }
        Request::ListTools {} => Ok((
            json!({"type":"tools", "tools":driver.list_tools().await?}),
            None,
        )),
        Request::ListWindows {
            app,
            pid,
            window_id,
            window_title,
            on_screen_only,
        } => Ok((
            list_windows_response(
                driver,
                app.as_deref(),
                pid,
                window_id,
                window_title.as_deref(),
                on_screen_only,
            )
            .await?,
            None,
        )),
        Request::ScreenSize {} => Ok((
            json!({"type":"screen_size", "result":driver.screen_size().await?}),
            None,
        )),
        Request::CursorPosition {} => Ok((
            json!({"type":"cursor_position", "result":driver.cursor_position().await?}),
            None,
        )),
        _ => Err(HostError::Protocol(
            "request is not eligible for parallel Host dispatch".into(),
        )),
    }
}

async fn list_windows_response(
    driver: &ComputerUseDriver,
    app: Option<&str>,
    pid: Option<u32>,
    window_id: Option<u64>,
    window_title: Option<&str>,
    on_screen_only: bool,
) -> Result<Value, HostError> {
    let query = ComputerUseWindowQuery {
        app: app.map(str::to_owned),
        process_id: pid,
        window_handle: window_id,
        window_title: window_title.map(str::to_owned),
        on_screen_only,
    };
    query.validate_selectors()?;
    let mut windows = driver
        .list_windows_filtered(query.process_id, query.on_screen_only)
        .await?;
    windows.retain(|window| query.matches_window(window));
    Ok(json!({"type":"windows", "windows":windows}))
}

fn image_response(
    image: ComputerUseImage,
    mode: SnapshotTransport,
    shared_image: &mut Option<SharedImage>,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    match mode {
        SnapshotTransport::SharedMemory => {
            let shared = SharedImage::from_bytes(&image.data, &image.mime_type)
                .map_err(|error| HostError::Protocol(error.to_string()))?;
            let mut descriptor = serde_json::to_value(shared.descriptor())
                .map_err(|error| HostError::Protocol(error.to_string()))?;
            descriptor["encoding"] = Value::String("shared_memory".into());
            *shared_image = Some(shared);
            Ok((descriptor, None))
        }
        SnapshotTransport::BinaryFrame => Ok((
            json!({
                "name": "",
                "id": "",
                "length": image.data.len(),
                "mime_type": image.mime_type,
                "encoding": "binary_frame",
            }),
            Some(image.data),
        )),
    }
}

fn browser_response(
    response_type: &str,
    session_id: String,
    result: dcc_mcp_cua_browser::BrowserResult,
    mode: SnapshotTransport,
    shared_image: &mut Option<SharedImage>,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let images = result.images;
    let mut response_result = result.value;
    let mut attachment_bytes = Vec::new();
    let mut attachments = Vec::with_capacity(images.len());
    let use_shared_memory = mode == SnapshotTransport::SharedMemory && images.len() == 1;

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

    if let Some(content) = response_result
        .get_mut("content")
        .and_then(Value::as_array_mut)
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
        "type": response_type,
        "session_id": session_id,
        "result": response_result,
    });
    if let Some(image) = image_descriptor {
        response["image"] = image;
    }
    if attachments.len() > 1 {
        response["attachments"] = Value::Array(attachments);
    }
    Ok((
        response,
        (!attachment_bytes.is_empty()).then_some(attachment_bytes),
    ))
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
                ["name", "label", "title", "text", "value", "automation_id"]
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

async fn authorized_session<'a>(
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
    if session.banner.interrupted() {
        let cleanup_note = session
            .session
            .stop()
            .await
            .err()
            .map(|error| format!("; CUA cleanup also failed: {error}"))
            .unwrap_or_default();
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::UserInterrupted,
            format!(
                "the user pressed Escape or the safety banner stopped; the session was stopped{cleanup_note}"
            ),
        )
        .into());
    }
    Ok(session)
}

fn authorized_desktop_session<'a>(
    sessions: &'a mut HashMap<String, HostDesktopSession>,
    session_id: &str,
    grant_id: &str,
    capability: &str,
) -> Result<&'a mut HostDesktopSession, HostError> {
    let session = sessions
        .get_mut(session_id)
        .ok_or_else(|| HostError::Protocol("desktop session not found".into()))?;
    if session.task_grant_id != grant_id || session.capability != capability {
        return Err(HostError::Protocol(
            "desktop session grant or capability mismatch".into(),
        ));
    }
    Ok(session)
}

fn native_tool_response_with_transport(
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

fn action_completed_response(
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
    let mut response = json!({
        "type": "action_completed",
        "success": true,
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

fn action_completed_with_snapshot_response(
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

fn desktop_action_completed_with_snapshot_response(
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
        HostError::Protocol(message) if message.contains("native CUA tool calls") => {
            "native_tool_not_granted"
        }
        HostError::Protocol(message) if message.contains("session escalation") => {
            "session_escalation_not_granted"
        }
        HostError::Protocol(message) if message.contains("already running") => {
            "request_in_progress"
        }
        HostError::Protocol(message)
            if message.contains("no wait_for") || message.contains("no window_wait") =>
        {
            "request_not_found"
        }
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

async fn write_json_locked<W: AsyncWrite + Unpin>(
    writer: &Arc<AsyncMutex<W>>,
    value: Value,
) -> Result<(), HostError> {
    let mut writer = writer.lock().await;
    write_json(&mut *writer, value).await
}

async fn write_response_locked<W: AsyncWrite + Unpin>(
    writer: &Arc<AsyncMutex<W>>,
    value: Value,
    attachment: Option<&[u8]>,
) -> Result<(), HostError> {
    let mut writer = writer.lock().await;
    write_response(&mut *writer, value, attachment).await
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
        serve_named_pipe(driver, endpoint).await
    }
    #[cfg(unix)]
    {
        serve_unix_socket(driver, endpoint).await
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
    use tokio::net::{UnixListener, UnixStream};

    let path = Path::new(&endpoint);
    if path.exists() {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_socket() {
            return Err(HostError::Protocol(format!(
                "endpoint exists and is not a socket: {endpoint}"
            )));
        }
        match UnixStream::connect(path).await {
            Ok(_) => {
                return Err(HostError::Protocol(format!(
                    "endpoint is already in use: {endpoint}"
                )));
            }
            Err(error) if stale_unix_socket_error(&error) => std::fs::remove_file(path)?,
            Err(error) => return Err(error.into()),
        }
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

#[cfg(unix)]
fn stale_unix_socket_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
    )
}

#[cfg(test)]
mod tests;
