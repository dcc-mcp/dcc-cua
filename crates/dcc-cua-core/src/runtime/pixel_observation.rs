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
pub(super) fn validate_exact_window_pixel_publication(
    before: &WindowTarget,
    after: &WindowTarget,
    before_dpi: u32,
    after_dpi: u32,
    before_generation: u64,
    after_generation: u64,
) -> ComputerUseResult<()> {
    if before.pid != after.pid
        || before.window_id != after.window_id
        || before.bounds != after.bounds
        || before_dpi != after_dpi
        || before_generation != after_generation
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "the exact PID/HWND, bounds, DPI, or capture generation changed during pixel capture",
        ));
    }
    validate_exact_window_pixel_target_state(after, true)
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
) -> ComputerUseResult<()> {
    if before.process_id != after.process_id
        || before.window_handle != after.window_handle
        || before.bounds != after.bounds
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
    if !before.unobscured || !after.unobscured {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "the exact pixel target was occluded; pixels were discarded",
        ));
    }
    Ok(())
}
