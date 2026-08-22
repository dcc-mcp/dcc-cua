//! Long-lived local Host IPC for dcc-cua.
//!
//! The wire format is a versioned length-prefixed protocol: a big-endian `u32`
//! followed by one UTF-8 JSON object. Screenshot bytes are sent as a second
//! framed payload when binary transport is selected, so control frames stay
//! bounded and the transport does not base64-encode pixels.

mod action_confirmation;
mod action_response;
mod browser_extension;
mod endpoint;
mod error_contract;
mod request_contract;
mod request_handler;
mod session_events;
mod session_identity;
mod session_state;
mod task_grant;
mod wait;
mod wire;
use action_confirmation::{ActionConfirmationOutcome, authorize_action_confirmation};
pub use action_confirmation::{
    TRUSTED_ACTION_CONFIRMATION_SCHEMA, TrustedActionConfirmationAction,
    TrustedActionConfirmationDecision, TrustedActionConfirmationHost,
    TrustedActionConfirmationHostError, TrustedActionConfirmationRequest,
};
use action_response::*;
pub use dcc_cua_protocol::{
    DEFAULT_SESSION_IDLE_TIMEOUT_MS, HOST_PROTOCOL_VERSION, MAX_BINARY_FRAME_BYTES,
    MAX_HOST_CONNECTIONS, MAX_JSON_FRAME_BYTES, MAX_PARALLEL_DISCOVERY_REQUESTS,
    MAX_REQUEST_ID_CHARS, MAX_SESSION_IDLE_TIMEOUT_MS, MAX_SESSIONS_PER_CONNECTION,
    MIN_SESSION_IDLE_TIMEOUT_MS,
};
pub use error_contract::{HostError, HostProtocolErrorCode, HostTransport};
use request_handler::handle_request_with_confirmation_host;
use session_identity::{new_runtime_session_id, rewrite_session_aliases};
use session_state::{
    ConnectionSessions, HostDesktopSession, HostEvidencePublication, HostLaunchSession, HostSession,
};
use task_grant::TaskGrant;
pub use task_grant::{MAX_APPLICATION_LABEL_CHARS, MAX_TASK_GRANT_ID_CHARS};
use wire::*;

pub const HOST_HELLO_TIMEOUT_MS: u64 = 10_000;
pub const MAX_POST_SNAPSHOT_DELAY_MS: u64 = 5_000;
pub const MAX_SESSION_EVENT_POLL_TIMEOUT_MS: u64 = 30_000;
pub const MAX_SESSION_INPUT_EVENTS: usize = 64;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinSet;
use uuid::Uuid;

use dcc_cua_browser::{
    BrowserClickRequest, BrowserDialogRequest, BrowserDownloadRequest, BrowserNavigateRequest,
    BrowserPointerRequest, BrowserPrepareRequest, BrowserSession, BrowserSetInputFilesRequest,
    BrowserSnapshotRequest, BrowserTypeRequest,
};
use dcc_cua_core::{
    ComputerUseAction, ComputerUseClipboardWriteRequest, ComputerUseDesktopSnapshot,
    ComputerUseDriver, ComputerUseError, ComputerUseErrorCode, ComputerUseImage,
    ComputerUseLiveObservationStartRequest, ComputerUseMenuRequest, ComputerUsePoint,
    ComputerUseRecordingHealth, ComputerUseRecordingStartRequest, ComputerUseResult,
    ComputerUseScreenshot, ComputerUseSessionHealth, ComputerUseSessionHealthEvaluation,
    ComputerUseSessionHealthPolicy, ComputerUseSessionStartRequest, ComputerUseSessionStopResult,
    ComputerUseTargetScope, ComputerUseToolResult, ComputerUseWindowFrameRequest,
    ComputerUseWindowQuery, ComputerUseWindowWaitRequest, ComputerUseZoomRequest,
    IndicatorMotionPolicy,
};
use dcc_cua_interrupt::{broadcast_interrupt, interrupt_generation, interrupt_generation_changed};
use dcc_cua_shm::SharedImage;

// ponytail: one OS input stream is process-global; shard only if platforms gain isolated seats.
static RAW_INPUT_QUEUE: AsyncMutex<()> = AsyncMutex::const_new(());

/// Capabilities this implementation actually provides.
pub const HOST_CAPABILITIES: &[&str] = &[
    "exact_window_capabilities",
    "exact_window_state",
    "connection_scoped_sessions",
    "multi_agent_sessions",
    "isolated_runtime_sessions",
    "observation_fencing",
    "action_post_snapshot",
    "action_post_snapshot_delay",
    "semantic_element_tokens",
    "background_first_input_delivery",
    "scoped_raw_input",
    "serialized_raw_input",
    "input_backend:windows.send_input.v1",
    "input_backend:windows.send_input.relative_drag.v1",
    "input_backend:windows.send_input.combined_down_drag.v1",
    "input_backend:windows.synthetic_touch.v1",
    "host_wide_interrupt",
    "accessibility_snapshot",
    "accessibility_find",
    "state_verification",
    "session_state",
    "session_health",
    "session_input_state_events",
    "session_target_state_events",
    "session_escalation",
    "cursor_controls",
    "uia_snapshot_and_actions",
    "windows_background_uia_fallback",
    "zoomed_window_observation",
    "semantic_value_actions",
    "semantic_profile_extensions",
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
    "exact_window_restore_activate",
    "open_session_activate_before",
    "indicator_motion_policy",
    "scoped_window_frame",
    "native_menu_path",
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
    "trusted_confirmation_grants",
    "application_launch",
    "application_terminate",
    "session_scoped_application_lifecycle",
    "clipboard_read",
    "clipboard_write",
    "trajectory_recording",
    "live_observation_latest_frame",
    "browser_exact_binding",
    "browser_prepare",
    "browser_semantic_snapshot",
    "nearest_ancestor_role_v1",
    "browser_typed_actions",
    "browser_file_upload",
    "browser_file_download",
    "browser_dialog",
    "browser_provider:cdp.v1",
    "browser_provider:extension.v1",
    "browser_extension_native_messaging_v1",
    "logical_task_session_idle_timeout",
    "request_correlation",
    "request_cancellation",
    "host_ping",
    "host_diagnostics",
    "pipelined_read_requests",
    "parallel_discovery_requests",
];

/// Return only capabilities backed by the current platform runtime.
#[must_use]
pub fn host_capabilities(cursor_controls_available: bool) -> Vec<&'static str> {
    HOST_CAPABILITIES
        .iter()
        .copied()
        .filter(|capability| {
            (!matches!(*capability, "cursor_controls" | "cua_cursor_marker")
                || cursor_controls_available)
                && (*capability != "windows_background_uia_fallback" || cfg!(windows))
                && (*capability != "exact_window_restore_activate" || cfg!(windows))
                && (!capability.starts_with("input_backend:windows.") || cfg!(windows))
        })
        .collect()
}

fn rewrite_runtime_session_ids(value: &mut Value, sessions: &ConnectionSessions) {
    let aliases =
        sessions
            .windows
            .iter()
            .map(|(public_id, session)| (session.runtime_session_id.as_str(), public_id.as_str()))
            .chain(sessions.desktops.iter().map(|(public_id, session)| {
                (session.runtime_session_id.as_str(), public_id.as_str())
            }))
            .chain(sessions.launches.iter().map(|(public_id, session)| {
                (session.runtime_session_id.as_str(), public_id.as_str())
            }))
            .collect::<Vec<_>>();
    rewrite_session_aliases(value, &aliases);
}

#[derive(Debug, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
enum Request {
    Hello(HelloParams),
    Ping {},
    Doctor {},
    RegisterBrowserExtension {
        hello: Value,
        invocation_origin: String,
        browser_process_id: u32,
    },
    BrowserExtensionNext {
        provider_id: String,
        provider_secret: String,
        #[serde(default = "default_browser_extension_poll_timeout_ms")]
        timeout_ms: u64,
    },
    CompleteBrowserExtension {
        provider_id: String,
        provider_secret: String,
        response: Value,
    },
    UnregisterBrowserExtension {
        provider_id: String,
        provider_secret: String,
    },
    BrowserExtensionStatus {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
    },
    BrowserExtensionCall {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        provider_id: String,
        expected_origin: String,
        method: String,
        #[serde(default = "default_json_object")]
        params: Value,
        #[serde(default = "default_browser_extension_call_timeout_ms")]
        timeout_ms: u64,
    },
    InterruptAll {},
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
        #[serde(default)]
        post_snapshot_delay_ms: u64,
    },
    StopDesktopSession {
        session_id: String,
    },
    LaunchApp {
        session_id: String,
        grant: TaskGrant,
        launch: dcc_cua_core::ComputerUseLaunchRequest,
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
    LiveObservationStart {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        request: ComputerUseLiveObservationStartRequest,
    },
    LiveObservationState {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
    },
    LiveObservationStop {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
    },
    OpenSession {
        session_id: String,
        grant: TaskGrant,
        #[serde(default)]
        activate_before: bool,
        #[serde(default)]
        indicator_motion: IndicatorMotionPolicy,
        #[serde(default = "default_session_idle_timeout_ms")]
        idle_timeout_ms: u64,
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
    SetWindowFrame {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        frame: ComputerUseWindowFrameRequest,
    },
    InvokeMenu {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        request: ComputerUseMenuRequest,
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
        post_snapshot_delay_ms: u64,
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
    GetInputState {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
    },
    SessionHealth {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        #[serde(default)]
        policy: ComputerUseSessionHealthPolicy,
    },
    PollSessionEvents {
        session_id: String,
        task_grant_id: String,
        window_capability: String,
        after_sequence: u64,
        #[serde(default)]
        timeout_ms: u64,
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
            return Err(HostError::coded_protocol(
                HostProtocolErrorCode::ProtocolMismatch,
                format!(
                    "protocol version {} is not supported",
                    params.protocol_version
                ),
            ));
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
#[serde(rename_all = "snake_case")]
enum WindowOperation {
    Activate,
    RestoreActivate,
    Close,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
struct HostAction {
    action: String,
    #[serde(default)]
    element_index: Option<u32>,
    #[serde(default)]
    element_token: Option<String>,
    #[serde(default)]
    delivery_mode: Option<String>,
    #[serde(default)]
    input_backend_id: Option<String>,
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
        const HARD_DENY: [&str; 5] = [
            "terminal_or_run_dialog",
            "credential_or_authentication",
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
        let action = match self.action.as_str() {
            "set_checked" => "set_value".to_owned(),
            "press" | "press_key" => "keypress".to_owned(),
            "hotkey" => "keyboard_shortcut".to_owned(),
            _ => self.action,
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
            input_backend_id: self.input_backend_id,
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
        return Err(HostError::coded_protocol(
            HostProtocolErrorCode::RequestInProgress,
            "wait_for is already running",
        ));
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
        return Err(HostError::coded_protocol(
            HostProtocolErrorCode::RequestInProgress,
            "window_wait is already running for this wait_id",
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
        .ok_or_else(|| {
            HostError::coded_protocol(
                HostProtocolErrorCode::RequestNotFound,
                "no window_wait is running for this wait_id",
            )
        })?;
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
        .ok_or_else(|| {
            HostError::coded_protocol(
                HostProtocolErrorCode::RequestNotFound,
                "no wait_for is running for this session",
            )
        })?;
    if handle.task_grant_id != task_grant_id || handle.window_capability != window_capability {
        return Err(HostError::coded_protocol(
            HostProtocolErrorCode::Forbidden,
            "cancel credentials do not match the running wait_for",
        ));
    }
    handle.cancelled.store(true, Ordering::Release);
    handle.notify.notify_one();
    Ok(json!({"type":"wait_cancel_requested", "session_id":session_id}))
}

/// Run one long-lived host connection over stdio or the platform local endpoint.
pub async fn run(driver: ComputerUseDriver, transport: HostTransport) -> Result<(), HostError> {
    run_internal(driver, transport, None).await
}

/// Run a Host with a constructor-owned action-time confirmation callback.
///
/// The callback is never reachable from Host IPC. `allow_trusted_confirmation`
/// in a task grant only permits a request to reach this boundary; it cannot
/// authorize an action by itself.
pub async fn run_with_confirmation_host(
    driver: ComputerUseDriver,
    transport: HostTransport,
    confirmation_host: Arc<dyn TrustedActionConfirmationHost>,
) -> Result<(), HostError> {
    run_internal(driver, transport, Some(confirmation_host)).await
}

async fn run_internal(
    driver: ComputerUseDriver,
    transport: HostTransport,
    confirmation_host: Option<Arc<dyn TrustedActionConfirmationHost>>,
) -> Result<(), HostError> {
    match transport {
        HostTransport::Stdio => {
            process_connection_parts_with_confirmation_host(
                driver,
                tokio::io::stdin(),
                tokio::io::stdout(),
                Duration::from_millis(HOST_HELLO_TIMEOUT_MS),
                confirmation_host,
            )
            .await
        }
        HostTransport::Endpoint(endpoint) => {
            endpoint::serve(driver, endpoint, confirmation_host).await
        }
    }
}

async fn process_connection_with_confirmation_host<S>(
    driver: ComputerUseDriver,
    stream: S,
    confirmation_host: Option<Arc<dyn TrustedActionConfirmationHost>>,
) -> Result<(), HostError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (reader, writer) = tokio::io::split(stream);
    process_connection_parts_with_confirmation_host(
        driver,
        reader,
        writer,
        Duration::from_millis(HOST_HELLO_TIMEOUT_MS),
        confirmation_host,
    )
    .await
}

async fn process_connection_parts_with_confirmation_host<R, W>(
    driver: ComputerUseDriver,
    reader: R,
    writer: W,
    hello_timeout: Duration,
    confirmation_host: Option<Arc<dyn TrustedActionConfirmationHost>>,
) -> Result<(), HostError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut reader = reader;
    let writer = Arc::new(AsyncMutex::new(writer));
    let mut parallel_tasks = JoinSet::new();
    let mut snapshot_transport = None;
    let mut sessions = ConnectionSessions::default();
    let mut desktop_shared_image = None;
    let cancellation_registry = Arc::new(Mutex::new(HashMap::new()));
    let hello_deadline = tokio::time::Instant::now() + hello_timeout;
    let mut observed_interrupt_generation = interrupt_generation();

    let connection_result = async {
        while let Some(frame) = if snapshot_transport.is_none() {
            tokio::time::timeout_at(
                hello_deadline,
                read_frame(&mut reader, MAX_JSON_FRAME_BYTES),
            )
            .await
            .map_err(|_| {
                HostError::Protocol(format!(
                    "hello was not completed within {} ms",
                    hello_timeout.as_millis()
                ))
            })??
        } else {
            read_frame_with_interrupt_poll(
                &mut reader,
                &mut sessions,
                &mut observed_interrupt_generation,
            )
            .await?
        } {
        reap_completed_parallel_requests(&mut parallel_tasks);
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

        if is_interruptible_connection_request(&request) {
            let mut operation = Box::pin(handle_request_with_confirmation_host(
                &driver,
                confirmation_host.as_deref(),
                &mut sessions,
                &mut snapshot_transport,
                &mut desktop_shared_image,
                &cancellation_registry,
                request,
            ));
            let mut interrupt_poll =
                tokio::time::interval(std::time::Duration::from_millis(50));
            interrupt_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    result = &mut operation => {
                        let (mut response, attachment) = match result {
                            Ok(result) => result,
                            Err(error) => (host_error_response(&error), None),
                        };
                        drop(operation);
                        rewrite_runtime_session_ids(&mut response, &sessions);
                        write_response_locked(
                            &writer,
                            with_request_id(response, request_id.as_deref()),
                            attachment.as_deref(),
                        ).await?;
                        break;
                    }
                    _ = interrupt_poll.tick() => {
                        let current = interrupt_generation();
                        if interrupt_generation_changed(observed_interrupt_generation, current) {
                            drop(operation);
                            stop_connection_control_sessions(&mut sessions).await;
                            observed_interrupt_generation = current;
                            write_json_locked(
                                &writer,
                                with_request_id(
                                    host_error_response(&HostError::ComputerUse(ComputerUseError::new(
                                        ComputerUseErrorCode::UserInterrupted,
                                        "Escape or a shared Host stop interrupted the in-flight request",
                                    ))),
                                    request_id.as_deref(),
                                ),
                            )
                            .await?;
                            break;
                        }
                    }
                    frame = read_frame(&mut reader, MAX_JSON_FRAME_BYTES) => {
                        let Some(frame) = frame? else {
                            drop(operation);
                            return Ok(());
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
                        if matches!(&next_request, Request::InterruptAll {}) {
                            drop(operation);
                            let (mut response, attachment) =
                                match handle_request_with_confirmation_host(
                                    &driver,
                                    confirmation_host.as_deref(),
                                    &mut sessions,
                                    &mut snapshot_transport,
                                    &mut desktop_shared_image,
                                    &cancellation_registry,
                                    Request::InterruptAll {},
                                )
                                .await
                                {
                                    Ok(result) => result,
                                    Err(error) => (host_error_response(&error), None),
                                };
                            observed_interrupt_generation = interrupt_generation();
                            rewrite_runtime_session_ids(&mut response, &sessions);
                            write_response_locked(
                                &writer,
                                with_request_id(response, cancel_id.as_deref()),
                                attachment.as_deref(),
                            )
                            .await?;
                            write_json_locked(
                                &writer,
                                with_request_id(
                                    host_error_response(&HostError::ComputerUse(ComputerUseError::new(
                                        ComputerUseErrorCode::UserInterrupted,
                                        "the shared Host stop interrupted the in-flight request",
                                    ))),
                                    request_id.as_deref(),
                                ),
                            )
                            .await?;
                            break;
                        } else if let Request::CancelWindowWait { wait_id } = next_request {
                            let response = match cancel_window_wait(&cancellation_registry, &wait_id) {
                                Ok(response) => response,
                                Err(error) => host_error_response(&error),
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
                                Err(error) => host_error_response(&error),
                            };
                            write_json_locked(&writer, with_request_id(response, cancel_id.as_deref())).await?;
                        } else {
                            write_json_locked(
                                &writer,
                                with_request_id(
                                    error_response("request_in_progress", "an interruptible wait or event poll is still running; cancel or interrupt it before sending another request"),
                                    cancel_id.as_deref(),
                                ),
                            ).await?;
                        }
                    }
                }
            }
            drop(window_wait_guard);
        } else if is_parallel_request(&request) {
            await_parallel_request_capacity(&mut parallel_tasks).await;
            let task_driver = driver.clone();
            let task_writer = writer.clone();
            parallel_tasks.spawn(async move {
                let (response, attachment) =
                    match handle_parallel_request(&task_driver, request).await {
                        Ok(result) => result,
                        Err(error) => (host_error_response(&error), None),
                    };
                write_response_locked(
                    &task_writer,
                    with_request_id(response, request_id.as_deref()),
                    attachment.as_deref(),
                )
                .await
            });
        } else {
            let (mut response, attachment) = match Box::pin(handle_request_with_confirmation_host(
                &driver,
                confirmation_host.as_deref(),
                &mut sessions,
                &mut snapshot_transport,
                &mut desktop_shared_image,
                &cancellation_registry,
                request,
            ))
            .await
            {
                Ok(result) => result,
                Err(error) => (host_error_response(&error), None),
            };
            rewrite_runtime_session_ids(&mut response, &sessions);
            write_response_locked(
                &writer,
                with_request_id(response, request_id.as_deref()),
                attachment.as_deref(),
            )
            .await?;
        }
        }
        Ok::<(), HostError>(())
    }
    .await;

    finalize_connection(
        connection_result,
        &mut parallel_tasks,
        cleanup_sessions(&driver, sessions),
    )
    .await
}

async fn read_frame_with_interrupt_poll<R>(
    reader: &mut R,
    sessions: &mut ConnectionSessions,
    observed_generation: &mut u64,
) -> Result<Option<Vec<u8>>, HostError>
where
    R: AsyncRead + Unpin,
{
    let mut pending_frame = Box::pin(read_frame(reader, MAX_JSON_FRAME_BYTES));
    let mut interrupt_poll = tokio::time::interval(Duration::from_millis(50));
    interrupt_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut session_state_poll = tokio::time::interval(Duration::from_millis(250));
    session_state_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            frame = &mut pending_frame => return frame,
            _ = interrupt_poll.tick() => {
                let current = interrupt_generation();
                if interrupt_generation_changed(*observed_generation, current) {
                    stop_connection_control_sessions(sessions).await;
                    *observed_generation = current;
                }
            }
            _ = session_state_poll.tick() => {
                reap_idle_window_sessions(&mut sessions.windows).await;
                refresh_connection_session_states(&mut sessions.windows).await;
            }
        }
    }
}

const fn default_session_idle_timeout_ms() -> u64 {
    DEFAULT_SESSION_IDLE_TIMEOUT_MS
}

const fn default_browser_extension_poll_timeout_ms() -> u64 {
    5_000
}

const fn default_browser_extension_call_timeout_ms() -> u64 {
    10_000
}

fn default_json_object() -> Value {
    json!({})
}

async fn reap_idle_window_sessions(sessions: &mut HashMap<String, HostSession>) {
    let expired = sessions
        .iter()
        .filter(|(_, session)| session.is_idle_expired())
        .map(|(session_id, _)| session_id.clone())
        .collect::<Vec<_>>();
    for session_id in expired {
        if let Some(mut session) = sessions.remove(&session_id) {
            let _ = session.session.stop().await;
            session.invalidate_observations();
        }
    }
}

async fn refresh_connection_session_states(sessions: &mut HashMap<String, HostSession>) {
    let (readiness, observed_at) = session_events::input_readiness_sample();
    for session in sessions.values_mut() {
        session.observe_input_readiness(readiness.clone(), observed_at);
        let availability = session.session.target_availability().await;
        if let Ok(availability) = session.finish_observation_sensitive_attempt(availability) {
            session.observe_target_availability(availability);
        }
    }
}

async fn stop_connection_control_sessions(sessions: &mut ConnectionSessions) {
    for session in sessions.windows.values_mut() {
        if session.interrupted {
            continue;
        }
        session.interrupted = true;
        let _ = session.session.stop().await;
        session.invalidate_observations();
    }
    for session in sessions.desktops.values_mut() {
        if session.interrupted {
            continue;
        }
        session.interrupted = true;
        let _ = session.session.stop().await;
    }
}

async fn finalize_connection(
    result: Result<(), HostError>,
    parallel_tasks: &mut JoinSet<Result<(), HostError>>,
    cleanup: impl std::future::Future<Output = Result<(), HostError>>,
) -> Result<(), HostError> {
    parallel_tasks.abort_all();
    while parallel_tasks.join_next().await.is_some() {}
    let cleanup_result = cleanup.await;
    match result {
        Ok(()) => cleanup_result,
        Err(error) => Err(error),
    }
}

fn reap_completed_parallel_requests(tasks: &mut JoinSet<Result<(), HostError>>) {
    while tasks.try_join_next().is_some() {}
}

async fn await_parallel_request_capacity(tasks: &mut JoinSet<Result<(), HostError>>) {
    reap_completed_parallel_requests(tasks);
    if tasks.len() >= MAX_PARALLEL_DISCOVERY_REQUESTS {
        let _ = tasks.join_next().await;
        reap_completed_parallel_requests(tasks);
    }
}

async fn cleanup_sessions(
    driver: &ComputerUseDriver,
    mut sessions: ConnectionSessions,
) -> Result<(), HostError> {
    stop_sessions(
        driver,
        &mut sessions.windows,
        &mut sessions.desktops,
        &mut sessions.launches,
    )
    .await;
    Ok(())
}

async fn stop_sessions(
    driver: &ComputerUseDriver,
    sessions: &mut HashMap<String, HostSession>,
    desktop_sessions: &mut HashMap<String, HostDesktopSession>,
    launch_sessions: &mut HashMap<String, HostLaunchSession>,
) -> (usize, usize, usize) {
    let counts = (
        sessions.len(),
        desktop_sessions.len(),
        launch_sessions.len(),
    );
    for (_, mut session) in sessions.drain() {
        let _ = session.session.stop().await;
    }
    for (_, mut session) in desktop_sessions.drain() {
        let _ = session.session.stop().await;
    }
    for (_, session) in launch_sessions.drain() {
        let _ = driver.end_launch_session(&session.runtime_session_id).await;
    }
    counts
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

fn is_interruptible_connection_request(request: &Request) -> bool {
    matches!(
        request,
        Request::WaitFor { .. } | Request::WaitForWindow(_) | Request::PollSessionEvents { .. }
    )
}

fn interrupt_poll_slice(remaining: Duration) -> Duration {
    remaining.min(Duration::from_millis(50))
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
    let prepared = prepare_image_transport(vec![image], mode, shared_image)?;
    let mut descriptor = prepared
        .primary
        .expect("one verification image must have a descriptor");
    if descriptor["encoding"] == "binary_frame" {
        descriptor["name"] = Value::String(String::new());
        descriptor["id"] = Value::String(String::new());
    }
    Ok((descriptor, prepared.attachment))
}

fn browser_response(
    response_type: &str,
    session_id: String,
    result: dcc_cua_browser::BrowserResult,
    mode: SnapshotTransport,
    shared_image: &mut Option<SharedImage>,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let mut response_result = result.value;
    let prepared = prepare_image_transport(result.images, mode, shared_image)?;
    prepared.annotate_content(&mut response_result);

    let mut response = json!({
        "type": response_type,
        "session_id": session_id,
        "result": response_result,
    });
    if let Some(image) = prepared.primary {
        response["image"] = image;
    }
    if prepared.attachments.len() > 1 {
        response["attachments"] = Value::Array(prepared.attachments);
    }
    Ok((response, prepared.attachment))
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
    let session = session_with_capability(sessions, session_id, grant_id, capability)?;
    if session.is_idle_expired() {
        let _ = session.session.stop().await;
        session.interrupted = true;
        session.invalidate_observations();
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::SessionRefreshRequired,
            "session idle timeout expired; open a new logical-task session",
        )
        .into());
    }
    ensure_session_not_interrupted(session).await?;
    session.mark_activity();
    Ok(session)
}

fn session_with_capability<'a>(
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

async fn ensure_session_not_interrupted(session: &mut HostSession) -> Result<(), HostError> {
    if session.interrupted {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::UserInterrupted,
            "Escape or a shared Host stop interrupted this session; the session is stopped",
        )
        .into());
    }
    if session.session.control_banner_interrupted() {
        let cleanup_note = session
            .session
            .stop()
            .await
            .err()
            .map(|error| format!("; CUA cleanup also failed: {error}"))
            .unwrap_or_default();
        session.interrupted = true;
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::UserInterrupted,
            format!(
                "Escape, the safety banner, or a shared Host stop interrupted this session; the session was stopped{cleanup_note}"
            ),
        )
        .into());
    }
    if let Some(failure) = session.session.control_banner_failure() {
        let cleanup_note = session
            .session
            .stop()
            .await
            .err()
            .map(|error| format!("; CUA cleanup also failed: {error}"))
            .unwrap_or_default();
        return Err(stopped_banner_failure(failure, &cleanup_note).into());
    }
    Ok(())
}

async fn wait_for_window_post_snapshot_delay(
    session: &mut HostSession,
    delay: Duration,
) -> Result<(), HostError> {
    let deadline = tokio::time::Instant::now() + delay;
    loop {
        ensure_session_not_interrupted(session).await?;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        tokio::time::sleep(interrupt_poll_slice(remaining)).await;
    }
}

fn stopped_banner_failure(failure: ComputerUseError, cleanup_note: &str) -> ComputerUseError {
    ComputerUseError::new(
        failure.code,
        format!(
            "the visible control banner failed and the session was stopped: {}{cleanup_note}",
            failure.message
        ),
    )
}

async fn authorized_desktop_session<'a>(
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
    ensure_desktop_session_not_interrupted(session).await?;
    Ok(session)
}

async fn ensure_desktop_session_not_interrupted(
    session: &mut HostDesktopSession,
) -> Result<(), HostError> {
    if session.interrupted {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::UserInterrupted,
            "Escape or a shared Host stop interrupted this desktop session; the session is stopped",
        )
        .into());
    }
    if interrupt_generation_changed(session.interrupt_generation, interrupt_generation()) {
        let cleanup_note = session
            .session
            .stop()
            .await
            .err()
            .map(|error| format!("; CUA cleanup also failed: {error}"))
            .unwrap_or_default();
        session.interrupted = true;
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::UserInterrupted,
            format!("a shared Host stop interrupted this desktop session{cleanup_note}"),
        )
        .into());
    }
    Ok(())
}

async fn wait_for_desktop_post_snapshot_delay(
    session: &mut HostDesktopSession,
    delay: Duration,
) -> Result<(), HostError> {
    let deadline = tokio::time::Instant::now() + delay;
    loop {
        ensure_desktop_session_not_interrupted(session).await?;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        tokio::time::sleep(interrupt_poll_slice(remaining)).await;
    }
}

#[cfg(test)]
mod tests;
