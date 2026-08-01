//! CUA-backed, Core-compatible scoped Computer Use.
//!
//! CUA owns native capture/input and its color-coded cursor. This crate keeps
//! the DCC-MCP safety shell: exact target scope, fresh observations, bounded
//! actions, stop semantics, and auditable provenance.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use cua_driver_sdk::CuaDriver;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

const MAX_TEXT_UTF16_UNITS: usize = 4_096;
const MAX_KEY_TOKENS: usize = 16;
const MAX_DRAG_POINTS: usize = 256;
const MAX_LAUNCH_ARGUMENTS: usize = 32;
const MAX_LAUNCH_URLS: usize = 16;
const MAX_LOCAL_PATH_CHARS: usize = 4_096;
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
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub button: Option<String>,
    pub scroll_x: Option<i32>,
    pub scroll_y: Option<i32>,
    #[serde(default)]
    pub path: Vec<ComputerUsePoint>,
    pub text: Option<String>,
    #[serde(default)]
    pub keys: Vec<String>,
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
}

impl ComputerUseDriver {
    pub fn create() -> ComputerUseResult<Self> {
        CuaDriver::create(None)
            .map(|driver| Self { driver })
            .map_err(|error| map_driver_error("create CUA runtime", error))
    }

    pub fn session(
        &self,
        scope: ComputerUseTargetScope,
        app_name: impl Into<String>,
        session_id: impl Into<String>,
    ) -> ComputerUseResult<ComputerUseSession> {
        ComputerUseSession::new(
            Arc::clone(&self.driver),
            scope,
            app_name.into(),
            session_id.into(),
        )
    }

    pub fn raw(&self) -> &Arc<CuaDriver> {
        &self.driver
    }

    /// List the currently visible native windows through the CUA runtime.
    pub async fn list_windows(&self) -> ComputerUseResult<Vec<Value>> {
        list_windows_with_driver(&self.driver).await
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
}

/// A long-lived, exact-window Computer Use session.
pub struct ComputerUseSession {
    driver: Arc<CuaDriver>,
    scope: ComputerUseTargetScope,
    app_name: String,
    session_id: String,
    marker: ComputerUseMarker,
    target: Option<WindowTarget>,
    observation: Option<ComputerUseObservation>,
    active: bool,
}

impl std::fmt::Debug for ComputerUseSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ComputerUseSession")
            .field("app_name", &self.app_name)
            .field("session_id", &self.session_id)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl ComputerUseSession {
    fn new(
        driver: Arc<CuaDriver>,
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
        })
    }

    /// Start CUA's bounded window session and show its color-coded marker.
    pub async fn start(&mut self) -> ComputerUseResult<Value> {
        let target = self.resolve_target().await?;
        let result = self
            .driver
            .call_tool(
                "start_session".into(),
                json!({
                    "session": self.session_id,
                    "capture_scope": "window",
                    "cursor_theme": {"theme_id": MOUSE_CURSOR_THEME, "reduced_motion": "auto"},
                    "_public_session_label": self.marker.label,
                })
                .to_string(),
            )
            .await
            .map_err(|error| map_driver_error("start CUA session", error))?;
        ensure_tool_ok("start CUA session", &result)?;
        let cursor = self
            .driver
            .call_tool(
                "set_agent_cursor_enabled".into(),
                json!({"session": self.session_id, "enabled": true}).to_string(),
            )
            .await
            .map_err(|error| map_driver_error("show CUA marker", error))?;
        ensure_tool_ok("show CUA marker", &cursor)?;
        self.target = Some(target.clone());
        self.active = true;
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
            .map_err(|error| map_driver_error("capture CUA window state", error))?;
        ensure_tool_ok("capture CUA window", &result)?;
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

    /// Execute one Core-shaped action through CUA after a fresh target fence.
    pub async fn perform_action(&mut self, action: &ComputerUseAction) -> ComputerUseResult<Value> {
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
            .call_tool(name.clone(), args.to_string())
            .await
            .map_err(|error| map_driver_error(&format!("execute CUA {name}"), error))?;
        ensure_tool_ok(&format!("execute CUA {name}"), &result)?;
        Ok(json!({
            "success": true,
            "action": action,
            "target": target,
            "marker": self.marker,
            "capture_provenance": observation.capture_provenance,
            "cua": result.raw_json,
        }))
    }

    async fn call_bound_tool_value(
        &self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<Value> {
        let mut object = arguments.as_object().cloned().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "bound CUA tool arguments must be a JSON object",
            )
        })?;
        object.insert("session".into(), json!(self.session_id));
        let result = self
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

    pub async fn stop(&mut self) -> ComputerUseResult<Value> {
        if !self.active {
            return Ok(json!({"success": true, "active": false}));
        }
        let result = self
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
        Ok(list_windows_with_driver(&self.driver)
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

async fn list_windows_with_driver(driver: &Arc<CuaDriver>) -> ComputerUseResult<Vec<Value>> {
    let result = driver
        .call_tool("list_windows".into(), "{}".into())
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

fn validate_clipboard_write_request(
    request: &ComputerUseClipboardWriteRequest,
) -> ComputerUseResult<()> {
    let values = [
        request.text.is_some(),
        request.image_path.is_some(),
        request.file_path.is_some(),
    ];
    if values.into_iter().filter(|present| *present).count() != 1 {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "clipboard_write requires exactly one of text, image_path, or file_path",
        ));
    }
    if request
        .text
        .as_deref()
        .is_some_and(|text| text.encode_utf16().count() > MAX_TEXT_UTF16_UNITS)
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "clipboard text exceeds the Core UTF-16 limit",
        ));
    }
    for path in [request.image_path.as_deref(), request.file_path.as_deref()]
        .into_iter()
        .flatten()
    {
        validate_local_file_path(path)?;
    }
    Ok(())
}

fn validate_local_file_path(path: &str) -> ComputerUseResult<()> {
    let candidate = std::path::Path::new(path);
    if path.is_empty()
        || path.chars().count() > MAX_LOCAL_PATH_CHARS
        || path.contains('\0')
        || !candidate.is_absolute()
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "clipboard file paths must be absolute and bounded",
        ));
    }
    let metadata = std::fs::symlink_metadata(candidate).map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            format!("clipboard file path is unavailable: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "clipboard file paths must name regular files directly",
        ));
    }
    Ok(())
}

fn validate_recording_start_request(
    request: &ComputerUseRecordingStartRequest,
) -> ComputerUseResult<()> {
    let path = request.output_dir.trim();
    if path.is_empty()
        || path.chars().count() > MAX_LOCAL_PATH_CHARS
        || path.contains('\0')
        || (!std::path::Path::new(path).is_absolute() && !path.starts_with("~/"))
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "recording output_dir must be an absolute path or ~/ path",
        ));
    }
    Ok(())
}

fn validate_action(action: &ComputerUseAction) -> ComputerUseResult<()> {
    const ACTIONS: [&str; 12] = [
        "click",
        "double_click",
        "right_click",
        "toggle",
        "move",
        "scroll",
        "drag",
        "type",
        "set_text",
        "set_value",
        "keypress",
        "keyboard_shortcut",
    ];
    if !ACTIONS.contains(&action.action.as_str()) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "unsupported Computer Use action",
        ));
    }
    if action.keys.len() > MAX_KEY_TOKENS || action.path.len() > MAX_DRAG_POINTS {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "action exceeds the Core safety limits",
        ));
    }
    if action
        .text
        .as_deref()
        .is_some_and(|text| text.encode_utf16().count() > MAX_TEXT_UTF16_UNITS)
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "text exceeds the Core UTF-16 limit",
        ));
    }
    for point in action.path.iter().chain(
        [action
            .x
            .zip(action.y)
            .map(|(x, y)| ComputerUsePoint { x, y })]
        .iter()
        .flatten(),
    ) {
        if !point.x.is_finite() || !point.y.is_finite() || point.x < 0.0 || point.y < 0.0 {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "coordinates must be finite and non-negative",
            ));
        }
    }
    if matches!(action.action.as_str(), "set_text" | "set_value") && action.element_index.is_none()
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "set_text and set_value require a semantic element_index",
        ));
    }
    Ok(())
}

fn action_arguments(action: &ComputerUseAction, session: &str, target: &WindowTarget) -> Value {
    let scope = json!({
        "_tool": match action.action.as_str() {
            "click" | "double_click" | "right_click" | "toggle" => "click",
            "drag" => "drag",
            "scroll" => "scroll",
            "type" => "type_text",
            "set_text" | "set_value" => "set_value",
            "keypress" => "press_key",
            "keyboard_shortcut" => "hotkey",
            "move" => "move_cursor",
            _ => "wait",
        },
        "pid": target.pid,
        "window_id": target.window_id,
        "session": session,
        "delivery_mode": "foreground",
    });
    let mut args = scope;
    let object = args
        .as_object_mut()
        .expect("action arguments are an object");
    match action.action.as_str() {
        "move" => {
            object.insert("x".into(), json!(action.x));
            object.insert("y".into(), json!(action.y));
        }
        "click" | "double_click" | "right_click" | "toggle" => {
            object.insert("x".into(), json!(action.x));
            object.insert("y".into(), json!(action.y));
            object.insert(
                "count".into(),
                json!(if action.action == "double_click" {
                    2
                } else {
                    1
                }),
            );
            object.insert(
                "button".into(),
                json!(
                    action
                        .button
                        .as_deref()
                        .unwrap_or(if action.action == "right_click" {
                            "right"
                        } else {
                            "left"
                        })
                ),
            );
        }
        "drag" => {
            let first = action.path.first().copied().unwrap_or(ComputerUsePoint {
                x: action.x.unwrap_or_default(),
                y: action.y.unwrap_or_default(),
            });
            let last = action.path.last().copied().unwrap_or(first);
            object.insert("from_x".into(), json!(first.x));
            object.insert("from_y".into(), json!(first.y));
            object.insert("to_x".into(), json!(last.x));
            object.insert("to_y".into(), json!(last.y));
        }
        "scroll" => {
            object.insert("x".into(), json!(action.x));
            object.insert("y".into(), json!(action.y));
            object.insert(
                "direction".into(),
                json!(if action.scroll_y.unwrap_or_default() < 0 {
                    "up"
                } else {
                    "down"
                }),
            );
            object.insert("by".into(), json!("amount"));
            object.insert(
                "amount".into(),
                json!(action.scroll_y.unwrap_or(1).unsigned_abs()),
            );
        }
        "type" => {
            object.insert(
                "text".into(),
                json!(action.text.as_deref().unwrap_or_default()),
            );
        }
        "set_text" | "set_value" => {
            object.insert(
                "value".into(),
                json!(action.text.as_deref().unwrap_or_default()),
            );
        }
        "keypress" => {
            object.insert(
                "key".into(),
                json!(action.keys.first().cloned().unwrap_or_default()),
            );
        }
        "keyboard_shortcut" => {
            object.insert("keys".into(), json!(action.keys));
        }
        _ => {}
    }
    if let Some(element_index) = action.element_index {
        object.insert("element_index".into(), json!(element_index));
        object.remove("x");
        object.remove("y");
    }
    args
}

fn ensure_tool_ok(context: &str, result: &cua_driver_sdk::ToolResult) -> ComputerUseResult<()> {
    if result.is_error {
        let code = result.error_code.as_deref().unwrap_or_default();
        let message = if result.text.is_empty() {
            result.raw_json.clone()
        } else {
            result.text.clone()
        };
        let mapped = if code.contains("interrupt")
            || message.to_ascii_lowercase().contains("user_interrupted")
        {
            ComputerUseErrorCode::UserInterrupted
        } else if code.contains("browser") || message.to_ascii_lowercase().contains("browser_") {
            ComputerUseErrorCode::BrowserRefused
        } else if code.contains("clipboard") || message.to_ascii_lowercase().contains("clipboard") {
            ComputerUseErrorCode::ClipboardRefused
        } else if code.contains("record") || message.to_ascii_lowercase().contains("recording") {
            ComputerUseErrorCode::RecordingRefused
        } else if code.contains("window") || code.contains("target") {
            ComputerUseErrorCode::InvalidTarget
        } else {
            ComputerUseErrorCode::InputFailed
        };
        return Err(ComputerUseError::new(
            mapped,
            format!("{context}: {message}"),
        ));
    }
    Ok(())
}

fn map_driver_error(context: &str, error: impl std::fmt::Display) -> ComputerUseError {
    let message = error.to_string();
    let lower = message.to_ascii_lowercase();
    let code = if lower.contains("interrupt") || lower.contains("user_interrupted") {
        ComputerUseErrorCode::UserInterrupted
    } else if lower.contains("browser_") {
        ComputerUseErrorCode::BrowserRefused
    } else if lower.contains("clipboard") {
        ComputerUseErrorCode::ClipboardRefused
    } else if lower.contains("recording") || lower.contains("record_") {
        ComputerUseErrorCode::RecordingRefused
    } else if lower.contains("window") || lower.contains("target") {
        ComputerUseErrorCode::InvalidTarget
    } else {
        ComputerUseErrorCode::BackendUnavailable
    };
    ComputerUseError::new(code, format!("{context}: {message}"))
}

fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 || !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    Some((
        u32::from_be_bytes(data[16..20].try_into().ok()?),
        u32::from_be_bytes(data[20..24].try_into().ok()?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_requires_exact_identity_and_action_rejects_unbounded_text() {
        assert!(ComputerUseTargetScope::default().validate().is_err());
        let action = ComputerUseAction {
            action: "type".into(),
            text: Some("x".repeat(MAX_TEXT_UTF16_UNITS + 1)),
            ..Default::default()
        };
        assert_eq!(
            validate_action(&action).unwrap_err().code,
            ComputerUseErrorCode::InvalidAction
        );
    }

    #[test]
    fn png_dimensions_reads_the_png_header() {
        let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
        data.extend_from_slice(&[0; 8]);
        data.extend_from_slice(&1280_u32.to_be_bytes());
        data.extend_from_slice(&720_u32.to_be_bytes());
        assert_eq!(png_dimensions(&data), Some((1280, 720)));
    }

    #[test]
    fn semantic_element_actions_replace_pixel_coordinates() {
        let action = ComputerUseAction {
            action: "click".into(),
            element_index: Some(7),
            x: Some(12.0),
            y: Some(13.0),
            ..Default::default()
        };
        let args = action_arguments(
            &action,
            "session",
            &WindowTarget {
                pid: 42,
                window_id: 7,
                title: "DCC".into(),
                app_name: "dcc".into(),
                bounds: [0, 0, 100, 100],
                is_on_screen: true,
                is_minimized: false,
                z_index: 1,
                is_foreground: true,
            },
        );
        assert_eq!(args["element_index"], 7);
        assert!(args.get("x").is_none());
        assert!(args.get("y").is_none());
    }

    #[test]
    fn semantic_value_actions_require_and_encode_element_values() {
        let action = ComputerUseAction {
            action: "set_text".into(),
            element_index: Some(11),
            text: Some("Hero".into()),
            ..Default::default()
        };
        let args = action_arguments(
            &action,
            "session",
            &WindowTarget {
                pid: 42,
                window_id: 7,
                title: "DCC".into(),
                app_name: "dcc".into(),
                bounds: [0, 0, 100, 100],
                is_on_screen: true,
                is_minimized: false,
                z_index: 1,
                is_foreground: true,
            },
        );
        assert_eq!(args["_tool"], "set_value");
        assert_eq!(args["element_index"], 11);
        assert_eq!(args["value"], "Hero");
        assert_eq!(
            validate_action(&ComputerUseAction {
                action: "set_value".into(),
                text: Some("Hero".into()),
                ..Default::default()
            })
            .unwrap_err()
            .code,
            ComputerUseErrorCode::InvalidAction
        );
    }

    #[test]
    fn launch_requires_one_safe_application_selector() {
        assert!(validate_launch_request(&ComputerUseLaunchRequest::default()).is_err());
        assert!(
            validate_launch_request(&ComputerUseLaunchRequest {
                name: Some("Calculator".into()),
                ..Default::default()
            })
            .is_ok()
        );
        assert!(
            validate_launch_request(&ComputerUseLaunchRequest {
                name: Some("Calculator".into()),
                bundle_id: Some("com.example.Calculator".into()),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            validate_launch_request(&ComputerUseLaunchRequest {
                launch_path: Some("powershell.exe".into()),
                ..Default::default()
            })
            .is_err()
        );
        let json = serde_json::to_value(ComputerUseLaunchRequest {
            name: Some("Calculator".into()),
            ..Default::default()
        })
        .expect("launch request should serialize");
        assert!(json.get("bundle_id").is_none());
    }

    #[test]
    fn clipboard_and_recording_requests_are_bounded() {
        assert!(
            validate_clipboard_write_request(&ComputerUseClipboardWriteRequest::default()).is_err()
        );
        assert!(
            validate_clipboard_write_request(&ComputerUseClipboardWriteRequest {
                text: Some("hello".into()),
                image_path: Some("C:\\image.png".into()),
                file_path: None,
            })
            .is_err()
        );
        assert!(
            validate_recording_start_request(&ComputerUseRecordingStartRequest {
                output_dir: "relative/output".into(),
                record_video: false,
            })
            .is_err()
        );
        assert!(
            validate_recording_start_request(&ComputerUseRecordingStartRequest {
                output_dir: "~/cua-recordings".into(),
                record_video: false,
            })
            .is_ok()
        );
    }
}
