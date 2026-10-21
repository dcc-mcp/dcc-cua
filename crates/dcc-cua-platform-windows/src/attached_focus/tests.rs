use super::*;
use rstest::rstest;
use std::cell::RefCell;

#[rstest]
fn native_attachment_probe_rechecks_stop_before_first_attach() {
    let stopped = std::cell::Cell::new(false);
    let attachments = std::cell::Cell::new(0);
    let events = RefCell::new(Vec::new());
    let result = run_responsive_attachment(
        || {
            events.borrow_mut().push("gate");
            if stopped.get() {
                Err(UiaError::UserInterrupted)
            } else {
                Ok(())
            }
        },
        || {
            events.borrow_mut().push("probe");
            stopped.set(true);
            Ok(())
        },
        || {
            events.borrow_mut().push("attach");
            attachments.set(attachments.get() + 1);
            Ok(())
        },
    );
    assert_eq!(
        attachments.get(),
        0,
        "stop during the probe must prevent the first native attachment"
    );
    assert!(matches!(result, Err(UiaError::UserInterrupted)));
    assert_eq!(events.into_inner(), ["gate", "probe", "gate"]);
}

#[rstest]
#[case("ready", 1, false)]
#[case("same_thread", 1, false)]
#[case("foreground_attachment", 1, false)]
#[case("attachment_failed", 0, false)]
#[case("already_foreground", 0, false)]
#[case("gate_rejected", 0, true)]
#[case("instance_changed", 0, true)]
#[case("hidden", 0, true)]
#[case("minimized", 0, true)]
#[case("disabled", 0, true)]
#[case("focus_error", 1, true)]
fn native_attached_focus_is_single_gated_and_detaches_before_return(
    #[case] variant: &str,
    #[case] expected_calls: usize,
    #[case] expected_error: bool,
) {
    let target = UiaTarget {
        process_id: 42,
        window_handle: 77,
    };
    let expected = crate::ExactWindowPixelInstanceEvidence {
        process_creation_time_100ns: 7,
        window_thread_id: 2,
        window_class_hash: 9,
        owner_window_handle: 0,
    };
    let mut queues = AttachedInputQueues {
        caller_thread_id: 1,
        target_thread_id: 2,
        foreground_thread_id: 3,
        attached_target: true,
        attached_foreground: true,
    };
    if variant == "same_thread" {
        queues.caller_thread_id = queues.target_thread_id;
        queues.attached_target = false;
    } else if variant == "foreground_attachment" {
        queues.foreground_thread_id = queues.target_thread_id;
        queues.attached_target = false;
    } else if variant == "attachment_failed" {
        queues.attached_target = false;
    }
    let events = RefCell::new(Vec::new());
    let previous_focus_window_handle = if variant == "same_thread" {
        0x8000_0001
    } else {
        0
    };
    let last_error_on_null =
        (previous_focus_window_handle == 0).then_some(if variant == "foreground_attachment" {
            87
        } else {
            0
        });
    let (diagnostic, outcome) = run_attached_focus(
        target,
        expected,
        queues,
        || {
            events.borrow_mut().push("preflight");
            if variant == "gate_rejected" {
                return Err(UiaError::UserInterrupted);
            }
            if variant == "disabled" {
                return Err(UiaError::OperationFailed(variant.into()));
            }
            let mut state = crate::ExactWindowNativeState {
                process_id: 42,
                window_handle: 77,
                bounds: Some([0, 0, 800, 600]),
                visible_bounds: Some([0, 0, 800, 600]),
                dpi: 96,
                visible: true,
                minimized: false,
                maximized: false,
                foreground: variant == "already_foreground",
                instance: expected,
            };
            if variant == "instance_changed" {
                state.instance.process_creation_time_100ns += 1;
            }
            if variant == "hidden" {
                state.visible = false;
            }
            if variant == "minimized" {
                state.minimized = true;
            }
            Ok(state)
        },
        || {
            events.borrow_mut().push("focus");
            if variant == "focus_error" {
                return Err(UiaError::OperationFailed(variant.into()));
            }
            Ok(AttachedFocusCall {
                previous_focus_window_handle,
                last_error_on_null,
                foreground_observed: false,
            })
        },
        || {
            if queues.attached_target {
                events.borrow_mut().push("detach_target");
            }
            if queues.attached_foreground {
                events.borrow_mut().push("detach_foreground");
            }
        },
    );
    let events = events.into_inner();
    if variant == "gate_rejected" {
        assert!(matches!(&outcome, Err(UiaError::UserInterrupted)));
    }
    assert_eq!(
        events.iter().filter(|event| **event == "focus").count(),
        expected_calls
    );
    assert_eq!(
        outcome.is_err(),
        expected_error,
        "guard/focus failure {variant}"
    );
    let cleanup = events
        .iter()
        .filter(|event| event.starts_with("detach"))
        .copied()
        .collect::<Vec<_>>();
    let expected_cleanup = [
        queues.attached_target.then_some("detach_target"),
        queues.attached_foreground.then_some("detach_foreground"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    assert_eq!(cleanup, expected_cleanup);
    assert!(
        events
            .iter()
            .position(|event| event.starts_with("detach"))
            .is_none_or(|position| events[position..]
                .iter()
                .all(|event| event.starts_with("detach")))
    );
    if expected_calls == 1 && !expected_error {
        let diagnostic =
            diagnostic.expect("a dispatched SetFocus retains its raw HWND, including NULL");
        assert_eq!(
            diagnostic.previous_focus_window_handle,
            previous_focus_window_handle
        );
        assert_eq!(diagnostic.last_error_on_null, last_error_on_null);
        assert!(
            !diagnostic.foreground_observed,
            "NULL or a previous HWND never proves activation"
        );
    } else {
        assert!(diagnostic.is_none());
    }
}
