use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PixelObservationRoute {
    #[cfg(any(windows, test))]
    ExplicitPixelsOnly,
    AccessibilityUnavailableDegraded,
    AccessibilityTimeoutDegraded,
}

impl PixelObservationRoute {
    #[cfg(any(windows, test))]
    pub(super) const fn observation_mode(self) -> &'static str {
        match self {
            #[cfg(any(windows, test))]
            Self::ExplicitPixelsOnly => "pixels_only",
            Self::AccessibilityUnavailableDegraded => "accessibility_unavailable_degraded",
            Self::AccessibilityTimeoutDegraded => "accessibility_timeout_degraded",
        }
    }

    #[cfg(any(windows, test))]
    pub(super) const fn degraded(self) -> bool {
        !matches!(self, Self::ExplicitPixelsOnly)
    }
}

pub(super) fn pixel_route_for_accessibility_failure(
    error: &ComputerUseError,
) -> Option<PixelObservationRoute> {
    if error
        .details
        .as_ref()
        .is_some_and(|details| details.timed_out == Some(true))
    {
        return Some(PixelObservationRoute::AccessibilityTimeoutDegraded);
    }
    (error.code == ComputerUseErrorCode::BackendUnavailable)
        .then_some(PixelObservationRoute::AccessibilityUnavailableDegraded)
}

pub(super) fn pixel_route_for_uia_tool_failure(
    result: &cua_driver_sdk::ToolResult,
) -> Option<PixelObservationRoute> {
    if !is_uia_snapshot_failure(result) {
        return None;
    }
    match result.error_code.as_deref() {
        Some("uia_timeout") => Some(PixelObservationRoute::AccessibilityTimeoutDegraded),
        Some("backend_unavailable" | "input_failed" | "target_unavailable" | "missing_window") => {
            Some(PixelObservationRoute::AccessibilityUnavailableDegraded)
        }
        _ => None,
    }
}

#[cfg(any(windows, test))]
pub(super) fn validate_exact_window_pixel_target_state(
    target: &WindowTarget,
    unobscured: bool,
) -> ComputerUseResult<()> {
    if target.is_minimized {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetMinimized,
            "the exact pixel target is minimized",
        ));
    }
    if !target.is_on_screen {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetUnavailable,
            "the exact pixel target is hidden or off-screen",
        ));
    }
    if !unobscured {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "the exact pixel target is occluded; pixels were discarded",
        ));
    }
    Ok(())
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ExactWindowPixelGeometry {
    pub bounds: [i32; 4],
    pub dpi: u32,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExactWindowPixelCaptureMode {
    WindowContent,
    VisibleDesktopCrop,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ExactWindowPixelInstanceIdentity {
    pub(super) process_creation_time_100ns: u64,
    pub(super) window_thread_id: u32,
    pub(super) window_class_hash: u64,
    pub(super) owner_window_handle: u64,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ExactWindowPixelPublicationFence {
    pub geometry: ExactWindowPixelGeometry,
    pub source_rect: [i32; 4],
    pub generation: u64,
    pub mode: ExactWindowPixelCaptureMode,
    pub instance: ExactWindowPixelInstanceIdentity,
}

#[cfg(windows)]
impl From<dcc_cua_platform_windows::ExactWindowPixelInstanceEvidence>
    for ExactWindowPixelInstanceIdentity
{
    fn from(value: dcc_cua_platform_windows::ExactWindowPixelInstanceEvidence) -> Self {
        Self {
            process_creation_time_100ns: value.process_creation_time_100ns,
            window_thread_id: value.window_thread_id,
            window_class_hash: value.window_class_hash,
            owner_window_handle: value.owner_window_handle,
        }
    }
}

#[cfg(any(windows, test))]
fn validate_final_exact_window_pixel_instance(
    captured: ExactWindowPixelInstanceIdentity,
    final_instance: ExactWindowPixelInstanceIdentity,
) -> ComputerUseResult<()> {
    if captured != final_instance {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "the exact process or HWND instance changed during pixel capture",
        ));
    }
    Ok(())
}

#[cfg(any(windows, test))]
impl ExactWindowPixelCaptureMode {
    const fn requires_unobscured_desktop(self) -> bool {
        matches!(self, Self::VisibleDesktopCrop)
    }
}

#[cfg(any(windows, test))]
pub(super) fn validate_final_exact_window_pixel_publication(
    captured_target: &WindowTarget,
    final_inventory: &WindowTarget,
    captured: ExactWindowPixelPublicationFence,
    final_fence: ExactWindowPixelPublicationFence,
    final_unobscured: bool,
) -> ComputerUseResult<()> {
    validate_final_exact_window_pixel_instance(captured.instance, final_fence.instance)?;
    if captured_target.bounds != captured.geometry.bounds
        || final_inventory.bounds != final_fence.geometry.bounds
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "the final native bounds do not match the captured or inventoried exact window",
        ));
    }
    if final_fence.generation <= captured.generation || final_fence.mode != captured.mode {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "the final exact-window capture generation or mode is stale",
        ));
    }
    if captured.source_rect != final_fence.source_rect
        || (captured.mode == ExactWindowPixelCaptureMode::WindowContent
            && (captured.source_rect != captured.geometry.bounds
                || final_fence.source_rect != final_fence.geometry.bounds))
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "the exact-window pixel source rectangle changed before final publication",
        ));
    }
    if captured_target.pid != final_inventory.pid
        || captured_target.window_id != final_inventory.window_id
        || captured_target.bounds != final_inventory.bounds
        || captured.geometry.dpi != final_fence.geometry.dpi
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "the exact PID/HWND, bounds, or DPI changed before final publication",
        ));
    }
    validate_exact_window_pixel_target_state(
        final_inventory,
        !captured.mode.requires_unobscured_desktop() || final_unobscured,
    )
}

#[cfg(any(windows, test))]
pub(super) fn exact_window_pixel_provenance(
    route: PixelObservationRoute,
    target: &WindowTarget,
    capture_generation: u64,
    window_dpi: u32,
    backend: &str,
) -> Value {
    json!({
        "backend": backend,
        "pixels_captured": true,
        "scope": "window",
        "whole_desktop_capture": false,
        "observation_mode": route.observation_mode(),
        "degraded": route.degraded(),
        "accessibility_available": false,
        "process_id": target.pid,
        "window_handle": target.window_id,
        "native_window_bounds": target.bounds,
        "capture_generation": capture_generation,
        "window_dpi": window_dpi,
    })
}

#[cfg(windows)]
pub(super) fn validate_native_exact_window_pixel_evidence(
    before: &dcc_cua_platform_windows::ExactWindowPixelEvidence,
    after: &dcc_cua_platform_windows::ExactWindowPixelEvidence,
    capture_mode: ExactWindowPixelCaptureMode,
) -> ComputerUseResult<()> {
    if before.process_id != after.process_id
        || before.window_handle != after.window_handle
        || before.instance != after.instance
        || before.bounds != after.bounds
        || before.visible_bounds != after.visible_bounds
        || before.dpi != after.dpi
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "the native exact-window identity, bounds, or DPI changed during pixel capture",
        ));
    }
    if before.minimized || after.minimized {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetMinimized,
            "the exact pixel target became minimized during capture",
        ));
    }
    if !before.visible || !after.visible {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetUnavailable,
            "the exact pixel target became hidden during capture",
        ));
    }
    if capture_mode.requires_unobscured_desktop() && (!before.unobscured || !after.unobscured) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "the exact pixel target was occluded; pixels were discarded",
        ));
    }
    Ok(())
}
