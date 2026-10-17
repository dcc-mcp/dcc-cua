use super::*;
use crate::ExactWindowPixelInstanceEvidence;
use rstest::rstest;
use std::cell::{Cell, RefCell};

fn proof() -> NativeMenuProof {
    let popup = ExactWindowNativeState {
        process_id: 42,
        window_handle: 99,
        bounds: Some([20, 30, 240, 160]),
        visible_bounds: Some([20, 30, 240, 160]),
        dpi: 96,
        visible: true,
        minimized: false,
        maximized: false,
        foreground: false,
        instance: ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: 123,
            window_thread_id: 7,
            window_class_hash: 555,
            owner_window_handle: 0,
        },
    };
    let mut owner = popup;
    owner.window_handle = 100;
    owner.visible = false;
    owner.bounds = None;
    owner.visible_bounds = None;
    owner.instance.window_class_hash = 777;
    NativeMenuProof {
        popup,
        owner,
        flags: GUI_INMENUMODE | GUI_POPUPMENUMODE,
        active_window_handle: 0,
        capture_window_handle: 99,
    }
}

#[rstest]
#[case("valid_hidden_owner", true)]
#[case("not_standard", false)]
#[case("wrong_pid", false)]
#[case("wrong_target", false)]
#[case("hidden_popup", false)]
#[case("bad_bounds", false)]
#[case("no_birth", false)]
#[case("no_thread", false)]
#[case("owner_foreign", false)]
#[case("owner_birth", false)]
#[case("owner_thread", false)]
#[case("owner_is_popup", false)]
#[case("foreign_popup_owner", false)]
#[case("no_owner", false)]
#[case("inactive", false)]
#[case("nonpopup_menu", false)]
#[case("system_menu", false)]
#[case("move_size", false)]
#[case("move_window", false)]
#[case("foreign_active", false)]
#[case("foreign_capture", false)]
#[case("missing_popup", false)]
#[case("ambiguous_popups", false)]
fn menu_authority_requires_a_complete_active_unique_native_relationship(
    #[case] scenario: &str,
    #[case] allowed: bool,
) {
    let mut actual = proof();
    let mut roots = vec![99];
    let mut move_window = 0;
    match scenario {
        "wrong_pid" => actual.popup.process_id += 1,
        "wrong_target" => actual.popup.window_handle += 1,
        "hidden_popup" => actual.popup.visible = false,
        "bad_bounds" => actual.popup.bounds = Some([0, 0, 0, 2]),
        "no_birth" => actual.popup.instance.process_creation_time_100ns = 0,
        "no_thread" => actual.popup.instance.window_thread_id = 0,
        "owner_foreign" => actual.owner.process_id += 1,
        "owner_birth" => actual.owner.instance.process_creation_time_100ns += 1,
        "owner_thread" => actual.owner.instance.window_thread_id += 1,
        "owner_is_popup" => actual.owner.window_handle = 99,
        "foreign_popup_owner" => actual.popup.instance.owner_window_handle = 101,
        "no_owner" => actual.owner.window_handle = 0,
        "inactive" => actual.flags = 0,
        "nonpopup_menu" => actual.flags = GUI_INMENUMODE,
        "system_menu" => actual.flags |= GUI_SYSTEMMENUMODE,
        "move_size" => actual.flags |= GUI_INMOVESIZE,
        "move_window" => move_window = 100,
        "foreign_active" => actual.active_window_handle = 101,
        "foreign_capture" => actual.capture_window_handle = 101,
        "missing_popup" => roots.clear(),
        "ambiguous_popups" => roots.push(101),
        _ => {}
    }
    assert_eq!(
        validate_proof(
            UiaTarget {
                process_id: 42,
                window_handle: 99
            },
            &actual,
            scenario != "not_standard",
            &roots,
            move_window,
        )
        .is_ok(),
        allowed,
        "{scenario}"
    );
}

#[rstest]
#[case("success", true, 1)]
#[case("stopped", false, 0)]
#[case("stopped_during_read", false, 0)]
#[case("read_failed", false, 0)]
#[case("owner_reused", false, 0)]
#[case("popup_reused", false, 0)]
#[case("relationship_changed", false, 0)]
#[case("timeout", false, 1)]
#[case("still_visible", false, 1)]
#[case("owner_exited", false, 1)]
#[case("stopped_after_send", false, 1)]
fn production_sequence_sends_once_and_never_turns_uncertainty_into_closed(
    #[case] scenario: &str,
    #[case] success: bool,
    #[case] attempts: usize,
) {
    let expected = proof();
    let interrupted = Cell::new(scenario == "stopped");
    let events = RefCell::new(Vec::new());
    let result = run_sequence(
        &expected,
        || {
            events.borrow_mut().push("gate");
            if interrupted.get() {
                Err(refused("stopped or desktop unavailable"))
            } else {
                Ok(())
            }
        },
        || {
            events.borrow_mut().push("read");
            let mut actual = expected.clone();
            match scenario {
                "read_failed" => return Err(refused("inactive but visible")),
                "owner_reused" => actual.owner.instance.process_creation_time_100ns += 1,
                "popup_reused" => actual.popup.instance.window_class_hash += 1,
                "relationship_changed" => actual.capture_window_handle = 0,
                "stopped_during_read" => interrupted.set(true),
                _ => {}
            }
            Ok(actual)
        },
        |_gate| {
            events.borrow_mut().push("send");
            if scenario == "timeout" {
                return Err(NativeMenuDismissError::AfterDispatch(refused("timed out")));
            }
            if scenario == "stopped_after_send" {
                interrupted.set(true);
            }
            Ok(())
        },
        |gate| {
            gate()?;
            events.borrow_mut().push("readback");
            if matches!(scenario, "still_visible" | "owner_exited") {
                Err(refused("postcondition unconfirmed"))
            } else {
                Ok(())
            }
        },
    );
    assert_eq!(result.is_ok(), success);
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|&&event| event == "send")
            .count(),
        attempts
    );
    match result {
        Ok(receipt) => {
            assert!(receipt.popup_hidden_or_gone && receipt.menu_inactive && receipt.owner_alive);
            assert!(receipt.message_attempted);
            assert!(!receipt.input_sent);
        }
        Err(error) => assert_eq!(
            matches!(error, NativeMenuDismissError::AfterDispatch(_)),
            attempts == 1
        ),
    }
}

#[rstest]
#[case("stop")]
#[case("desktop")]
#[case("owner_reuse")]
#[case("popup_reuse")]
fn actual_sender_rechecks_authority_after_thread_scheduling(#[case] scenario: &'static str) {
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let scheduled = Arc::new(Barrier::new(2));
    let changed = Arc::new(AtomicBool::new(false));
    let messages = AtomicUsize::new(0);
    let expected = proof();
    let controller_barrier = Arc::clone(&scheduled);
    let controller_changed = Arc::clone(&changed);
    let controller = std::thread::spawn(move || {
        controller_barrier.wait();
        controller_changed.store(true, Ordering::Release);
        controller_barrier.wait();
    });
    let caller_thread = std::thread::current().id();
    let result = run_on_sender(|| {
        assert_ne!(std::thread::current().id(), caller_thread);
        // Freeze the real production sender after creation. A caller-side
        // cached permit would miss each change made while it is unscheduled.
        scheduled.wait();
        scheduled.wait();
        run_sequence(
            &expected,
            || {
                if changed.load(Ordering::Acquire) && matches!(scenario, "stop" | "desktop") {
                    Err(refused("current lifecycle or desktop revoked"))
                } else {
                    Ok(())
                }
            },
            || {
                let mut current = expected.clone();
                if changed.load(Ordering::Acquire) {
                    if scenario == "owner_reuse" {
                        current.owner.instance.process_creation_time_100ns += 1;
                    }
                    if scenario == "popup_reuse" {
                        current.popup.instance.window_class_hash += 1;
                    }
                }
                Ok(current)
            },
            |_| {
                messages.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_| panic!("no post-read without a message"),
        )
    });
    controller.join().unwrap();
    assert!(matches!(
        result,
        Err(NativeMenuDismissError::BeforeDispatch(_))
    ));
    assert_eq!(messages.load(Ordering::SeqCst), 0);
}
