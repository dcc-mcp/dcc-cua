use rstest::rstest;

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

#[rstest]
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

#[rstest]
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

#[rstest]
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

#[rstest]
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

#[rstest]
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
