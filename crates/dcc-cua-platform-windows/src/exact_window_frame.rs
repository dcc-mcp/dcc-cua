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
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn state() -> ExactWindowNativeState {
        ExactWindowNativeState {
            process_id: 42,
            window_handle: 99,
            bounds: Some([80, 80, 1500, 1400]),
            visible_bounds: Some([80, 80, 1500, 1400]),
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

    #[test]
    fn physical_frame_dispatch_flags_never_request_activation_or_z_order() {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        assert_eq!(
            FRAME_FLAGS,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_ASYNCWINDOWPOS
        );
        for bad in [
            [0, 0, 0, 1],
            [0, 0, 1, -1],
            [i32::MAX, 0, 1, 1],
            [0, i32::MAX, 1, 1],
        ] {
            assert!(validate_exact_window_frame(bad).is_err());
        }
        assert!(validate_exact_window_frame([-100, 50, 926, 680]).is_ok());
    }

    #[test]
    fn exact_frame_full_instance_and_geometry_matrix_is_cpu_only() {
        for change in 0..12 {
            let expected = state();
            let mut before = expected;
            match change {
                0 => before.process_id += 1,
                1 => before.window_handle += 1,
                2 => before.instance.process_creation_time_100ns += 1,
                3 => before.instance.window_thread_id += 1,
                4 => before.instance.window_class_hash += 1,
                5 => before.instance.owner_window_handle += 1,
                6 => before.bounds.as_mut().unwrap()[0] += 1,
                7 => before.visible_bounds.as_mut().unwrap()[0] += 1,
                8 => before.dpi += 1,
                9 => before.visible = false,
                10 => before.minimized = true,
                _ => before.bounds = None,
            }
            let dispatched = Cell::new(false);
            let result = run_exact_window_frame_sequence(
                UiaTarget {
                    process_id: 42,
                    window_handle: 99,
                },
                expected,
                [50, 800, 926, 680],
                || Ok(()),
                || Ok(before),
                || {
                    dispatched.set(true);
                    Ok(())
                },
                || {},
            );
            assert!(matches!(
                result,
                Err(ExactWindowFrameError::BeforeDispatch(_))
            ));
            assert!(!dispatched.get(), "change {change}");
        }
    }

    #[test]
    fn exact_frame_late_interrupt_prevents_single_dispatch() {
        let interrupted = Cell::new(false);
        let expected = state();
        let result = run_exact_window_frame_sequence(
            UiaTarget {
                process_id: 42,
                window_handle: 99,
            },
            expected,
            [50, 800, 926, 680],
            || {
                if interrupted.get() {
                    Err(UiaError::PermissionDenied("stopped".into()))
                } else {
                    Ok(())
                }
            },
            || {
                interrupted.set(true);
                Ok(expected)
            },
            || panic!("no dispatch"),
            || panic!("no wait"),
        );
        assert!(matches!(
            result,
            Err(ExactWindowFrameError::BeforeDispatch(_))
        ));
    }

    #[test]
    fn exact_frame_bounded_readback_never_redispatches_and_rejects_reuse() {
        for change in 0..8 {
            let expected = state();
            let reads = Cell::new(0);
            let sends = Cell::new(0);
            let waits = Cell::new(0);
            let result = run_exact_window_frame_sequence(
                UiaTarget {
                    process_id: 42,
                    window_handle: 99,
                },
                expected,
                [50, 800, 926, 680],
                || Ok(()),
                || {
                    reads.set(reads.get() + 1);
                    let mut current = expected;
                    if reads.get() > 1 {
                        current.bounds = Some([50, 800, 926, 680]);
                        match change {
                            0 => {}
                            1 => current.instance.process_creation_time_100ns += 1,
                            2 => current.instance.window_thread_id += 1,
                            3 => current.instance.window_class_hash += 1,
                            4 => current.instance.owner_window_handle += 1,
                            5 => current.process_id += 1,
                            6 => current.window_handle += 1,
                            _ => current.bounds = expected.bounds,
                        }
                    }
                    Ok(current)
                },
                || {
                    sends.set(sends.get() + 1);
                    Ok(())
                },
                || waits.set(waits.get() + 1),
            );
            assert_eq!(sends.get(), 1);
            assert_eq!(result.is_ok(), change == 0);
            if change == 7 {
                assert_eq!(reads.get(), 41);
                assert_eq!(waits.get(), 39);
            } else {
                assert_eq!(reads.get(), 2);
            }
        }
        let events = RefCell::new(Vec::new());
        let expected = state();
        let result = run_exact_window_frame_sequence(
            UiaTarget {
                process_id: 42,
                window_handle: 99,
            },
            expected,
            [50, 800, 926, 680],
            || {
                events.borrow_mut().push("gate");
                Ok(())
            },
            || {
                events.borrow_mut().push("read");
                Ok(expected)
            },
            || {
                events.borrow_mut().push("dispatch");
                Err(UiaError::OperationFailed("refused".into()))
            },
            || panic!("no wait"),
        );
        assert!(matches!(
            result,
            Err(ExactWindowFrameError::AfterDispatch(_))
        ));
        assert_eq!(*events.borrow(), ["gate", "read", "gate", "dispatch"]);
    }

    #[test]
    fn exact_frame_post_dispatch_metadata_failures_are_unknown_without_retry() {
        for change in 0..7 {
            let expected = state();
            let reads = Cell::new(0);
            let dispatches = Cell::new(0);
            let result = run_exact_window_frame_sequence(
                UiaTarget {
                    process_id: 42,
                    window_handle: 99,
                },
                expected,
                [50, 800, 926, 680],
                || Ok(()),
                || {
                    reads.set(reads.get() + 1);
                    if reads.get() == 1 {
                        return Ok(expected);
                    }
                    let mut after = expected;
                    after.bounds = Some([50, 800, 926, 680]);
                    match change {
                        0 => after.dpi += 1,
                        1 => after.visible = false,
                        2 => after.minimized = true,
                        3 => after.bounds = None,
                        4 => after.visible_bounds = None,
                        5 => after.visible_bounds = Some([0, 0, 0, 10]),
                        _ => return Err(UiaError::OperationFailed("readback failed".into())),
                    }
                    Ok(after)
                },
                || {
                    dispatches.set(dispatches.get() + 1);
                    Ok(())
                },
                || panic!("no readback retry after invalid metadata"),
            );
            assert!(matches!(
                result,
                Err(ExactWindowFrameError::AfterDispatch(_))
            ));
            assert_eq!(dispatches.get(), 1);
            assert_eq!(reads.get(), 2);
        }
    }
}
