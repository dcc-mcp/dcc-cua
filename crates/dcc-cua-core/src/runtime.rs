use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use base64::Engine;
use cua_driver_sdk::CuaDriver;
use dcc_cua_indicator::{
    BannerActivity, BannerActivityGuard, BannerFailureKind, BannerTarget, ControlBanner,
    IndicatorError, localized_control_label,
};
use serde_json::{Value, json};

use crate::contracts::*;
use crate::interactive_desktop;
use crate::live_observation::{LiveObservation, LiveObservationFence, observation_sequence_fence};
use crate::observation::semantic_observation;
use crate::policy::*;
use crate::showcase::{ShowcaseRecorder, fit_dimensions_with_bounds, resize_bgra};
use crate::window_target::{WindowTarget, validate_target_policy};
#[cfg(windows)]
use crate::windows_uia_fallback::WindowsUiaFallback;

mod action_result;
#[cfg(any(windows, test))]
mod drag_sequences;
#[cfg(any(windows, test))]
pub(crate) use drag_sequences::*;
pub(crate) mod application;
mod menu_commands;
mod recording;
pub(crate) use recording::{
    RecordingHealth, RecordingKeepalive, RecordingVideoTerminalEvidence, aggregate_recording_state,
    call_recording_tool_without_refresh, probe_recording_state,
};
mod session;
#[allow(unused_imports)]
pub(crate) use session::{
    ensure_target_available_for_action, gated_cursor_operation, gated_desktop_observation,
    gated_exact_window_observation, gated_exact_window_publication,
    preflight_live_observation_start, resolved_application_name,
};
#[cfg(test)]
pub(crate) use session::{
    run_gated_preinvalidated_window_mutation, run_preinvalidated_window_mutation,
};
#[cfg(any(windows, test))]
mod windows_input;
#[cfg(any(windows, test))]
pub(crate) use windows_input::*;
mod window_commands;

#[cfg(any(not(windows), test))]
pub(crate) fn activation_completion_unknown(error: ComputerUseError) -> ComputerUseError {
    let detail = error
        .message
        .replace("; the window session was invalidated", "");
    ComputerUseError::new(
        ComputerUseErrorCode::CompletionUnknown,
        format!(
            "{detail}; completion_unknown=true; automatic_input=false; blind_retry=false; fresh_observation_required=true"
        ),
    )
}

#[cfg(windows)]
fn windows_platform_input_gate(
    stage: &'static str,
) -> Result<(), dcc_cua_platform_windows::UiaError> {
    interactive_desktop::require_input_available().map_err(|error| {
        dcc_cua_platform_windows::UiaError::PermissionDenied(format!(
            "input_gate_stage={stage}: {error}"
        ))
    })
}

#[cfg(windows)]
pub(crate) fn map_windows_window_mutation_error(
    context: &str,
    error: dcc_cua_platform_windows::UiaError,
) -> ComputerUseError {
    let code = match &error {
        dcc_cua_platform_windows::UiaError::PermissionDenied(message)
            if message.contains("input_gate_stage=") =>
        {
            ComputerUseErrorCode::InteractiveDesktopUnavailable
        }
        dcc_cua_platform_windows::UiaError::InvalidTarget(message)
            if message.contains("target_minimized:") =>
        {
            ComputerUseErrorCode::TargetMinimized
        }
        dcc_cua_platform_windows::UiaError::InvalidTarget(_) => {
            ComputerUseErrorCode::TargetUnavailable
        }
        _ => ComputerUseErrorCode::InputFailed,
    };
    ComputerUseError::new(code, format!("{context}: {error}"))
}

const INPUT_CALL_TIMEOUT: Duration = Duration::from_secs(15);
const CURSOR_GLIDE_MS: u64 = 180;
const SESSION_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const RECORDING_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(any(windows, test))]
const RAW_DRAG_PRE_DOWN_SETTLE: Duration = Duration::from_millis(50);
#[cfg(any(windows, test))]
const RAW_DRAG_DROP_SETTLE: Duration = Duration::from_millis(75);
#[cfg(any(windows, test))]
const RAW_DRAG_POST_UP_SETTLE: Duration = Duration::from_millis(50);
#[cfg(windows)]
pub(crate) const RELATIVE_DRAG_MAX_ATTEMPTS_PER_WAYPOINT: usize = 6;
#[cfg(windows)]
const RELATIVE_DRAG_ENDPOINT_TOLERANCE_PX: i32 = 0;
#[cfg(windows)]
const RELATIVE_DRAG_QUANTIZED_STALL_TOLERANCE_PX: i32 = 1;
#[cfg(windows)]
const RELATIVE_DRAG_INTERMEDIATE_TOLERANCE_PX: i32 = 1;
#[cfg(windows)]
const RELATIVE_DRAG_DAMPING_MIN_EFFECTIVE_COMMAND_PX: i32 = 2;
#[cfg(windows)]
const RELATIVE_DRAG_STAGNATION_ESCAPE_MAX_RESIDUAL_PX: i32 = 3;
#[cfg(windows)]
const RELATIVE_DRAG_STAGNATION_ESCAPE_MAX_COMMAND_PX: i32 = 4;

pub(crate) fn explicit_input_backend_rejection(action: &ComputerUseAction) -> Option<String> {
    action.input_backend_id.as_ref()?;
    #[cfg(windows)]
    {
        select_windows_foreground_drag_backend(action).err()
    }
    #[cfg(not(windows))]
    {
        Some(format!(
            "input backend {:?} is not supported on this platform",
            action.input_backend_id.as_deref().unwrap_or_default()
        ))
    }
}

fn input_target_fence(target: &WindowTarget, foreground_verified: bool) -> Value {
    json!({
        "process_id": target.pid,
        "window_handle": target.window_id,
        "exact_window": true,
        "foreground_required": true,
        "foreground_verified": foreground_verified,
    })
}

pub(crate) fn input_backend_rejection_result(
    backend_id: &str,
    reason: &str,
    target: &WindowTarget,
) -> ComputerUseToolResult {
    ComputerUseToolResult {
        value: json!({
            "success": false,
            "route": "input_backend_selection",
            "delivery": {
                "mode": "foreground",
                "backend_id": backend_id,
                "api_accepted": false,
                "consumer_effect_confirmed": false,
                "completion_known": false,
                "verification_required": true,
                "retry_safe": false,
                "fallback_attempted": false,
                "rejection_reason": reason,
                "target_fence": input_target_fence(target, target.is_foreground),
            },
            "effect": "not_attempted",
        }),
        text: format!("Rejected input backend {backend_id:?} without attempting input: {reason}"),
        images: Vec::new(),
        degraded: true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionBannerPhase {
    Preparing,
    #[cfg(any(windows, test))]
    Injecting,
}

pub(crate) fn banner_activity_for_action_phase(
    _action: &ComputerUseAction,
    phase: ActionBannerPhase,
) -> BannerActivity {
    match phase {
        ActionBannerPhase::Preparing => BannerActivity::Operating,
        #[cfg(any(windows, test))]
        ActionBannerPhase::Injecting => match _action.action.as_str() {
            "type" | "type_chars" | "set_text" | "set_value" | "keypress" | "keyboard_shortcut" => {
                BannerActivity::KeyboardInput
            }
            _ => BannerActivity::PointerInput,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveObservationStartDisposition {
    ReuseExisting,
    StartedNew,
}

pub(crate) fn live_observation_start_disposition(
    state: Option<&Value>,
) -> LiveObservationStartDisposition {
    if state.is_some_and(|state| state["active"] == true && state["terminal_reason"].is_null()) {
        LiveObservationStartDisposition::ReuseExisting
    } else {
        LiveObservationStartDisposition::StartedNew
    }
}

pub(crate) fn banner_activity_for_bound_tool(name: &str) -> BannerActivity {
    match name {
        "browser_navigate" | "browser_prepare" => BannerActivity::Navigating,
        "get_browser_state" => BannerActivity::Observing,
        _ => BannerActivity::Operating,
    }
}

pub(crate) fn attach_banner_status(mut value: Value, banner: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("banner".into(), banner);
        value
    } else {
        json!({"cua": value, "banner": banner})
    }
}

pub(crate) fn attach_indicator_motion_to_activation(
    mut activation: Value,
    banner: &Value,
) -> Value {
    let Some(motion) = banner.get("motion") else {
        return activation;
    };
    if let Some(object) = activation.as_object_mut() {
        object.insert("indicator_motion".into(), motion.clone());
        activation
    } else {
        json!({"cua": activation, "indicator_motion": motion})
    }
}

pub(crate) fn map_indicator_error(context: &str, error: IndicatorError) -> ComputerUseError {
    let code = match error {
        IndicatorError::InvalidTarget(_) => ComputerUseErrorCode::InvalidTarget,
        IndicatorError::Backend(_) => ComputerUseErrorCode::BackendUnavailable,
    };
    ComputerUseError::new(code, format!("{context}: {error}"))
}

pub(crate) fn held_coordinate_click_as_drag(
    action: &ComputerUseAction,
) -> Option<ComputerUseAction> {
    let (Some(duration_ms), Some(x), Some(y)) = (action.duration_ms, action.x, action.y) else {
        return None;
    };
    if action.action != "click" || duration_ms == 0 {
        return None;
    }

    let mut drag = action.clone();
    drag.action = "drag".into();
    drag.x = None;
    drag.y = None;
    drag.path = vec![ComputerUsePoint { x, y }; 2];
    drag.steps = Some(20);
    Some(drag)
}

async fn call_driver_tool(
    driver: &CuaDriver,
    name: impl Into<String>,
    arguments: String,
    operation: &str,
) -> ComputerUseResult<cua_driver_sdk::ToolResult> {
    call_driver_tool_with_timeout(driver, name, arguments, operation, INPUT_CALL_TIMEOUT).await
}

async fn call_driver_tool_with_timeout(
    driver: &CuaDriver,
    name: impl Into<String>,
    arguments: String,
    operation: &str,
    timeout: Duration,
) -> ComputerUseResult<cua_driver_sdk::ToolResult> {
    let name = name.into();
    let result = await_input_call(driver.call_tool(name, arguments), timeout, operation).await?;
    result.map_err(|error| map_driver_error(operation, error))
}

pub(crate) async fn await_input_call<T>(
    future: impl Future<Output = T>,
    timeout: Duration,
    operation: &str,
) -> ComputerUseResult<T> {
    tokio::time::timeout(timeout, future).await.map_err(|_| {
        ComputerUseError::new(
            ComputerUseErrorCode::InputFailed,
            format!("CUA {operation} timed out after {} ms", timeout.as_millis()),
        )
    })
}

pub(crate) fn action_dispatch_completion_unknown(error: ComputerUseError) -> ComputerUseError {
    ComputerUseError::new(
        ComputerUseErrorCode::CompletionUnknown,
        format!(
            "{}; phase=action_dispatch; action_attempted=true; input_sent=unknown; completion_unknown=true; local_session_invalidated=true; automatic_input=false; blind_retry=false; fresh_observation_required=true",
            error.message
        ),
    )
}

/// One shared CUA runtime. Create it once per host process.
#[derive(Clone)]
pub struct ComputerUseDriver {
    driver: Arc<CuaDriver>,
    upstream_cursor_renderer_enabled: bool,
    // ponytail: cache the static tool registry for this driver; recreate the
    // driver if a future SDK supports runtime tool registration.
    tool_inventory: Arc<tokio::sync::OnceCell<Value>>,
}

impl ComputerUseDriver {
    pub fn create() -> ComputerUseResult<Self> {
        crate::driver_factory::ensure_bundled_cursor_theme()?;
        crate::driver_factory::create_embedded()
            .map(Self::from_driver)
            .map_err(|error| map_driver_error("create CUA runtime", error))
    }

    /// Create a directly supervised official CUA worker.
    ///
    /// The packaged macOS Host uses this upstream-owned process boundary so
    /// CUA can keep AppKit on the worker's main thread and render its native
    /// per-session cursor without turning the Host itself into an app bundle.
    pub fn create_private_worker(options: PrivateWorkerOptions) -> ComputerUseResult<Self> {
        crate::driver_factory::ensure_bundled_cursor_theme()?;
        crate::driver_factory::create_private_worker(options)
            .map(Self::from_driver)
            .map_err(|error| map_driver_error("create CUA private worker", error))
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
        crate::driver_factory::ensure_bundled_cursor_theme()?;
        crate::driver_factory::create_authorized(options, host)
            .map(Self::from_driver)
            .map_err(|error| map_driver_error("create authorized CUA runtime", error))
    }

    fn from_driver((driver, upstream_cursor_renderer_enabled): (Arc<CuaDriver>, bool)) -> Self {
        Self {
            driver,
            upstream_cursor_renderer_enabled,
            tool_inventory: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    #[must_use]
    pub const fn upstream_cursor_renderer_enabled(&self) -> bool {
        self.upstream_cursor_renderer_enabled
    }

    pub fn session(
        &self,
        scope: ComputerUseTargetScope,
        app_name: impl Into<String>,
        session_id: impl Into<String>,
    ) -> ComputerUseResult<ComputerUseSession> {
        self.session_with_agent(scope, app_name, "Agent", session_id)
    }

    pub fn session_with_agent(
        &self,
        scope: ComputerUseTargetScope,
        app_name: impl Into<String>,
        agent_name: impl Into<String>,
        session_id: impl Into<String>,
    ) -> ComputerUseResult<ComputerUseSession> {
        ComputerUseSession::new(
            self.clone(),
            scope,
            app_name.into(),
            agent_name.into(),
            session_id.into(),
        )
    }

    pub fn desktop_session(
        &self,
        session_id: impl Into<String>,
    ) -> ComputerUseResult<ComputerUseDesktopSession> {
        self.desktop_session_with_agent("Agent", session_id)
    }

    pub fn desktop_session_with_agent(
        &self,
        agent_name: impl Into<String>,
        session_id: impl Into<String>,
    ) -> ComputerUseResult<ComputerUseDesktopSession> {
        ComputerUseDesktopSession::new(self.clone(), agent_name.into(), session_id.into())
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

    #[cfg(not(windows))]
    pub(crate) async fn capture_exact_window_png(
        &self,
        process_id: u32,
        window_handle: u64,
        session_id: &str,
    ) -> ComputerUseResult<Vec<u8>> {
        let matches_target = |windows: &[Value]| {
            windows.iter().any(|window| {
                WindowTarget::from_value(window).is_some_and(|target| {
                    target.pid == process_id && target.window_id == window_handle
                })
            })
        };
        let before = self.list_windows_filtered(Some(process_id), false).await?;
        if !matches_target(&before) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                "live observation target identity changed",
            ));
        }
        let result = call_driver_tool(
            &self.driver,
            "get_window_state",
            json!({
                "window_id": window_handle,
                "pid": process_id,
                "include_screenshot": true,
                "max_elements": 1,
                "max_depth": 1,
                "session": session_id,
            })
            .to_string(),
            "capture portable live window frame",
        )
        .await?;
        ensure_tool_ok("capture portable live window frame", &result)?;
        let after = self.list_windows_filtered(Some(process_id), false).await?;
        if !matches_target(&after) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                "live observation target identity changed during capture",
            ));
        }
        let image = result.images.first().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "CUA live window state returned no screenshot",
            )
        })?;
        base64::engine::general_purpose::STANDARD
            .decode(&image.data_base64)
            .map_err(|error| {
                ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
            })
    }

    /// Wait until the bounded native window query returns at least one row.
    pub async fn wait_for_window(
        &self,
        request: &ComputerUseWindowWaitRequest,
    ) -> ComputerUseResult<Value> {
        let (timeout_ms, interval_ms) = request.limits()?;
        let started = Instant::now();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let mut windows = match wait_for_window_probe_until(
                deadline,
                self.list_windows_filtered(request.query.process_id, request.query.on_screen_only),
            )
            .await
            {
                WindowWaitProbeOutcome::TimedOut => return Err(window_wait_timeout()),
                WindowWaitProbeOutcome::Completed(result) => result?,
            };
            windows.retain(|window| request.query.matches_window(window));
            if !windows.is_empty() {
                return Ok(json!({
                    "windows": windows,
                    "count": windows.len(),
                    "waited_ms": started.elapsed().as_millis(),
                }));
            }
            if matches!(
                wait_for_window_probe_until(
                    deadline,
                    tokio::time::sleep(Duration::from_millis(interval_ms)),
                )
                .await,
                WindowWaitProbeOutcome::TimedOut
            ) {
                return Err(window_wait_timeout());
            }
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

    /// Probe the embedded CUA runtime without creating a second process.
    ///
    /// Diagnostics are always returned as data so supervisors can distinguish
    /// a live-but-unready Host from a transport failure.
    pub async fn diagnostics(&self) -> Value {
        let (metadata, windows, permissions, health) = tokio::join!(
            self.driver.metadata(),
            self.list_windows(),
            self.call_global_tool("check_permissions", json!({})),
            self.call_global_tool("health_report", json!({})),
        );
        let interactive_desktop = interactive_desktop::diagnostic();
        let driver = match metadata {
            Ok(result) => json!({"success": true, "result": result}),
            Err(error) => json!({"success": false, "message": error.to_string()}),
        };
        let window_inventory = match windows {
            Ok(result) => json!({"success": true, "count": result.len()}),
            Err(error) => json!({
                "success": false,
                "code": error.code,
                "message": error.message,
            }),
        };
        let permissions = diagnostic_tool_check(permissions);
        let health = diagnostic_tool_check(health);
        let ready = driver["success"] == true
            && window_inventory["success"] == true
            && permissions["success"] == true
            && health["success"] == true
            && health["result"]["overall"] == "ok"
            && interactive_desktop["success"] == true
            && interactive_desktop["input_ready"] == true;
        json!({
            "type": "diagnostics",
            "schema_version": 1,
            "backend": "cua-driver-sdk",
            "ready": ready,
            "checks": {
                "driver": driver,
                "window_inventory": window_inventory,
                "permissions": permissions,
                "health": health,
                "interactive_desktop": interactive_desktop,
            },
        })
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
        let result = call_driver_tool(
            &self.driver,
            name,
            arguments.to_string(),
            &format!("call CUA {name}"),
        )
        .await?;
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
        interactive_desktop::require_desktop_observation_available()?;
        let mut arguments = json!({});
        if let Some(session) = session {
            arguments["session"] = Value::String(session.to_owned());
        }
        let result = call_driver_tool(
            &self.driver,
            "get_desktop_state",
            arguments.to_string(),
            "capture CUA desktop state",
        )
        .await?;
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

    async fn call_tool_value(&self, name: &str, arguments: Value) -> ComputerUseResult<Value> {
        let result = call_driver_tool(
            &self.driver,
            name,
            arguments.to_string(),
            &format!("call CUA {name}"),
        )
        .await?;
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

pub(crate) fn diagnostic_tool_check(result: ComputerUseResult<ComputerUseToolResult>) -> Value {
    match result {
        Ok(result) => json!({
            "success": true,
            "degraded": result.degraded,
            "summary": result.text,
            "result": result.value.get("structuredContent").unwrap_or(&result.value),
        }),
        Err(error) => json!({
            "success": false,
            "code": error.code,
            "message": error.message,
        }),
    }
}

pub(crate) fn tool_schema_from_inventory(
    inventory: &Value,
    name: &str,
) -> ComputerUseResult<Value> {
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
    if !driver.upstream_cursor_renderer_enabled() {
        return Ok(());
    }

    let result = call_driver_tool(
        &driver.driver,
        "set_agent_cursor_enabled",
        json!({"session": session_id, "enabled": true}).to_string(),
        context,
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            cleanup_started_session(driver, session_id).await;
            return Err(error);
        }
    };
    if let Err(error) = ensure_tool_ok(context, &result) {
        cleanup_started_session(driver, session_id).await;
        return Err(error);
    }
    let result = call_driver_tool(
        &driver.driver,
        "set_agent_cursor_motion",
        json!({"session": session_id, "glide_duration_ms": CURSOR_GLIDE_MS}).to_string(),
        context,
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            cleanup_started_session(driver, session_id).await;
            return Err(error);
        }
    };
    if let Err(error) = ensure_tool_ok(context, &result) {
        cleanup_started_session(driver, session_id).await;
        return Err(error);
    }
    Ok(())
}

async fn cleanup_started_session(driver: &ComputerUseDriver, session_id: &str) {
    let _ = call_driver_tool(
        &driver.driver,
        "end_session",
        json!({"session": session_id}).to_string(),
        "cleanup CUA session",
    )
    .await;
}

/// A long-lived, exact-window Computer Use session.
struct ActiveShowcase {
    recorder: ShowcaseRecorder,
    owns_live_observation: bool,
}

struct LiveObservationStartOutcome {
    state: Value,
    disposition: LiveObservationStartDisposition,
}

pub struct ComputerUseSession {
    driver: ComputerUseDriver,
    scope: ComputerUseTargetScope,
    app_name: String,
    agent_name: String,
    session_id: String,
    marker: ComputerUseMarker,
    control_banner: Option<ControlBanner>,
    target: Option<WindowTarget>,
    pub(crate) observation: Option<ComputerUseObservation>,
    action_evidence_epoch: ActionEvidenceEpoch,
    live_observation: Option<LiveObservation>,
    post_action_live_sequence_fence: Option<LiveObservationFence>,
    observation_transition_live_sequence_fence: Option<LiveObservationFence>,
    showcase: Option<ActiveShowcase>,
    last_recording_video: Option<RecordingVideoTerminalEvidence>,
    recording_active: bool,
    recording_expected_video: bool,
    recording_health: Option<RecordingHealth>,
    recording_keepalive: Option<RecordingKeepalive>,
    #[cfg(windows)]
    pub(crate) windows_uia: Option<WindowsUiaFallback>,
    last_upstream_session_refresh: Option<Instant>,
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
    fn new(
        driver: ComputerUseDriver,
        agent_name: String,
        session_id: String,
    ) -> ComputerUseResult<Self> {
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
                label: localized_control_label(&agent_name, "Desktop"),
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
        interactive_desktop::require_desktop_observation_available()?;
        let result = call_driver_tool(
            &self.driver.driver,
            "start_session",
            json!({
                "session": self.session_id,
                "capture_scope": "desktop",
                "cursor_theme": {"theme_id": MOUSE_CURSOR_THEME, "reduced_motion": "auto"},
                "_public_session_label": self.marker.label,
            })
            .to_string(),
            "start CUA desktop session",
        )
        .await?;
        ensure_tool_ok("start CUA desktop session", &result)?;
        enable_session_marker(&self.driver, &self.session_id, "show CUA desktop marker").await?;
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
        let snapshot = tokio::time::timeout(
            INPUT_CALL_TIMEOUT,
            self.driver.desktop_snapshot_for(Some(&self.session_id)),
        )
        .await
        .map_err(|_| {
            ComputerUseError::new(
                ComputerUseErrorCode::InputFailed,
                "CUA desktop snapshot timed out; the desktop session was invalidated",
            )
        })??;
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
            || action.element_token.is_some()
            || matches!(action.action.as_str(), "set_text" | "set_value")
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
        interactive_desktop::require_input_available()?;
        let arguments = desktop_action_arguments(action, &self.session_id);
        let tool = arguments["_tool"].as_str().unwrap_or_default().to_owned();
        let mut arguments = arguments;
        arguments
            .as_object_mut()
            .expect("desktop action arguments are an object")
            .remove("_tool");
        let result = call_driver_tool(
            &self.driver.driver,
            tool.clone(),
            arguments.to_string(),
            &format!("execute desktop CUA {tool}"),
        )
        .await?;
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
        let result = call_driver_tool(
            &self.driver.driver,
            "end_session",
            json!({"session": self.session_id}).to_string(),
            "stop CUA desktop session",
        )
        .await;
        if let Err(error) = &result {
            self.active = false;
            self.marker.visible = false;
            self.latest_observation_id = None;
            return Err(error.clone());
        }
        let result = result?;
        ensure_tool_ok("stop CUA desktop session", &result)?;
        self.active = false;
        self.marker.visible = false;
        self.latest_observation_id = None;
        Ok(json!({"success": true, "active": false, "marker": self.marker}))
    }
}

#[cfg(windows)]
fn desktop_crop_bounds(target: &WindowTarget) -> ComputerUseResult<([i32; 4], u32)> {
    let window_handle = usize::try_from(target.window_id).map_err(|_| {
        ComputerUseError::new(
            ComputerUseErrorCode::InvalidTarget,
            "window handle does not fit the current Windows target",
        )
    })?;
    let dpi = unsafe {
        windows_sys::Win32::UI::HiDpi::GetDpiForWindow(window_handle as *mut core::ffi::c_void)
    };
    Ok((scale_bounds_for_dpi(target.bounds, dpi)?, dpi))
}

#[cfg(not(windows))]
fn desktop_crop_bounds(target: &WindowTarget) -> ComputerUseResult<([i32; 4], u32)> {
    Ok((target.bounds, 96))
}

#[cfg(windows)]
async fn capture_exact_window(window_id: u64) -> ComputerUseResult<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        platform_windows::capture::screenshot_window_bytes(window_id).map_err(|error| {
            ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
        })
    })
    .await
    .map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            format!("exact window capture task failed: {error}"),
        )
    })?
}

#[cfg(not(windows))]
async fn capture_exact_window(_window_id: u64) -> ComputerUseResult<Vec<u8>> {
    Err(ComputerUseError::new(
        ComputerUseErrorCode::BackendUnavailable,
        "exact native window capture is unavailable on this platform",
    ))
}

#[cfg(windows)]
fn mark_foreground_window(windows: &mut [WindowTarget]) {
    let foreground = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }
        as usize as u64;
    for window in windows {
        window.is_foreground = window.window_id == foreground;
    }
}

#[cfg(not(windows))]
fn mark_foreground_window(windows: &mut [WindowTarget]) {
    if windows.iter().any(|window| window.is_foreground) {
        return;
    }
    mark_foreground_by_z_index(windows);
}

#[cfg(any(not(windows), test))]
pub(crate) fn mark_foreground_by_z_index(windows: &mut [WindowTarget]) {
    let Some(highest) = windows.iter().filter_map(|window| window.z_index).max() else {
        return;
    };
    for window in windows {
        window.is_foreground = window.z_index == Some(highest);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WindowWaitProbeOutcome<T> {
    TimedOut,
    Completed(T),
}

pub(crate) async fn wait_for_window_probe_until<T>(
    deadline: tokio::time::Instant,
    probe: impl std::future::Future<Output = T>,
) -> WindowWaitProbeOutcome<T> {
    tokio::select! {
        biased;
        _ = tokio::time::sleep_until(deadline) => WindowWaitProbeOutcome::TimedOut,
        result = probe => WindowWaitProbeOutcome::Completed(result),
    }
}

fn window_wait_timeout() -> ComputerUseError {
    ComputerUseError::new(
        ComputerUseErrorCode::MissingWindow,
        "window query timed out",
    )
}

async fn list_windows_with_driver(
    driver: &Arc<CuaDriver>,
    pid: Option<u32>,
    on_screen_only: bool,
) -> ComputerUseResult<Vec<Value>> {
    if let Some(windows) = crate::window_target::native_inventory(pid, on_screen_only) {
        return Ok(windows);
    }
    let mut arguments = json!({});
    if let Some(pid) = pid {
        arguments["pid"] = json!(pid);
    }
    if on_screen_only {
        arguments["on_screen_only"] = Value::Bool(true);
    }
    let result = call_driver_tool(
        driver,
        "list_windows",
        arguments.to_string(),
        "list CUA windows",
    )
    .await?;
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
