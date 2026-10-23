use super::*;
use rstest::rstest;
use std::cell::{Cell, RefCell};

struct ClosingEnterBackend {
    balanced: bool,
    batches: Vec<Vec<Event>>,
    reads: usize,
    now: Cell<Duration>,
}

impl InputBackend for ClosingEnterBackend {
    fn check(
        &mut self,
        _fence: WindowsPhysicalInputFence,
        _point: Option<(i32, i32)>,
    ) -> Result<(), Reason> {
        if !self.balanced {
            return Ok(());
        }
        completed_popup_postcheck(
            self.balanced,
            |_settling| {
                self.reads += 1;
                // Popup hidden with old capture, capture zero with selected
                // shadow, then the original unobscured-root postcheck succeeds.
                if self.reads < 3 {
                    (Err(Reason::TargetOccluded), true)
                } else {
                    (Ok(()), false)
                }
            },
            |duration| self.now.set(self.now.get() + duration),
            || self.now.get(),
        )
    }
    fn mutate(
        &mut self,
        _fence: WindowsPhysicalInputFence,
        _point: Option<(i32, i32)>,
        events: &[Event],
        _cursor: Option<(i32, i32)>,
        cleanup_only: bool,
    ) -> Result<MutationReceipt, Reason> {
        let receipt = MutationReceipt {
            requested: events.len() as u32,
            inserted: events.len() as u32,
            cursor_attempted: false,
            cursor_succeeded: false,
        };
        self.batches.push(events.to_vec());
        self.balanced = keyboard_release_completed(events, receipt, cleanup_only);
        Ok(receipt)
    }
    fn interrupted(&mut self) -> bool {
        false
    }
    fn elapsed_ms(&self) -> u64 {
        0
    }
    fn pause(&mut self, _milliseconds: u64) {}
}

#[rstest]
fn enter_balanced_batch_waits_read_only_for_the_same_popup_to_finish_closing() {
    let mut backend = ClosingEnterBackend {
        balanced: false,
        batches: vec![],
        reads: 0,
        now: Cell::new(Duration::ZERO),
    };
    let outcome = run_keypress(&mut backend, fixture().0, &["Enter".into()], None)
        .expect("balanced Enter may finish closing before the read-only root postcheck");
    assert_eq!(
        backend.batches,
        vec![vec![
            Event::Key(VK_RETURN, false),
            Event::Key(VK_RETURN, true)
        ]]
    );
    assert_eq!(backend.reads, 3);
    assert_eq!(outcome.inserted_events, 2);
    assert_eq!(outcome.cleanup_requested_events, 0);
    assert!(outcome.delivery_completed && outcome.post_dispatch_validated);
}

#[rstest]
#[case("not_balanced")]
#[case("foreign")]
#[case("desktop")]
#[case("escape")]
#[case("deadline")]
#[case("slow_read")]
#[case("pause_overrun")]
fn completed_popup_wait_never_replays_input_or_restarts_its_budget(#[case] case: &str) {
    let now = Cell::new(Duration::ZERO);
    let reads = Cell::new(0);
    let result = completed_popup_postcheck(
        case != "not_balanced",
        |_settling| {
            reads.set(reads.get() + 1);
            if case == "slow_read" {
                now.set(now.get() + Duration::from_millis(60));
                return if reads.get() == 1 {
                    (Err(Reason::TargetOccluded), true)
                } else {
                    (Ok(()), false)
                };
            }
            if reads.get() > 1 {
                match case {
                    "foreign" => return (Err(Reason::TargetOccluded), false),
                    "desktop" => return (Err(Reason::DesktopUnavailable), false),
                    "escape" => return (Err(Reason::Interrupted), false),
                    _ => {}
                }
            }
            (Err(Reason::TargetOccluded), true)
        },
        |duration| {
            now.set(
                now.get()
                    + if case == "pause_overrun" {
                        Duration::from_millis(105)
                    } else {
                        duration
                    },
            )
        },
        || now.get(),
    );
    assert!(result.is_err());
    assert_eq!(
        reads.get(),
        if case == "not_balanced" || case == "pause_overrun" {
            1
        } else if case == "deadline" {
            20
        } else {
            2
        }
    );
    assert!(
        now.get()
            <= Duration::from_millis(if case == "slow_read" {
                125
            } else if case == "pause_overrun" {
                105
            } else {
                100
            })
    );
}

#[rstest]
#[case(NativeCapturePopupRefusal::PopupNotUsable, true, true)]
#[case(NativeCapturePopupRefusal::CaptureNotSelected, false, true)]
#[case(NativeCapturePopupRefusal::PopupUnavailable, false, true)]
#[case(NativeCapturePopupRefusal::NativeStateUnavailable, false, true)]
#[case(NativeCapturePopupRefusal::NativeStateChanged, false, false)]
#[case(NativeCapturePopupRefusal::GuiStateUnavailable, false, false)]
#[case(NativeCapturePopupRefusal::UnselectedOccluder, false, false)]
fn only_known_closing_stages_can_share_the_completed_keyboard_budget(
    #[case] reason: NativeCapturePopupRefusal,
    #[case] starts: bool,
    #[case] continues: bool,
) {
    assert_eq!(may_wait_for_completed_popup(false, Some(reason)), starts);
    assert_eq!(may_wait_for_completed_popup(true, Some(reason)), continues);
    assert!(!may_wait_for_completed_popup(true, None));
}

fn fixture() -> (WindowsPhysicalInputFence, ExactWindowNativeState) {
    let instance = ExactWindowPixelInstanceEvidence {
        process_creation_time_100ns: 7,
        window_thread_id: 8,
        window_class_hash: 9,
        owner_window_handle: 0,
    };
    let fence = WindowsPhysicalInputFence {
        target: UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        native_instance: instance,
        native_window_bounds: [100, 100, 800, 600],
        native_visible_bounds: [100, 100, 800, 600],
        window_dpi: 96,
    };
    let state = ExactWindowNativeState {
        process_id: 42,
        window_handle: 99,
        instance,
        bounds: Some(fence.native_window_bounds),
        visible_bounds: Some(fence.native_visible_bounds),
        dpi: 96,
        visible: true,
        minimized: false,
        maximized: false,
        foreground: true,
    };
    (fence, state)
}

struct GeometryTransitionBackend {
    expected: NativeCapturePopupProof,
    current: NativeCapturePopupProof,
    after_delivery: NativeCapturePopupProof,
    released: bool,
    sent_events: usize,
}

impl InputBackend for GeometryTransitionBackend {
    fn check(
        &mut self,
        fence: WindowsPhysicalInputFence,
        _point: Option<(i32, i32)>,
    ) -> Result<(), Reason> {
        check_capture_popup_fence(
            fence,
            || Ok(()),
            || false,
            || validate_current_proof(&self.expected, &self.current, self.released),
            || Ok(self.current.root),
        )
    }

    fn mutate(
        &mut self,
        _fence: WindowsPhysicalInputFence,
        _point: Option<(i32, i32)>,
        events: &[Event],
        _cursor: Option<(i32, i32)>,
        cleanup_only: bool,
    ) -> Result<MutationReceipt, Reason> {
        // As in the production backend, even a previously completed delivery
        // cannot use the relaxed postcheck to authorize another mutation.
        validate_current_proof(&self.expected, &self.current, false)?;
        let receipt = MutationReceipt {
            requested: events.len() as u32,
            inserted: events.len() as u32,
            cursor_attempted: false,
            cursor_succeeded: false,
        };
        self.sent_events += events.len();
        self.current = self.after_delivery.clone();
        self.released = keyboard_release_completed(events, receipt, cleanup_only);
        Ok(receipt)
    }

    fn interrupted(&mut self) -> bool {
        false
    }
    fn elapsed_ms(&self) -> u64 {
        0
    }
    fn pause(&mut self, _milliseconds: u64) {}
}

#[rstest]
#[case(false)]
#[case(true)]
fn completion_resizing_after_balanced_text_does_not_report_unsent_input(
    #[case] changed_before_delivery: bool,
) {
    let (expected, current) = NativeCapturePopupProof::test_geometry_transition();
    let root = expected.root;
    let fence = WindowsPhysicalInputFence {
        target: UiaTarget {
            process_id: root.process_id,
            window_handle: root.window_handle,
        },
        native_instance: root.instance,
        native_window_bounds: root.bounds.unwrap(),
        native_visible_bounds: root.visible_bounds.unwrap(),
        window_dpi: root.dpi,
    };
    let mut backend = GeometryTransitionBackend {
        current: if changed_before_delivery {
            current.clone()
        } else {
            expected.clone()
        },
        expected,
        after_delivery: current,
        released: false,
        sent_events: 0,
    };
    let result = run_text(&mut backend, fence, "NoOp");
    if changed_before_delivery {
        assert!(result.unwrap_err().is_pre_dispatch());
        assert_eq!(backend.sent_events, 0);
    } else {
        assert!(backend.sent_events > 0);
        let outcome = result.expect("fully delivered text may resize the same proven completion");
        assert!(outcome.delivery_completed && outcome.post_dispatch_validated);
        let sent = backend.sent_events;
        assert!(
            backend
                .mutate(fence, None, &[Event::Key(65, false)], None, false)
                .is_err()
        );
        assert_eq!(backend.sent_events, sent);
    }
}

#[rstest]
#[case("valid", true)]
#[case("birth", false)]
#[case("bounds", false)]
#[case("dwm", false)]
#[case("dpi", false)]
#[case("foreground", false)]
#[case("hidden", false)]
#[case("minimized", false)]
fn a_composite_never_changes_the_exact_main_root_fence(#[case] case: &str, #[case] allowed: bool) {
    let (fence, mut state) = fixture();
    match case {
        "birth" => state.instance.process_creation_time_100ns += 1,
        "bounds" => state.bounds.as_mut().unwrap()[0] += 1,
        "dwm" => state.visible_bounds = None,
        "dpi" => state.dpi += 1,
        "foreground" => state.foreground = false,
        "hidden" => state.visible = false,
        "minimized" => state.minimized = true,
        _ => {}
    }
    assert_eq!(validate_root_fence(fence, state).is_ok(), allowed);
}

#[rstest]
#[case("valid", true)]
#[case("changed_proof", false)]
#[case("refused_proof", false)]
#[case("late_bounds", false)]
#[case("late_interrupt", false)]
#[case("late_desktop", false)]
fn dedicated_keyboard_checks_full_proof_then_final_state_before_any_mutation(
    #[case] case: &str,
    #[case] allowed: bool,
) {
    let (fence, mut state) = fixture();
    if case == "late_bounds" {
        state.bounds.as_mut().unwrap()[0] += 1;
    }
    let gates = Cell::new(0);
    let interrupts = Cell::new(0);
    let trace = RefCell::new(Vec::new());
    let result = check_capture_popup_fence(
        fence,
        || {
            trace.borrow_mut().push("gate");
            gates.set(gates.get() + 1);
            if case == "late_desktop" && gates.get() == 2 {
                Err(UiaError::OperationFailed("locked".into()))
            } else {
                Ok(())
            }
        },
        || {
            interrupts.set(interrupts.get() + 1);
            case == "late_interrupt" && interrupts.get() > 1
        },
        || {
            trace.borrow_mut().push("proof");
            if case == "refused_proof" {
                Err(Reason::TargetOccluded)
            } else if case == "changed_proof" {
                Err(Reason::InstanceChanged)
            } else {
                Ok(())
            }
        },
        || {
            trace.borrow_mut().push("final_state");
            Ok(state)
        },
    );
    let mutated = Cell::new(false);
    let result = final_physical_boundary(false, false, || result, || mutated.set(true));
    assert_eq!(result.is_ok(), allowed);
    assert_eq!(mutated.get(), allowed);
    if case == "refused_proof" {
        assert_eq!(result, Err(Reason::TargetOccluded));
        assert_eq!(*trace.borrow(), vec!["gate", "proof"]);
    }
    if allowed {
        assert_eq!(
            *trace.borrow(),
            vec!["gate", "proof", "gate", "final_state"]
        );
    }
}

#[rstest]
#[case("balanced", true)]
#[case("down", false)]
#[case("unmatched_release", false)]
#[case("unbalanced_pair", false)]
#[case("partial", false)]
#[case("owned_cleanup", true)]
#[case("empty", false)]
fn capture_ended_is_only_a_postcheck_after_completed_balanced_release(
    #[case] case: &str,
    #[case] allowed: bool,
) {
    let down = Event::Key(65, false);
    let up = Event::Key(65, true);
    let events = match case {
        "down" => vec![down],
        "unmatched_release" | "owned_cleanup" => vec![up],
        "unbalanced_pair" => vec![down, Event::Key(66, true)],
        "empty" => vec![],
        _ => vec![down, up],
    };
    let receipt = MutationReceipt {
        requested: events.len() as u32,
        inserted: if case == "partial" {
            1
        } else {
            events.len() as u32
        },
        cursor_attempted: false,
        cursor_succeeded: false,
    };
    assert_eq!(
        keyboard_release_completed(&events, receipt, case == "owned_cleanup"),
        allowed
    );
}
