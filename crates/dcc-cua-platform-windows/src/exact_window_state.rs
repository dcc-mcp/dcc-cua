//! Native exact-instance state and one explicitly requested minimize mutation.
use serde::Serialize;
use thiserror::Error;
use windows::Win32::{
    Foundation::{HWND, RECT},
    UI::{
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{GetForegroundWindow, GetWindowRect, IsIconic, IsWindowVisible},
    },
};

use crate::{
    UiaError, UiaTarget,
    capture_identity::validate_exact_window_owner,
    visible_capture::{
        ExactWindowPixelInstanceEvidence, ThreadDpiAwarenessGuard, exact_window_instance_evidence,
        physical_root_bounds, physical_window_rect,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ExactWindowNativeState {
    pub process_id: u32,
    pub window_handle: u64,
    pub bounds: Option<[i32; 4]>,
    /// Actual DWM extended bounds; absent metadata never substitutes Win32 bounds.
    /// Pixel mutation fences require this field; semantic state reads may report None.
    pub visible_bounds: Option<[i32; 4]>,
    pub dpi: u32,
    pub visible: bool,
    pub minimized: bool,
    pub foreground: bool,
    pub instance: ExactWindowPixelInstanceEvidence,
}

/// No UIA, pixel capture, activation or visibility requirement. Minimized state
/// is observable without pretending it supplies a fresh screenshot token.
pub fn exact_window_native_state(
    process_id: u32,
    window_handle: u64,
) -> Result<ExactWindowNativeState, UiaError> {
    let _dpi = ThreadDpiAwarenessGuard::per_monitor_v2().map_err(|_| {
        UiaError::OperationFailed("native physical coordinate scope is unavailable".into())
    })?;
    validate_exact_window_owner(process_id, window_handle)
        .map_err(|_| UiaError::InvalidTarget("exact native root identity is unavailable".into()))?;
    let before = exact_window_instance_evidence(process_id, window_handle)
        .map_err(|_| UiaError::InvalidTarget("exact native instance is unavailable".into()))?;
    let hwnd = HWND(window_handle as usize as *mut _);
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }
        .map_err(|_| UiaError::OperationFailed("native physical bounds are unavailable".into()))?;
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        return Err(UiaError::InvalidTarget(
            "exact native DPI is unavailable".into(),
        ));
    }
    let state = ExactWindowNativeState {
        process_id,
        window_handle,
        bounds: physical_root_bounds(rect),
        visible_bounds: physical_window_rect(hwnd)
            .ok()
            .and_then(physical_root_bounds),
        dpi,
        visible: unsafe { IsWindowVisible(hwnd) }.as_bool(),
        minimized: unsafe { IsIconic(hwnd) }.as_bool(),
        foreground: unsafe { GetForegroundWindow() } == hwnd,
        instance: before,
    };
    let after = exact_window_instance_evidence(process_id, window_handle).map_err(|_| {
        UiaError::InvalidTarget("exact native instance changed while reading state".into())
    })?;
    validate_exact_window_owner(process_id, window_handle).map_err(|_| {
        UiaError::InvalidTarget("exact native root changed while reading state".into())
    })?;
    if before != after {
        return Err(UiaError::InvalidTarget(
            "exact native instance changed while reading state".into(),
        ));
    }
    Ok(state)
}

#[derive(Debug, Error)]
pub enum ExactWindowMinimizeError {
    #[error("exact minimize was refused before dispatch: {0}")]
    BeforeDispatch(UiaError),
    #[error("exact minimize requires fresh evidence after dispatch: {0}")]
    AfterDispatch(UiaError),
}

fn validate_instance(
    target: UiaTarget,
    expected: ExactWindowPixelInstanceEvidence,
    state: &ExactWindowNativeState,
) -> Result<(), UiaError> {
    if target.process_id == 0
        || target.window_handle == 0
        || expected.process_creation_time_100ns == 0
        || expected.window_thread_id == 0
        || state.process_id != target.process_id
        || state.window_handle != target.window_handle
        || state.instance != expected
    {
        return Err(UiaError::InvalidTarget(
            "exact observed native instance changed".into(),
        ));
    }
    Ok(())
}

pub(crate) fn run_exact_minimize_sequence(
    target: UiaTarget,
    expected: ExactWindowPixelInstanceEvidence,
    mut gate: impl FnMut() -> Result<(), UiaError>,
    read_before: impl FnOnce() -> Result<ExactWindowNativeState, UiaError>,
    dispatch: impl FnOnce() -> Result<(), UiaError>,
    read_after: impl FnOnce() -> Result<ExactWindowNativeState, UiaError>,
) -> Result<ExactWindowNativeState, ExactWindowMinimizeError> {
    use ExactWindowMinimizeError::{AfterDispatch, BeforeDispatch};
    gate().map_err(BeforeDispatch)?;
    let before = read_before().map_err(BeforeDispatch)?;
    validate_instance(target, expected, &before).map_err(BeforeDispatch)?;
    if !before.visible || before.minimized {
        return Err(BeforeDispatch(UiaError::InvalidTarget(
            "exact observed target is hidden or already minimized".into(),
        )));
    }
    gate().map_err(BeforeDispatch)?;
    dispatch().map_err(AfterDispatch)?;
    let after = read_after().map_err(AfterDispatch)?;
    validate_instance(target, expected, &after).map_err(AfterDispatch)?;
    if !after.minimized {
        return Err(AfterDispatch(UiaError::OperationFailed(
            "exact target was not minimized after the request".into(),
        )));
    }
    Ok(after)
}

/// One bounded ShowWindowAsync request. Never activates, closes, terminates or
/// follows an owned popup. Full observed instance is checked after the input
/// gate, immediately before dispatch, and throughout native readback.
pub fn minimize_exact_window(
    target: UiaTarget,
    expected: ExactWindowPixelInstanceEvidence,
    input_available: impl FnMut() -> Result<(), UiaError>,
) -> Result<ExactWindowNativeState, ExactWindowMinimizeError> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWMINNOACTIVE, ShowWindowAsync};
    run_exact_minimize_sequence(
        target,
        expected,
        input_available,
        || exact_window_native_state(target.process_id, target.window_handle),
        || {
            if unsafe { ShowWindowAsync(target.window_handle as *mut _, SW_SHOWMINNOACTIVE) } == 0 {
                return Err(UiaError::OperationFailed(
                    "Windows refused the exact minimize request".into(),
                ));
            }
            Ok(())
        },
        || {
            for attempt in 0..20 {
                let state = exact_window_native_state(target.process_id, target.window_handle)?;
                validate_instance(target, expected, &state)?;
                if state.minimized {
                    return Ok(state);
                }
                if attempt < 19 {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
            Err(UiaError::OperationFailed(
                "exact minimize native readback deadline elapsed".into(),
            ))
        },
    )
}

#[cfg(test)]
mod tests;
