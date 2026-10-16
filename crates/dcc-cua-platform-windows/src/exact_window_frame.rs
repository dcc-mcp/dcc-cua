//! One explicitly authorized, non-activating exact-instance frame mutation.
use thiserror::Error;

use crate::visible_capture::{ExactWindowPixelInstanceEvidence, ThreadDpiAwarenessGuard};
use crate::{ExactWindowNativeState, UiaError, UiaTarget, exact_window_native_state};

const READBACK_ATTEMPTS: usize = 40;
pub(crate) const FRAME_FLAGS: u32 = windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE
    | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
    | windows_sys::Win32::UI::WindowsAndMessaging::SWP_ASYNCWINDOWPOS;

#[derive(Debug, Error)]
pub enum ExactWindowFrameError {
    #[error("exact frame mutation was refused before dispatch: {0}")]
    BeforeDispatch(UiaError),
    #[error("exact frame mutation requires fresh evidence after dispatch: {0}")]
    AfterDispatch(UiaError),
}

pub fn validate_exact_window_frame(frame: [i32; 4]) -> Result<(), UiaError> {
    let [x, y, width, height] = frame;
    if width <= 0
        || height <= 0
        || x.checked_add(width).is_none()
        || y.checked_add(height).is_none()
    {
        return Err(UiaError::InvalidAction(
            "physical frame has empty or overflowing extents".into(),
        ));
    }
    Ok(())
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
            "exact metadata native instance changed".into(),
        ));
    }
    Ok(())
}

pub(crate) fn run_exact_window_frame_sequence(
    target: UiaTarget,
    expected: ExactWindowNativeState,
    requested: [i32; 4],
    mut gate: impl FnMut() -> Result<(), UiaError>,
    mut read: impl FnMut() -> Result<ExactWindowNativeState, UiaError>,
    dispatch: impl FnOnce() -> Result<(), UiaError>,
    mut wait: impl FnMut(),
) -> Result<ExactWindowNativeState, ExactWindowFrameError> {
    use ExactWindowFrameError::{AfterDispatch, BeforeDispatch};
    validate_exact_window_frame(requested).map_err(BeforeDispatch)?;
    validate_instance(target, expected.instance, &expected).map_err(BeforeDispatch)?;
    for frame in [expected.bounds, expected.visible_bounds] {
        validate_exact_window_frame(frame.ok_or_else(|| {
            BeforeDispatch(UiaError::InvalidTarget(
                "fresh physical metadata is incomplete".into(),
            ))
        })?)
        .map_err(BeforeDispatch)?;
    }
    gate().map_err(BeforeDispatch)?;
    let before = read().map_err(BeforeDispatch)?;
    validate_instance(target, expected.instance, &before).map_err(BeforeDispatch)?;
    if before.bounds.is_none()
        || before.bounds != expected.bounds
        || before.visible_bounds != expected.visible_bounds
        || before.dpi == 0
        || before.dpi != expected.dpi
        || !before.visible
        || before.minimized
    {
        return Err(BeforeDispatch(UiaError::InvalidTarget(
            "fresh physical metadata changed, or target is hidden/minimized".into(),
        )));
    }
    gate().map_err(BeforeDispatch)?;
    dispatch().map_err(AfterDispatch)?;
    for attempt in 0..READBACK_ATTEMPTS {
        let after = read().map_err(AfterDispatch)?;
        validate_instance(target, expected.instance, &after).map_err(AfterDispatch)?;
        for frame in [after.bounds, after.visible_bounds] {
            validate_exact_window_frame(frame.ok_or_else(|| {
                AfterDispatch(UiaError::InvalidTarget(
                    "physical readback metadata is incomplete".into(),
                ))
            })?)
            .map_err(AfterDispatch)?;
        }
        if after.dpi != expected.dpi || !after.visible || after.minimized {
            return Err(AfterDispatch(UiaError::InvalidTarget(
                "native frame readback changed DPI or visibility".into(),
            )));
        }
        if after.bounds == Some(requested) {
            return Ok(after);
        }
        if attempt + 1 < READBACK_ATTEMPTS {
            wait();
        }
    }
    Err(AfterDispatch(UiaError::OperationFailed(
        "exact physical frame readback deadline elapsed".into(),
    )))
}

/// No capture, UIA, restore, foreground takeover, or z-order change is requested.
/// The same full native instance is required before the single dispatch and on
/// every bounded readback; metadata cannot authorize a different window.
pub fn set_exact_window_frame(
    target: UiaTarget,
    expected: ExactWindowNativeState,
    requested: [i32; 4],
    input_available: impl FnMut() -> Result<(), UiaError>,
) -> Result<ExactWindowNativeState, ExactWindowFrameError> {
    let _dpi = ThreadDpiAwarenessGuard::per_monitor_v2().map_err(|_| {
        ExactWindowFrameError::BeforeDispatch(UiaError::OperationFailed(
            "physical frame coordinate scope unavailable".into(),
        ))
    })?;
    run_exact_window_frame_sequence(
        target,
        expected,
        requested,
        input_available,
        || exact_window_native_state(target.process_id, target.window_handle),
        || {
            let [x, y, width, height] = requested;
            if unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SetWindowPos(
                    target.window_handle as *mut _,
                    std::ptr::null_mut(),
                    x,
                    y,
                    width,
                    height,
                    FRAME_FLAGS,
                )
            } == 0
            {
                return Err(UiaError::OperationFailed(
                    "Windows refused exact physical frame request".into(),
                ));
            }
            Ok(())
        },
        || std::thread::sleep(std::time::Duration::from_millis(5)),
    )
}

#[cfg(test)]
mod tests;
