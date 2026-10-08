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
        physical_root_bounds,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ExactWindowNativeState {
    pub process_id: u32,
    pub window_handle: u64,
    pub bounds: Option<[i32; 4]>,
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
    gate: impl FnOnce() -> Result<(), UiaError>,
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
    input_available: impl FnOnce() -> Result<(), UiaError>,
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
mod tests {
    use super::*;
    use rstest::rstest;
    use std::cell::RefCell;

    fn state() -> ExactWindowNativeState {
        ExactWindowNativeState {
            process_id: 42,
            window_handle: 99,
            bounds: Some([80, 80, 1500, 1400]),
            dpi: 240,
            visible: true,
            minimized: false,
            foreground: false,
            instance: ExactWindowPixelInstanceEvidence {
                process_creation_time_100ns: 7,
                window_thread_id: 8,
                window_class_hash: 9,
                owner_window_handle: 0,
            },
        }
    }

    #[rstest]
    #[case("success", true, true)]
    #[case("gate", false, false)]
    #[case("creation", false, false)]
    #[case("thread", false, false)]
    #[case("class", false, false)]
    #[case("owner", false, false)]
    #[case("pid", false, false)]
    #[case("hwnd", false, false)]
    #[case("hidden", false, false)]
    #[case("already_minimized", false, false)]
    #[case("dispatch_refused", false, true)]
    #[case("readback_changed", false, true)]
    #[case("not_minimized", false, true)]
    fn exact_minimize_full_instance_fences_single_dispatch(
        #[case] scenario: &str,
        #[case] success: bool,
        #[case] attempted: bool,
    ) {
        let expected = state();
        let mut before = expected;
        match scenario {
            "creation" => before.instance.process_creation_time_100ns += 1,
            "thread" => before.instance.window_thread_id += 1,
            "class" => before.instance.window_class_hash += 1,
            "owner" => before.instance.owner_window_handle += 1,
            "pid" => before.process_id += 1,
            "hwnd" => before.window_handle += 1,
            "hidden" => before.visible = false,
            "already_minimized" => before.minimized = true,
            _ => {}
        }
        let events = RefCell::new(Vec::new());
        let result = run_exact_minimize_sequence(
            UiaTarget {
                process_id: 42,
                window_handle: 99,
            },
            expected.instance,
            || {
                events.borrow_mut().push("gate");
                if scenario == "gate" {
                    Err(UiaError::OperationFailed("denied".into()))
                } else {
                    Ok(())
                }
            },
            || {
                events.borrow_mut().push("before");
                Ok(before)
            },
            || {
                events.borrow_mut().push("dispatch");
                if scenario == "dispatch_refused" {
                    Err(UiaError::OperationFailed("refused".into()))
                } else {
                    Ok(())
                }
            },
            || {
                events.borrow_mut().push("after");
                let mut after = expected;
                after.minimized = scenario != "not_minimized";
                if scenario == "readback_changed" {
                    after.instance.process_creation_time_100ns += 1;
                }
                Ok(after)
            },
        );
        assert_eq!(result.is_ok(), success);
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|&&event| event == "dispatch")
                .count(),
            usize::from(attempted)
        );
        if let Err(error) = result {
            assert_eq!(
                matches!(error, ExactWindowMinimizeError::AfterDispatch(_)),
                attempted
            );
        }
        if attempted {
            assert_eq!(events.borrow()[..3], ["gate", "before", "dispatch"]);
        }
    }
}
