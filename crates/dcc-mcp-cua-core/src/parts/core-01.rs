use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use base64::Engine;
use cua_driver_sdk::CuaDriver;
pub use cua_driver_sdk::{
    ConfiguredDriverOptions, DriverAuthorizationAction, DriverAuthorizationDecision,
    DriverAuthorizationHost, DriverAuthorizationHostError, DriverAuthorizationRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

const MAX_TEXT_UTF16_UNITS: usize = 4_096;
const MAX_TYPE_CHAR_DELAY_MS: u64 = 1_000;
const MAX_KEY_TOKENS: usize = 16;
const MAX_MODIFIER_TOKENS: usize = 8;
const MAX_MODIFIER_CHARS: usize = 32;
const MAX_DRAG_POINTS: usize = 256;
const MAX_DRAG_DURATION_MS: u64 = 10_000;
const MAX_DRAG_STEPS: u32 = 200;
const MAX_ELEMENT_TOKEN_CHARS: usize = 512;
const MAX_LAUNCH_ARGUMENTS: usize = 32;
const MAX_LAUNCH_URLS: usize = 16;
const MAX_LOCAL_PATH_CHARS: usize = 4_096;
const MAX_WINDOW_QUERY_CHARS: usize = 512;
const MAX_WINDOW_WAIT_TIMEOUT_MS: u64 = 30_000;
const MAX_WINDOW_WAIT_INTERVAL_MS: u64 = 1_000;
const MAX_NATIVE_TOOL_NAME_CHARS: usize = 128;
const MAX_NATIVE_TOOL_ARGUMENT_BYTES: usize = 1 * 1024 * 1024;
const MAX_NATIVE_TOOL_IMAGES: usize = 8;
const MAX_NATIVE_TOOL_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_NATIVE_TOOL_TOTAL_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MOUSE_CURSOR_THEME: &str = "cua.default";
static OBSERVATION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Exact identity supplied by the adapter/runtime. Agent input cannot widen it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseTargetScope {
    pub process_id: Option<u32>,
    pub window_handle: Option<u64>,
    pub window_title: Option<String>,
}

impl ComputerUseTargetScope {
    pub fn validate(&self) -> ComputerUseResult<()> {
        if self.process_id.is_none() && self.window_handle.is_none() && self.window_title.is_none()
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::MissingWindow,
                "Computer Use requires an exact process_id, window_handle, or window_title",
            ));
        }
        if self
            .window_title
            .as_deref()
            .is_some_and(|title| title.is_empty() || title.chars().count() > MAX_WINDOW_QUERY_CHARS)
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                format!("window_title must contain 1..{MAX_WINDOW_QUERY_CHARS} characters"),
            ));
        }
        Ok(())
    }
}

/// Bounded selector used to wait for a native window to become available.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseWindowQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_handle: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    #[serde(default)]
    pub on_screen_only: bool,
}

impl ComputerUseWindowQuery {
    pub fn validate_selectors(&self) -> ComputerUseResult<()> {
        if self
            .app
            .as_deref()
            .is_some_and(|value| value.is_empty())
            || self
                .window_title
                .as_deref()
                .is_some_and(|value| value.is_empty())
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                "window query selectors cannot be empty",
            ));
        }
        for (name, value) in [
            ("app", self.app.as_deref()),
            ("window_title", self.window_title.as_deref()),
        ] {
            if value.is_some_and(|value| value.chars().count() > MAX_WINDOW_QUERY_CHARS) {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::InvalidTarget,
                    format!("{name} exceeds {MAX_WINDOW_QUERY_CHARS} characters"),
                ));
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> ComputerUseResult<()> {
        self.validate_selectors()?;
        if self.app.is_none()
            && self.process_id.is_none()
            && self.window_handle.is_none()
            && self.window_title.is_none()
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::MissingWindow,
                "window query requires app, process_id, window_handle, or window_title",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn matches_window(&self, window: &Value) -> bool {
        self.app.as_deref().is_none_or(|app| {
            window["app_name"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(app))
        }) && self
            .process_id
            .is_none_or(|pid| window["pid"] == json!(pid))
            && self
                .window_handle
                .is_none_or(|handle| window["window_id"] == json!(handle))
            && self
                .window_title
                .as_deref()
                .is_none_or(|title| window["title"].as_str() == Some(title))
    }
}

/// Request for a bounded poll of the native window inventory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseWindowWaitRequest {
    pub query: ComputerUseWindowQuery,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub interval_ms: Option<u64>,
}

impl ComputerUseWindowWaitRequest {
    fn limits(&self) -> ComputerUseResult<(u64, u64)> {
        self.query.validate()?;
        Ok((
            self.timeout_ms.unwrap_or(5_000).min(MAX_WINDOW_WAIT_TIMEOUT_MS),
            self.interval_ms
                .unwrap_or(100)
                .clamp(10, MAX_WINDOW_WAIT_INTERVAL_MS),
        ))
    }
}

/// One native action in screenshot coordinates.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ComputerUseAction {
    pub action: String,
    #[serde(default)]
    pub observation_id: Option<String>,
    #[serde(default)]
    pub element_index: Option<u32>,
    #[serde(default)]
    pub element_token: Option<String>,
    #[serde(default)]
    pub delivery_mode: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub button: Option<String>,
    pub scroll_x: Option<i32>,
    pub scroll_y: Option<i32>,
    #[serde(default)]
    pub path: Vec<ComputerUsePoint>,
    pub text: Option<String>,
    #[serde(default)]
    pub delay_ms: Option<u64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub steps: Option<u32>,
    #[serde(default)]
    pub type_chars_only: bool,
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub modifiers: Vec<String>,
}

/// A bounded crop of the latest exact-window observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputerUseZoomRequest {
    pub observation_id: String,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ComputerUsePoint {
    pub x: f64,
    pub y: f64,
}

/// Metadata binding model coordinates to one fresh target capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputerUseObservation {
    pub observation_id: String,
    pub window_handle: u64,
    pub process_id: u32,
    pub window_title: String,
    pub width: u32,
    pub height: u32,
    pub source_rect: [i32; 4],
    pub capture_backend: String,
    pub capture_provenance: Value,
    pub session_id: String,
}

/// Exact CUA application selector used by the host and CLI launch surface.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComputerUseLaunchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aumid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_arguments: Vec<String>,
    #[serde(default)]
    pub creates_new_application_instance: bool,
    #[serde(default)]
    pub start_minimized: bool,
}

/// One explicit value that may replace the system clipboard.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComputerUseClipboardWriteRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
}

/// A bounded recording destination owned by the caller's task grant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputerUseRecordingStartRequest {
    pub output_dir: String,
    #[serde(default)]
    pub record_video: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputerUseScreenshot {
    pub data: Vec<u8>,
    pub observation: ComputerUseObservation,
    pub accessibility: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputerUseDesktopSnapshot {
    pub data: Vec<u8>,
    pub state: Value,
    pub observation_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputerUseImage {
    pub data: Vec<u8>,
    pub mime_type: String,
}

/// Result of an open-ended CUA SDK tool call. The raw MCP result stays
/// available for extension tools; image pixels are decoded separately so a
/// Host can forward them as binary frames instead of base64 JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct ComputerUseToolResult {
    pub value: Value,
    pub text: String,
    pub images: Vec<ComputerUseImage>,
    pub degraded: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputerUseVerification {
    pub value: Value,
    pub image: Option<ComputerUseImage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerUseErrorCode {
    BackendUnavailable,
    BrowserRefused,
    ClipboardRefused,
    RecordingRefused,
    MissingWindow,
    InvalidTarget,
    StaleObservation,
    UserInterrupted,
    InvalidAction,
    InputFailed,
    CaptureFailed,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{code:?}: {message}")]
pub struct ComputerUseError {
    pub code: ComputerUseErrorCode,
    pub message: String,
}

impl ComputerUseError {
    pub fn new(code: ComputerUseErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub type ComputerUseResult<T> = Result<T, ComputerUseError>;

/// The visible marker carried by CUA's color-coded cursor/session badge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseMarker {
    pub visible: bool,
    pub label: String,
    pub backend: &'static str,
}

/// One shared CUA runtime. Create it once per host process.
#[derive(Clone)]
pub struct ComputerUseDriver {
    driver: Arc<CuaDriver>,
    // ponytail: cache the static tool registry for this driver; recreate the
    // driver if a future SDK supports runtime tool registration.
    tool_inventory: Arc<tokio::sync::OnceCell<Value>>,
}

impl ComputerUseDriver {
    pub fn create() -> ComputerUseResult<Self> {
        CuaDriver::create(None)
            .map(|driver| Self {
                driver,
                tool_inventory: Arc::new(tokio::sync::OnceCell::new()),
            })
            .map_err(|error| map_driver_error("create CUA runtime", error))
    }

    /// Create a configured runtime with a trusted host authorization callback.
    ///
    /// The callback is constructor-only and must be owned by Core or another
    /// trusted embedding host. It is never exposed through CUA tools or Host
    /// IPC, and returning `Allow` must require an explicit user decision.
    pub fn create_with_authorization_host(
        options: ConfiguredDriverOptions,
        host: Arc<dyn DriverAuthorizationHost>,
    ) -> ComputerUseResult<Self> {
        CuaDriver::create_configured_with_authorization_host(options, host)
            .map(|driver| Self {
                driver,
                tool_inventory: Arc::new(tokio::sync::OnceCell::new()),
            })
            .map_err(|error| map_driver_error("create authorized CUA runtime", error))
    }

    pub fn session(
        &self,
        scope: ComputerUseTargetScope,
        app_name: impl Into<String>,
        session_id: impl Into<String>,
    ) -> ComputerUseResult<ComputerUseSession> {
        ComputerUseSession::new(self.clone(), scope, app_name.into(), session_id.into())
    }

    pub fn desktop_session(
        &self,
        session_id: impl Into<String>,
    ) -> ComputerUseResult<ComputerUseDesktopSession> {
        ComputerUseDesktopSession::new(self.clone(), session_id.into())
    }

    pub fn raw(&self) -> &Arc<CuaDriver> {
        &self.driver
    }

    /// List the currently visible native windows through the CUA runtime.
    pub async fn list_windows(&self) -> ComputerUseResult<Vec<Value>> {
        self.list_windows_filtered(None, false).await
    }

    /// Enumerate windows with filters applied by the native CUA backend.
    pub async fn list_windows_filtered(
        &self,
        pid: Option<u32>,
        on_screen_only: bool,
    ) -> ComputerUseResult<Vec<Value>> {
        list_windows_with_driver(&self.driver, pid, on_screen_only).await
    }

    /// Wait until the bounded native window query returns at least one row.
    pub async fn wait_for_window(
        &self,
        request: &ComputerUseWindowWaitRequest,
    ) -> ComputerUseResult<Value> {
        let (timeout_ms, interval_ms) = request.limits()?;
        let started = Instant::now();
        loop {
            let mut windows = self
                .list_windows_filtered(request.query.process_id, request.query.on_screen_only)
                .await?;
            windows.retain(|window| request.query.matches_window(window));
            if !windows.is_empty() {
                return Ok(json!({
                    "windows": windows,
                    "count": windows.len(),
                    "waited_ms": started.elapsed().as_millis(),
                }));
            }
            if started.elapsed() >= Duration::from_millis(timeout_ms) {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::MissingWindow,
                    "window query timed out",
                ));
            }
            tokio::time::sleep(Duration::from_millis(interval_ms)).await;
        }
    }

    /// Enumerate installed and currently running applications through CUA.
    pub async fn list_apps(&self) -> ComputerUseResult<Value> {
        let value = self.call_tool_value("list_apps", json!({})).await?;
        value["structuredContent"]
            .as_object()
            .cloned()
            .map(Value::Object)
            .ok_or_else(|| {
                ComputerUseError::new(
                    ComputerUseErrorCode::BackendUnavailable,
                    "CUA list_apps omitted structuredContent",
                )
            })
    }

    /// Return the CUA tool inventory for adapter capability discovery.
    /// The registry is cached for the lifetime of this shared driver so Host
    /// calls do not re-fetch it just to validate one tool schema.
    pub async fn list_tools(&self) -> ComputerUseResult<Value> {
        Ok(self.cached_tool_inventory().await?.clone())
    }

    /// Call a non-window-bound CUA tool from the local CLI surface.
    /// Window-bound tools must use an exact `ComputerUseSession` instead.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        validate_native_tool_request(name, &arguments)?;
        self.call_unscoped_tool(name, arguments).await
    }

    /// Call a CUA tool that is not bound to an exact native window.
    ///
    /// Host IPC callers must use the grant-gated global route. Dedicated
    /// application, input, browser, recording, and lifecycle routes stay out
    /// of this escape hatch.
    pub async fn call_global_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        validate_native_tool_request(name, &arguments)?;
        if !native_tool_allowed_globally(name) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!("CUA tool {name:?} must use its dedicated or window-bound route"),
            ));
        }
        self.call_unscoped_tool(name, arguments).await
    }

    async fn call_unscoped_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        let schema = self.tool_schema(name).await?;
        if schema["properties"].get("pid").is_some()
            || schema["properties"].get("window_id").is_some()
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                format!("CUA tool {name:?} requires an exact window session"),
            ));
        }
        let result = self
            .driver
            .call_tool(name.to_owned(), arguments.to_string())
            .await
            .map_err(|error| map_driver_error(&format!("call CUA {name}"), error))?;
        ensure_tool_ok(&format!("call CUA {name}"), &result)?;
        native_tool_result(result)
    }

    /// Capture the full desktop without widening any window-scoped session.
    pub async fn desktop_snapshot(&self) -> ComputerUseResult<ComputerUseDesktopSnapshot> {
        self.desktop_snapshot_for(None).await
    }

    async fn desktop_snapshot_for(
        &self,
        session: Option<&str>,
    ) -> ComputerUseResult<ComputerUseDesktopSnapshot> {
        let mut arguments = json!({});
        if let Some(session) = session {
            arguments["session"] = Value::String(session.to_owned());
        }
        let result = self
            .driver
            .call_tool("get_desktop_state".into(), arguments.to_string())
            .await
            .map_err(|error| map_driver_error("capture CUA desktop state", error))?;
        ensure_tool_ok("capture CUA desktop state", &result)?;
        let image = result.images.first().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "CUA desktop state returned no screenshot",
            )
        })?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(&image.data_base64)
            .map_err(|error| {
                ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
            })?;
        let state = result
            .structured_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_else(|| json!({}));
        Ok(ComputerUseDesktopSnapshot {
            data,
            state,
            observation_id: format!(
                "desktop-{}",
                OBSERVATION_COUNTER.fetch_add(1, Ordering::Relaxed)
            ),
        })
    }

    pub async fn screen_size(&self) -> ComputerUseResult<Value> {
        self.call_tool_value("get_screen_size", json!({})).await
    }

    pub async fn cursor_position(&self) -> ComputerUseResult<Value> {
        self.call_tool_value("get_cursor_position", json!({})).await
    }

    /// Launch one explicitly selected application through CUA's platform backend.
    pub async fn launch_app(&self, request: &ComputerUseLaunchRequest) -> ComputerUseResult<Value> {
        validate_launch_request(request)?;
        let arguments = serde_json::to_value(request).map_err(|error| {
            ComputerUseError::new(ComputerUseErrorCode::InvalidAction, error.to_string())
        })?;
        self.call_tool_value("launch_app", arguments).await
    }

    async fn call_tool_value(&self, name: &str, arguments: Value) -> ComputerUseResult<Value> {
        let result = self
            .driver
            .call_tool(name.to_owned(), arguments.to_string())
            .await
            .map_err(|error| map_driver_error(&format!("call CUA {name}"), error))?;
        ensure_tool_ok(&format!("call CUA {name}"), &result)?;
        serde_json::from_str(&result.raw_json).map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA {name} returned invalid JSON: {error}"),
            )
        })
    }

    async fn tool_schema(&self, name: &str) -> ComputerUseResult<Value> {
        tool_schema_from_inventory(self.cached_tool_inventory().await?, name)
    }

    async fn cached_tool_inventory(&self) -> ComputerUseResult<&Value> {
        self.tool_inventory
            .get_or_try_init(|| async {
                let raw = self
                    .driver
                    .list_tools_json()
                    .await
                    .map_err(|error| map_driver_error("list CUA tools", error))?;
                serde_json::from_str(&raw).map_err(|error| {
                    ComputerUseError::new(
                        ComputerUseErrorCode::BackendUnavailable,
                        format!("CUA tool inventory returned invalid JSON: {error}"),
                    )
                })
            })
            .await
    }
}

fn tool_schema_from_inventory(inventory: &Value, name: &str) -> ComputerUseResult<Value> {
    inventory["tools"]
        .as_array()
        .and_then(|tools| {
            tools
                .iter()
                .find(|tool| tool["name"].as_str() == Some(name))
        })
        .and_then(|tool| tool.get("inputSchema"))
        .cloned()
        .ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!("CUA tool {name:?} is not present in the live inventory"),
            )
    })
}

async fn enable_session_marker(
    driver: &ComputerUseDriver,
    session_id: &str,
    context: &str,
) -> ComputerUseResult<()> {
    let result = driver
        .driver
        .call_tool(
            "set_agent_cursor_enabled".into(),
            json!({"session": session_id, "enabled": true}).to_string(),
        )
        .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            cleanup_started_session(driver, session_id).await;
            return Err(map_driver_error(context, error));
        }
    };
    if let Err(error) = ensure_tool_ok(context, &result) {
        cleanup_started_session(driver, session_id).await;
        return Err(error);
    }
    Ok(())
}

async fn cleanup_started_session(driver: &ComputerUseDriver, session_id: &str) {
    let _ = driver
        .driver
        .call_tool(
            "end_session".into(),
            json!({"session": session_id}).to_string(),
        )
        .await;
}

/// A long-lived, exact-window Computer Use session.
pub struct ComputerUseSession {
    driver: ComputerUseDriver,
    scope: ComputerUseTargetScope,
    app_name: String,
    session_id: String,
    marker: ComputerUseMarker,
    target: Option<WindowTarget>,
    observation: Option<ComputerUseObservation>,
    active: bool,
    escalated: bool,
}

/// A bounded desktop-scope session for screen-absolute discovery and input.
/// Window-scoped sessions remain the preferred path for semantic controls.
pub struct ComputerUseDesktopSession {
    driver: ComputerUseDriver,
    session_id: String,
    marker: ComputerUseMarker,
    active: bool,
    latest_observation_id: Option<String>,
}

impl std::fmt::Debug for ComputerUseDesktopSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ComputerUseDesktopSession")
            .field("session_id", &self.session_id)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl ComputerUseDesktopSession {
    fn new(driver: ComputerUseDriver, session_id: String) -> ComputerUseResult<Self> {
        if session_id.trim().is_empty() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "desktop session_id must not be empty",
            ));
        }
        Ok(Self {
            driver,
            session_id,
            marker: ComputerUseMarker {
                visible: false,
                label: "DCC UI Control · Desktop · Esc to stop".into(),
                backend: "cua-driver-sdk",
            },
            active: false,
            latest_observation_id: None,
        })
    }

    pub async fn start(&mut self) -> ComputerUseResult<Value> {
        if self.active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "desktop session is already active",
            ));
        }
        let result = self
            .driver
            .driver
            .call_tool(
                "start_session".into(),
                json!({
                    "session": self.session_id,
                    "capture_scope": "desktop",
                    "cursor_theme": {"theme_id": MOUSE_CURSOR_THEME, "reduced_motion": "auto"},
                    "_public_session_label": self.marker.label,
                })
                .to_string(),
            )
            .await
            .map_err(|error| map_driver_error("start CUA desktop session", error))?;
        ensure_tool_ok("start CUA desktop session", &result)?;
        enable_session_marker(
            &self.driver,
            &self.session_id,
            "show CUA desktop marker",
        )
        .await?;
        self.active = true;
        self.marker.visible = true;
        Ok(json!({"success": true, "active": true, "marker": self.marker}))
    }

    pub async fn screenshot(&mut self) -> ComputerUseResult<ComputerUseDesktopSnapshot> {
        if !self.active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "desktop session is not active",
            ));
        }
        let snapshot = self
            .driver
            .desktop_snapshot_for(Some(&self.session_id))
            .await?;
        self.latest_observation_id = Some(snapshot.observation_id.clone());
        Ok(snapshot)
    }

    pub async fn perform_action(
        &mut self,
        action: &ComputerUseAction,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        if !self.active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "desktop session is not active",
            ));
        }
        validate_action(action)?;
        if action.element_index.is_some()
            || matches!(
                action.action.as_str(),
                "set_text" | "set_value" | "type_chars"
            )
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "desktop actions support only screen-coordinate input",
            ));
        }
        if self.latest_observation_id.as_deref() != action.observation_id.as_deref() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "take a fresh desktop snapshot before acting",
            ));
        }
        let arguments = desktop_action_arguments(action, &self.session_id);
        let tool = arguments["_tool"].as_str().unwrap_or_default().to_owned();
        let mut arguments = arguments;
        arguments
            .as_object_mut()
            .expect("desktop action arguments are an object")
            .remove("_tool");
        let result = self
            .driver
            .driver
            .call_tool(tool.clone(), arguments.to_string())
            .await
            .map_err(|error| map_driver_error(&format!("execute desktop CUA {tool}"), error))?;
        ensure_tool_ok(&format!("execute desktop CUA {tool}"), &result)?;
        let mut result = native_tool_result(result)?;
        self.latest_observation_id = None;
        result.value = json!({
            "success": true,
            "action": action,
            "marker": self.marker,
            "cua": result.value,
        });
        Ok(result)
    }

    pub async fn stop(&mut self) -> ComputerUseResult<Value> {
        if !self.active {
            return Ok(json!({"success": true, "active": false}));
        }
        let result = self
            .driver
            .driver
            .call_tool(
                "end_session".into(),
                json!({"session": self.session_id}).to_string(),
            )
            .await
            .map_err(|error| map_driver_error("stop CUA desktop session", error))?;
        ensure_tool_ok("stop CUA desktop session", &result)?;
        self.active = false;
        self.marker.visible = false;
        self.latest_observation_id = None;
        Ok(json!({"success": true, "active": false, "marker": self.marker}))
    }
}

impl std::fmt::Debug for ComputerUseSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ComputerUseSession")
            .field("app_name", &self.app_name)
            .field("session_id", &self.session_id)
            .field("active", &self.active)
            .field("escalated", &self.escalated)
            .finish_non_exhaustive()
    }
}

impl ComputerUseSession {
    fn new(
        driver: ComputerUseDriver,
        scope: ComputerUseTargetScope,
        app_name: String,
        session_id: String,
    ) -> ComputerUseResult<Self> {
        scope.validate()?;
        if session_id.trim().is_empty() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "session_id must not be empty",
            ));
        }
        let label = format!("DCC UI Control · {} · Esc to stop", app_name.trim());
        Ok(Self {
            driver,
            scope,
            app_name,
            session_id,
            marker: ComputerUseMarker {
                visible: false,
                label,
                backend: "cua-driver-sdk",
            },
            target: None,
            observation: None,
            active: false,
            escalated: false,
        })
    }

    /// Start CUA's bounded window session and show its color-coded marker.
    pub async fn start(&mut self) -> ComputerUseResult<Value> {
        if self.active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "window session is already active",
            ));
        }
        let target = self.resolve_target().await?;
        let result = self
            .driver
            .driver
            .call_tool(
                "start_session".into(),
                json!({
                    "session": self.session_id,
                    "capture_scope": "auto",
                    "cursor_theme": {"theme_id": MOUSE_CURSOR_THEME, "reduced_motion": "auto"},
                    "_public_session_label": self.marker.label,
                })
                .to_string(),
            )
            .await
            .map_err(|error| map_driver_error("start CUA session", error))?;
        ensure_tool_ok("start CUA session", &result)?;
        enable_session_marker(&self.driver, &self.session_id, "show CUA marker").await?;
        self.target = Some(target.clone());
        self.active = true;
        self.escalated = false;
        self.marker.visible = true;
        Ok(json!({
            "success": true,
            "target": target,
            "marker": self.marker,
            "cursor_theme": MOUSE_CURSOR_THEME,
            "backend": "cua-driver-sdk",
        }))
    }

    /// Capture a fresh exact-window observation. Every action must consume it.
    pub async fn screenshot(&mut self) -> ComputerUseResult<ComputerUseScreenshot> {
        self.ensure_active()?;
        let target = self.revalidate_target().await?;
        let result = self
            .driver
            .driver
            .call_tool(
                "get_window_state".into(),
                json!({
                    "window_id": target.window_id,
                    "pid": target.pid,
                    "include_screenshot": true,
                    "max_elements": 1,
                    "max_depth": 1,
                    "session": self.session_id,
                })
                .to_string(),
            )
            .await
            .map_err(|error| map_driver_error("capture CUA window state", error));
        let result = match result {
            Ok(result) if result.is_error && is_uia_snapshot_failure(&result) => {
                return self.capture_window_from_desktop(&target).await;
            }
            Ok(result) => {
                ensure_tool_ok("capture CUA window", &result)?;
                result
            }
            Err(error) if is_uia_snapshot_message(&error.message) => {
                return self.capture_window_from_desktop(&target).await;
            }
            Err(error) => return Err(error),
        };
        let image = result.images.first().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "CUA window state returned no screenshot",
            )
        })?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(&image.data_base64)
            .map_err(|error| {
                ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
            })?;
        let (width, height) = png_dimensions(&data).ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "CUA returned a non-PNG or truncated screenshot",
            )
        })?;
        let accessibility = result
            .structured_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_else(|| json!({}));
        let observation = ComputerUseObservation {
            observation_id: format!(
                "{}-{}",
                self.session_id,
                OBSERVATION_COUNTER.fetch_add(1, Ordering::Relaxed)
            ),
            window_handle: target.window_id,
            process_id: target.pid,
            window_title: target.title.clone(),
            width,
            height,
            source_rect: target.bounds,
            capture_backend: "cua-driver-sdk".into(),
            capture_provenance: json!({
                "backend": "cua-driver-sdk",
                "pixels_captured": true,
                "scope": "window",
                "process_id": target.pid,
                "window_handle": target.window_id,
            }),
            session_id: self.session_id.clone(),
        };
        self.target = Some(target);
        self.observation = Some(observation.clone());
        Ok(ComputerUseScreenshot {
            data,
            observation,
            accessibility,
        })
    }

    /// Capture a native-resolution crop from the latest window observation.
    pub async fn zoom(
        &self,
        request: &ComputerUseZoomRequest,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        self.ensure_active()?;
        let observation = self.observation.as_ref().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "take a screenshot before zooming an observation region",
            )
        })?;
        validate_zoom_request(request, observation)?;
        let target = self.revalidate_target().await?;
        if target.window_id != observation.window_handle || target.pid != observation.process_id {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "the exact target window changed after the screenshot",
            ));
        }
        let result = self
            .call_bound_tool(
                "zoom",
                json!({
                    "pid": target.pid,
                    "window_id": target.window_id,
                    "x1": request.x1,
                    "y1": request.y1,
                    "x2": request.x2,
                    "y2": request.y2,
                }),
            )
            .await?;
        native_tool_result(result)
    }

    async fn capture_window_from_desktop(
        &mut self,
        target: &WindowTarget,
    ) -> ComputerUseResult<ComputerUseScreenshot> {
        if !self.escalated {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "UIA window snapshot timed out; call escalate_session with explicit approval before using the desktop visual fallback",
            ));
        }
        if !target.is_on_screen || target.is_minimized {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "UIA window snapshot timed out and the target is not capturable from the desktop",
            ));
        }
        let result = self
            .driver
            .driver
            .call_tool(
                "get_desktop_state".into(),
                json!({"session": self.session_id}).to_string(),
            )
            .await
            .map_err(|error| map_driver_error("capture CUA desktop fallback", error))?;
        ensure_tool_ok("capture CUA desktop fallback", &result)?;
        let image = result.images.first().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "CUA desktop fallback returned no screenshot",
            )
        })?;
        let desktop = base64::engine::general_purpose::STANDARD
            .decode(&image.data_base64)
            .map_err(|error| {
                ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
            })?;
        let data = crop_png_to_bounds(&desktop, target.bounds)?;
        let (width, height) = png_dimensions(&data).ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "desktop fallback crop returned a non-PNG screenshot",
            )
        })?;
        let desktop_state = result
            .structured_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_else(|| json!({}));
        let observation = ComputerUseObservation {
            observation_id: format!(
                "{}-{}",
                self.session_id,
                OBSERVATION_COUNTER.fetch_add(1, Ordering::Relaxed)
            ),
            window_handle: target.window_id,
            process_id: target.pid,
            window_title: target.title.clone(),
            width,
            height,
            source_rect: target.bounds,
            capture_backend: "cua-driver-sdk-desktop-crop".into(),
            capture_provenance: json!({
                "backend": "cua-driver-sdk-desktop-crop",
                "pixels_captured": true,
                "scope": "window",
                "fallback": "desktop_crop",
                "accessibility_available": false,
                "process_id": target.pid,
                "window_handle": target.window_id,
                "desktop_state": desktop_state,
            }),
            session_id: self.session_id.clone(),
        };
        self.target = Some(target.clone());
        self.observation = Some(observation.clone());
        Ok(ComputerUseScreenshot {
            data,
            observation,
            accessibility: json!({
                "degraded": true,
                "accessibility_available": false,
                "fallback": "desktop_crop",
                "window_id": target.window_id,
                "pid": target.pid,
            }),
        })
    }

    /// Read CUA's bounded semantic tree without transferring screenshot pixels.
    pub async fn accessibility_snapshot(
        &self,
        max_elements: u32,
        max_depth: u32,
    ) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        let target = self.revalidate_target().await?;
        let result = self
            .driver
            .driver
            .call_tool(
                "get_window_state".into(),
                json!({
                    "window_id": target.window_id,
                    "pid": target.pid,
                    "include_screenshot": false,
                    "max_elements": max_elements.max(1),
                    "max_depth": max_depth.max(1),
                    "session": self.session_id,
                })
                .to_string(),
            )
            .await
            .map_err(|error| map_driver_error("capture CUA accessibility state", error))?;
        ensure_tool_ok("capture CUA accessibility state", &result)?;
        result
            .structured_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .ok_or_else(|| {
                ComputerUseError::new(
                    ComputerUseErrorCode::CaptureFailed,
                    "CUA window state returned no structured accessibility state",
                )
            })
    }

    /// Verify bounded structured predicates against this exact native window.
    ///
    /// Verification is read-only and remains target-bound; the CUA driver
    /// owns predicate semantics and returns tri-state evidence.
    pub async fn verify_state(
        &self,
        expect: Value,
        timeout_ms: Option<u64>,
        stable_samples: Option<u64>,
        include_screenshot: bool,
    ) -> ComputerUseResult<ComputerUseVerification> {
        self.ensure_active()?;
        validate_verify_state_request(&expect, timeout_ms, stable_samples)?;
        let expect = expect.as_array().expect("verify state was validated");
        let target = self.revalidate_target().await?;
        let result = self
            .call_bound_tool(
                "verify_state",
                json!({
                    "pid": target.pid,
                    "window_id": target.window_id,
                    "expect": expect,
                    "timeout_ms": timeout_ms,
                    "stable_samples": stable_samples,
                    "include_screenshot": include_screenshot,
                }),
            )
            .await?;
        let value = serde_json::from_str(&result.raw_json).map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA verify_state returned invalid JSON: {error}"),
            )
        })?;
        let image = result.images.first().map(|image| {
            base64::engine::general_purpose::STANDARD
                .decode(&image.data_base64)
                .map(|data| ComputerUseImage {
                    data,
                    mime_type: image.mime_type.clone(),
                })
        });
        let image = image.transpose().map_err(|error| {
            ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
        })?;
        Ok(ComputerUseVerification { value, image })
    }

    /// Call an extension CUA tool while retaining this session's exact target.
    /// Typed action/browser/lifecycle routes remain the only path for their
    /// sensitive operations.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        validate_native_tool_request(name, &arguments)?;
        if !native_tool_allowed_in_window_session(name) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!("CUA tool {name:?} must use its dedicated typed route"),
            ));
        }
        self.ensure_active()?;
        let target = self.revalidate_target().await?;
        let schema = self.driver.tool_schema(name).await?;
        let mut object = arguments.as_object().cloned().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "native CUA tool arguments must be a JSON object",
            )
        })?;
        let properties = schema["properties"].as_object().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA tool {name:?} returned an invalid input schema"),
            )
        })?;
        for reserved in ["pid", "window_id", "session"] {
            if object.remove(reserved).is_some() && !properties.contains_key(reserved) {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::InvalidAction,
                    format!("native CUA tool argument {reserved:?} is not target-bindable"),
                ));
            }
        }
        if properties.contains_key("pid") {
            object.insert("pid".into(), json!(target.pid));
        }
        if properties.contains_key("window_id") {
            object.insert("window_id".into(), json!(target.window_id));
        }
        if properties.contains_key("session") {
            object.insert("session".into(), json!(self.session_id));
        }
        if !properties.contains_key("pid")
            && !properties.contains_key("window_id")
            && !properties.contains_key("session")
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                format!("CUA tool {name:?} is not bindable to an exact window session"),
            ));
        }
        let result = self
            .driver
            .driver
            .call_tool(name.to_owned(), Value::Object(object).to_string())
            .await
            .map_err(|error| map_driver_error(&format!("call CUA {name}"), error))?;
        ensure_tool_ok(&format!("call CUA {name}"), &result)?;
        native_tool_result(result)
    }

    /// Call one of CUA's typed browser tools within this exact native window.
    ///
    /// The allow-list is deliberate: browser adapters must not turn the Core
    /// host into an arbitrary CUA command proxy. CUA still owns browser target,
    /// tab, ref, origin, and input-trust validation.
    pub async fn call_browser_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<Value> {
        const ALLOWED_TOOLS: [&str; 8] = [
            "get_browser_state",
            "browser_prepare",
            "browser_navigate",
            "browser_click",
            "browser_type",
            "browser_pointer",
            "browser_set_input_files",
            "browser_dialog",
        ];
        if !ALLOWED_TOOLS.contains(&name) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!("browser tool {name:?} is not exposed by this host"),
            ));
        }
        self.ensure_active()?;
        let target = self.revalidate_target().await?;
        let mut object = arguments.as_object().cloned().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "browser tool arguments must be a JSON object",
            )
        })?;
        object.insert("session".into(), json!(self.session_id));
        if name == "browser_prepare"
            || (name == "get_browser_state" && !object.contains_key("target_id"))
        {
            object.insert("pid".into(), json!(target.pid));
            object.insert("window_id".into(), json!(target.window_id));
        }
        let result = self
            .driver
            .driver
            .call_tool(name.to_owned(), Value::Object(object).to_string())
            .await
            .map_err(|error| map_driver_error(&format!("call CUA {name}"), error))?;
        ensure_tool_ok(&format!("call CUA {name}"), &result)?;
        serde_json::from_str(&result.raw_json).map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA {name} returned invalid JSON: {error}"),
            )
        })
    }

    /// Call the one browser destructive tool through CUA's trusted adapter
    /// ingress. The approval evidence is created here, never accepted from
    /// caller JSON.
    pub async fn call_browser_download_tool(&self, arguments: Value) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        self.revalidate_target().await?;
        let mut object = arguments.as_object().cloned().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "browser download arguments must be a JSON object",
            )
        })?;
        object.insert("session".into(), json!(self.session_id));
        object.insert(
            "_cua_browser_download_mcp_host_approved".into(),
            Value::Bool(true),
        );
        let result = self
            .driver
            .driver
            .call_tool_from_trusted_adapter("browser_download", Value::Object(object))
            .await
            .map_err(|error| map_driver_error("call CUA browser_download", error))?;
        ensure_tool_ok("call CUA browser_download", &result)?;
        serde_json::from_str(&result.raw_json).map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA browser_download returned invalid JSON: {error}"),
            )
        })
    }

    /// Read clipboard types, optionally including privacy-sensitive text.
    pub async fn clipboard_read(&self, include_text: bool) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        self.revalidate_target().await?;
        self.call_bound_tool_value("clipboard_read", json!({"include_text": include_text}))
            .await
    }

    /// Replace the clipboard with exactly one validated value.
    pub async fn clipboard_write(
        &self,
        request: &ComputerUseClipboardWriteRequest,
    ) -> ComputerUseResult<Value> {
        validate_clipboard_write_request(request)?;
        self.ensure_active()?;
        self.revalidate_target().await?;
        self.call_bound_tool_value(
            "clipboard_write",
            serde_json::to_value(request).map_err(|error| {
                ComputerUseError::new(ComputerUseErrorCode::InvalidAction, error.to_string())
            })?,
        )
        .await
    }

    /// Start trajectory/evidence recording for this exact CUA session.
    pub async fn recording_start(
        &self,
        request: &ComputerUseRecordingStartRequest,
    ) -> ComputerUseResult<Value> {
        validate_recording_start_request(request)?;
        self.ensure_active()?;
        self.revalidate_target().await?;
        self.call_bound_tool_value(
            "start_recording",
            json!({
                "output_dir": request.output_dir,
                "record_video": request.record_video,
            }),
        )
        .await
    }

    /// Stop recording and return the finalized recording state.
    pub async fn recording_stop(&self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        self.revalidate_target().await?;
        self.call_bound_tool_value("stop_recording", json!({}))
            .await
    }

    /// Read the current recording state without exposing arbitrary CUA calls.
    pub async fn recording_state(&self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        self.revalidate_target().await?;
        self.call_bound_tool_value("get_recording_state", json!({}))
            .await
    }

    /// Execute one scoped action through CUA after a fresh target fence.
    pub async fn perform_action(
        &mut self,
        action: &ComputerUseAction,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        self.ensure_active()?;
        validate_action(action)?;
        let observation = self.observation.as_ref().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "take a screenshot before performing Computer Use actions",
            )
        })?;
        if action.observation_id.as_deref() != Some(observation.observation_id.as_str()) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "action observation_id does not match the latest screenshot",
            ));
        }
        let target = self.revalidate_target().await?;
        if target.window_id != observation.window_handle || target.pid != observation.process_id {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "the exact target window changed after the screenshot",
            ));
        }
        let args = action_arguments(action, &self.session_id, &target);
        let name = args["_tool"].as_str().unwrap_or_default().to_string();
        let mut args = args;
        args.as_object_mut()
            .expect("action arguments are an object")
            .remove("_tool");
        let result = self
            .driver
            .driver
            .call_tool(name.clone(), args.to_string())
            .await
            .map_err(|error| map_driver_error(&format!("execute CUA {name}"), error))?;
        ensure_tool_ok(&format!("execute CUA {name}"), &result)?;
        let mut result = native_tool_result(result)?;
        result.value = json!({
            "success": true,
            "action": action,
            "target": target,
            "marker": self.marker,
            "capture_provenance": observation.capture_provenance,
            "cua": result.value,
        });
        Ok(result)
    }

    async fn call_bound_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<cua_driver_sdk::ToolResult> {
        let mut object = arguments.as_object().cloned().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "bound CUA tool arguments must be a JSON object",
            )
        })?;
        object.insert("session".into(), json!(self.session_id));
        let result = self
            .driver
            .driver
            .call_tool(name.to_owned(), Value::Object(object).to_string())
            .await
            .map_err(|error| map_driver_error(&format!("call CUA {name}"), error))?;
        ensure_tool_ok(&format!("call CUA {name}"), &result)?;
        Ok(result)
    }

    async fn call_bound_tool_value(
        &self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<Value> {
        let result = self.call_bound_tool(name, arguments).await?;
        serde_json::from_str(&result.raw_json).map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA {name} returned invalid JSON: {error}"),
            )
        })
    }

    pub async fn stop(&mut self) -> ComputerUseResult<Value> {
        if !self.active {
            return Ok(json!({"success": true, "active": false}));
        }
        let result = self
            .driver
            .driver
            .call_tool(
                "end_session".into(),
                json!({"session": self.session_id}).to_string(),
            )
            .await
            .map_err(|error| map_driver_error("stop CUA session", error))?;
        ensure_tool_ok("stop CUA session", &result)?;
        self.active = false;
        self.marker.visible = false;
        self.target = None;
        self.observation = None;
        Ok(json!({"success": true, "active": false, "marker": self.marker}))
    }

    /// Read CUA's live capture policy for this exact session.
    pub async fn session_state(&self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        self.call_bound_tool_value("get_session_state", json!({}))
            .await
    }

    /// Control or inspect the visible CUA mouse marker for this session.
    pub async fn cursor_tool(&mut self, name: &str, arguments: Value) -> ComputerUseResult<Value> {
        if !cursor_tool_allowed(name) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!("cursor tool {name:?} is not exposed by this host"),
            ));
        }
        validate_native_tool_request(name, &arguments)?;
        let mut object = arguments.as_object().cloned().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "cursor tool arguments must be a JSON object",
            )
        })?;
        if object.contains_key("session") {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "cursor tool session is host-owned",
            ));
        }
        if name == "move_cursor" {
            validate_window_cursor_move(&object)?;
            object.insert("scope".into(), Value::String("window".into()));
        }
        let enabled = if name == "set_agent_cursor_enabled" {
            Some(
                object
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| {
                        ComputerUseError::new(
                            ComputerUseErrorCode::InvalidAction,
                            "set_agent_cursor_enabled requires enabled",
                        )
                    })?,
            )
        } else {
            None
        };
        self.ensure_active()?;
        self.revalidate_target().await?;
        let result = self
            .call_bound_tool_value(name, Value::Object(object))
            .await?;
        if let Some(enabled) = enabled {
            self.marker.visible = enabled;
        }
        Ok(result)
    }

    /// Explicitly unlock the desktop phase after the window ladder is exhausted.
    pub async fn escalate(
        &mut self,
        reason: &str,
        detail: Option<&str>,
    ) -> ComputerUseResult<Value> {
        validate_escalation_request(reason, detail)?;
        self.ensure_active()?;
        self.revalidate_target().await?;
        let mut arguments = json!({"reason": reason});
        if let Some(detail) = detail {
            arguments["detail"] = Value::String(detail.to_owned());
        }
        let result = self
            .call_bound_tool_value("escalate_session", arguments)
            .await?;
        self.escalated = true;
        Ok(result)
    }

    pub async fn resume_after_user_approval(&mut self) -> ComputerUseResult<Value> {
        if self.active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "stop the active session before resuming after user approval",
            ));
        }
        self.start().await
    }

    /// Return the last CUA-validated exact target without exposing mutable state.
    pub fn target(&self) -> Option<Value> {
        self.target.as_ref().map(|target| json!(target))
    }

    /// Revalidate and return the current exact-window state.
    pub async fn window_state(&self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        let target = self.revalidate_target().await?;
        Ok(json!({
            "process_id": target.pid,
            "window_handle": target.window_id,
            "exists": true,
            "visible": target.is_on_screen,
            "minimized": target.is_minimized,
            "foreground": target.is_foreground,
        }))
    }

    /// Activate only the exact target through CUA's scoped window action.
    pub async fn activate(&self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        let target = self.revalidate_target().await?;
        let result = self
            .driver
            .driver
            .call_tool(
                "bring_to_front".into(),
                json!({
                    "pid": target.pid,
                    "window_id": target.window_id,
                    "session": self.session_id,
                })
                .to_string(),
            )
            .await
            .map_err(|error| map_driver_error("activate CUA window", error))?;
        ensure_tool_ok("activate CUA window", &result)?;
        Ok(json!({"success": true, "target": target}))
    }

    /// Force-terminate only the exact process bound to this session.
    ///
    /// This is intentionally separate from `stop`: stopping ends the CUA
    /// control session, while termination is a destructive application
    /// operation that requires an explicit Host grant.
    pub async fn terminate_app(&mut self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        let target = self.revalidate_target().await?;
        let termination = self
            .call_bound_tool_value("kill_app", json!({"pid": target.pid}))
            .await?;
        let cleanup = self.stop().await?;
        Ok(json!({
            "success": true,
            "target": target,
            "termination": termination,
            "cleanup": cleanup,
        }))
    }

    pub fn status(&self) -> Value {
        json!({
            "active": self.active,
            "escalated": self.escalated,
            "session_id": self.session_id,
            "target": self.target,
            "marker": self.marker,
            "latest_observation_id": self.observation.as_ref().map(|value| &value.observation_id),
            "backend": "cua-driver-sdk",
        })
    }

    async fn resolve_target(&self) -> ComputerUseResult<WindowTarget> {
        let rows = self.list_windows().await?;
        let matches = rows
            .into_iter()
            .filter(|row| self.scope.matches(row))
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::MissingWindow,
                format!(
                    "expected exactly one scoped window, found {}",
                    matches.len()
                ),
            ));
        }
        let target = matches.into_iter().next().expect("one match");
        validate_target_policy(&target)?;
        Ok(target)
    }

    async fn revalidate_target(&self) -> ComputerUseResult<WindowTarget> {
        let target = self.resolve_target().await?;
        let original = self.target.as_ref().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::MissingWindow,
                "session target is missing",
            )
        })?;
        if target.pid != original.pid || target.window_id != original.window_id {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                "the exact target identity changed",
            ));
        }
        Ok(target)
    }

    async fn list_windows(&self) -> ComputerUseResult<Vec<WindowTarget>> {
        Ok(list_windows_with_driver(&self.driver.driver, None, false)
            .await?
            .into_iter()
            .filter_map(|value| WindowTarget::from_value(&value))
            .collect())
    }

    fn ensure_active(&self) -> ComputerUseResult<()> {
        if self.active {
            Ok(())
        } else {
            Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "Computer Use session is not active",
            ))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct WindowTarget {
    pid: u32,
    window_id: u64,
    title: String,
    app_name: String,
    bounds: [i32; 4],
    #[serde(default)]
    is_on_screen: bool,
    #[serde(default)]
    is_minimized: bool,
    #[serde(default)]
    z_index: i32,
    #[serde(default)]
    is_foreground: bool,
}

impl WindowTarget {
    fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            pid: value["pid"].as_u64()?.try_into().ok()?,
            window_id: value["window_id"].as_u64()?,
            title: value["title"].as_str().unwrap_or_default().to_owned(),
            app_name: value["app_name"].as_str().unwrap_or_default().to_owned(),
            bounds: bounds(value["bounds"].as_object()?)?,
            is_on_screen: value["is_on_screen"].as_bool().unwrap_or(false),
            is_minimized: value["minimized"].as_bool().unwrap_or(false),
            z_index: value["z_index"].as_i64().unwrap_or_default() as i32,
            is_foreground: value["is_foreground"].as_bool().unwrap_or(false),
        })
    }
}

async fn list_windows_with_driver(
    driver: &Arc<CuaDriver>,
    pid: Option<u32>,
    on_screen_only: bool,
) -> ComputerUseResult<Vec<Value>> {
    let mut arguments = json!({});
    if let Some(pid) = pid {
        arguments["pid"] = json!(pid);
    }
    if on_screen_only {
        arguments["on_screen_only"] = Value::Bool(true);
    }
    let result = driver
        .call_tool("list_windows".into(), arguments.to_string())
        .await
        .map_err(|error| map_driver_error("list CUA windows", error))?;
    ensure_tool_ok("list CUA windows", &result)?;
    let value: Value = result.raw_json.parse::<Value>().map_err(|error| {
        ComputerUseError::new(ComputerUseErrorCode::BackendUnavailable, error.to_string())
    })?;
    value["structuredContent"]["windows"]
        .as_array()
        .cloned()
        .ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "CUA list_windows omitted structuredContent.windows",
            )
        })
}

impl ComputerUseTargetScope {
    fn matches(&self, target: &WindowTarget) -> bool {
        self.process_id.is_none_or(|value| value == target.pid)
            && self
                .window_handle
                .is_none_or(|value| value == target.window_id)
            && self
                .window_title
                .as_deref()
                .is_none_or(|value| value == target.title)
    }
}

fn bounds(value: &serde_json::Map<String, Value>) -> Option<[i32; 4]> {
    Some([
        value["x"].as_i64()?.try_into().ok()?,
        value["y"].as_i64()?.try_into().ok()?,
        value["width"].as_i64()?.try_into().ok()?,
        value["height"].as_i64()?.try_into().ok()?,
    ])
}

fn validate_target_policy(target: &WindowTarget) -> ComputerUseResult<()> {
    let value = format!("{} {}", target.app_name, target.title).to_ascii_lowercase();
    const DENIED: [&str; 7] = [
        "password",
        "credential",
        "authentication",
        "sign in",
        "login",
        "terminal",
        "command prompt",
    ];
    if DENIED.iter().any(|marker| value.contains(marker)) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTarget,
            "system, terminal, authentication, and password targets are not allowed",
        ));
    }
    Ok(())
}

fn validate_launch_request(request: &ComputerUseLaunchRequest) -> ComputerUseResult<()> {
    let selectors = [
        request.name.as_deref(),
        request.bundle_id.as_deref(),
        request.aumid.as_deref(),
        request.path.as_deref(),
        request.launch_path.as_deref(),
    ];
    if selectors
        .iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .count()
        != 1
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "launch requires exactly one non-empty name, bundle_id, or launch_path",
        ));
    }
    let selected = selectors
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
        .unwrap_or_default()
        .to_ascii_lowercase();
    const DENIED: [&str; 12] = [
        "password",
        "credential",
        "authentication",
        "sign in",
        "login",
        "terminal",
        "command prompt",
        "cmd.exe",
        "powershell",
        "pwsh",
        "bash",
        "security",
    ];
    if DENIED.iter().any(|marker| selected.contains(marker)) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTarget,
            "system, terminal, authentication, password, and security applications are not allowed",
        ));
    }
    if request.urls.len() > MAX_LAUNCH_URLS
        || request.additional_arguments.len() > MAX_LAUNCH_ARGUMENTS
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "launch contains too many URLs or arguments",
        ));
    }
    Ok(())
}
