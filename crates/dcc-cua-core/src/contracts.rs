// Public agent-facing contracts and the shared limits enforced by runtime policy.
use std::sync::atomic::AtomicU64;

pub use cua_driver_sdk::{
    ConfiguredDriverOptions, DriverAuthorizationAction, DriverAuthorizationDecision,
    DriverAuthorizationHost, DriverAuthorizationHostError, DriverAuthorizationRequest,
    EmbeddedEnvironmentVariable, PrivateWorkerOptions, RuntimeAuthorizationOptions,
    SessionPermissionMode,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

pub(crate) const MAX_TEXT_UTF16_UNITS: usize = 4_096;
pub(crate) const MAX_TYPE_CHAR_DELAY_MS: u64 = 200;
pub(crate) const MAX_KEY_TOKENS: usize = 16;
pub(crate) const MAX_MODIFIER_TOKENS: usize = 8;
pub(crate) const MAX_MODIFIER_CHARS: usize = 32;
pub(crate) const MAX_DRAG_POINTS: usize = 256;
pub(crate) const MAX_DRAG_DURATION_MS: u64 = 10_000;
pub(crate) const MAX_DRAG_STEPS: u32 = 200;
pub(crate) const MAX_SCROLL_AMOUNT: u32 = 50;
pub(crate) const MAX_ELEMENT_TOKEN_CHARS: usize = 512;
pub(crate) const MAX_LAUNCH_ARGUMENTS: usize = 32;
pub(crate) const MAX_LAUNCH_URLS: usize = 16;
pub(crate) const MAX_LOCAL_PATH_CHARS: usize = 4_096;
pub(crate) const MAX_WINDOW_QUERY_CHARS: usize = 512;
pub(crate) const MAX_WINDOW_WAIT_TIMEOUT_MS: u64 = 30_000;
pub(crate) const MAX_WINDOW_WAIT_INTERVAL_MS: u64 = 1_000;
pub(crate) const MAX_MENU_PATH_SEGMENTS: usize = 16;
pub(crate) const MAX_MENU_PATH_SEGMENT_CHARS: usize = 200;
pub(crate) const MAX_NATIVE_TOOL_NAME_CHARS: usize = 128;
pub(crate) const MAX_NATIVE_TOOL_ARGUMENT_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_NATIVE_TOOL_IMAGES: usize = 8;
pub(crate) const MAX_NATIVE_TOOL_IMAGE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_NATIVE_TOOL_TOTAL_IMAGE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MOUSE_CURSOR_THEME: &str = "com.dcc-mcp.cursor";
pub(crate) const DEFAULT_SNAPSHOT_MAX_ELEMENTS: u32 = 512;
pub(crate) const DEFAULT_SNAPSHOT_MAX_DEPTH: u32 = 16;
pub(crate) const MAX_SNAPSHOT_ELEMENTS: u32 = 2_000;
pub(crate) const MAX_SNAPSHOT_DEPTH: u32 = 25;
pub(crate) static OBSERVATION_COUNTER: AtomicU64 = AtomicU64::new(1);
pub const DEFAULT_LIVE_OBSERVATION_FPS: u32 = 10;
pub const MAX_LIVE_OBSERVATION_FPS: u32 = 30;
pub const DEFAULT_LIVE_OBSERVATION_MAX_DIMENSION: u32 = 1_568;
pub const MIN_LIVE_OBSERVATION_MAX_DIMENSION: u32 = 256;
pub const MAX_LIVE_OBSERVATION_MAX_DIMENSION: u32 = 4_096;

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
        if self.app.as_deref().is_some_and(|value| value.is_empty())
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
    pub(crate) fn limits(&self) -> ComputerUseResult<(u64, u64)> {
        self.query.validate()?;
        Ok((
            self.timeout_ms
                .unwrap_or(5_000)
                .min(MAX_WINDOW_WAIT_TIMEOUT_MS),
            self.interval_ms
                .unwrap_or(100)
                .clamp(10, MAX_WINDOW_WAIT_INTERVAL_MS),
        ))
    }
}

/// Exact top-level window frame in CUA desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ComputerUseWindowFrameRequest {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ComputerUseWindowFrameRequest {
    pub fn validate(&self) -> ComputerUseResult<()> {
        if ![self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f64::is_finite)
            || self.width < 1.0
            || self.height < 1.0
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "window frame coordinates must be finite and dimensions must be at least 1",
            ));
        }
        Ok(())
    }
}

/// A live native application-menu path resolved one level at a time by CUA.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseMenuRequest {
    pub path: Vec<String>,
}

impl ComputerUseMenuRequest {
    pub fn validate(&self) -> ComputerUseResult<()> {
        if self.path.is_empty()
            || self.path.len() > MAX_MENU_PATH_SEGMENTS
            || self.path.iter().any(|segment| {
                segment.trim().is_empty() || segment.chars().count() > MAX_MENU_PATH_SEGMENT_CHARS
            })
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!(
                    "menu path requires 1..{MAX_MENU_PATH_SEGMENTS} non-empty segments of at most {MAX_MENU_PATH_SEGMENT_CHARS} characters"
                ),
            ));
        }
        Ok(())
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
    pub scroll_by: Option<String>,
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

/// A bounded exact-window latest-frame feed for latency-sensitive agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseLiveObservationStartRequest {
    #[serde(default = "default_live_observation_fps")]
    pub fps: u32,
    #[serde(default = "default_live_observation_max_dimension")]
    pub max_dimension: u32,
}

impl Default for ComputerUseLiveObservationStartRequest {
    fn default() -> Self {
        Self {
            fps: DEFAULT_LIVE_OBSERVATION_FPS,
            max_dimension: DEFAULT_LIVE_OBSERVATION_MAX_DIMENSION,
        }
    }
}

impl ComputerUseLiveObservationStartRequest {
    pub fn validate(&self) -> ComputerUseResult<()> {
        if !(1..=MAX_LIVE_OBSERVATION_FPS).contains(&self.fps) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!("live observation fps must be 1..={MAX_LIVE_OBSERVATION_FPS}"),
            ));
        }
        if !(MIN_LIVE_OBSERVATION_MAX_DIMENSION..=MAX_LIVE_OBSERVATION_MAX_DIMENSION)
            .contains(&self.max_dimension)
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!(
                    "live observation max_dimension must be {MIN_LIVE_OBSERVATION_MAX_DIMENSION}..={MAX_LIVE_OBSERVATION_MAX_DIMENSION}"
                ),
            ));
        }
        Ok(())
    }
}

const fn default_live_observation_fps() -> u32 {
    DEFAULT_LIVE_OBSERVATION_FPS
}

const fn default_live_observation_max_dimension() -> u32 {
    DEFAULT_LIVE_OBSERVATION_MAX_DIMENSION
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
    InteractiveDesktopUnavailable,
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
