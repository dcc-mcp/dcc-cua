use super::*;
use rstest::rstest;
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
fn minimize_final_gate_refuses_dwm_motion_during_native_state_read() {
    use crate::{NativeWindowGeometry, validate_native_window_geometry};
    let expected = NativeWindowGeometry {
        win32_bounds: [60, 80, 672, 508],
        dwm_bounds: Some([73, 80, 646, 495]),
        dpi: 240,
    };
    let actual = Cell::new(expected);
    let dispatched = Cell::new(false);
    let current = state();
    let result = run_exact_minimize_sequence(
        UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        current.instance,
        || {
            validate_native_window_geometry(expected, actual.get())
                .map_err(|_| UiaError::InvalidTarget("captured DWM bounds changed".into()))
        },
        || {
            let mut moved = actual.get();
            moved.dwm_bounds.as_mut().unwrap()[0] += 1;
            actual.set(moved);
            Ok(current)
        },
        || {
            dispatched.set(true);
            Ok(())
        },
        || panic!("no readback without dispatch"),
    );
    assert!(matches!(
        result,
        Err(ExactWindowMinimizeError::BeforeDispatch(_))
    ));
    assert!(!dispatched.get());
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
        assert_eq!(events.borrow()[..4], ["gate", "before", "gate", "dispatch"]);
    }
}

#[rstest]
fn exact_minimize_late_interruption_gate_prevents_dispatch() {
    let expected = state();
    let interrupted = Cell::new(false);
    let events = RefCell::new(Vec::new());
    let result = run_exact_minimize_sequence(
        UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        expected.instance,
        || {
            events.borrow_mut().push("gate");
            if interrupted.get() {
                Err(UiaError::OperationFailed("interrupted".into()))
            } else {
                Ok(())
            }
        },
        || {
            events.borrow_mut().push("before");
            interrupted.set(true);
            Ok(expected)
        },
        || {
            events.borrow_mut().push("dispatch");
            Ok(())
        },
        || {
            events.borrow_mut().push("after");
            let mut after = expected;
            after.minimized = true;
            Ok(after)
        },
    );
    assert!(matches!(
        result,
        Err(ExactWindowMinimizeError::BeforeDispatch(_))
    ));
    assert_eq!(*events.borrow(), ["gate", "before", "gate"]);
}
