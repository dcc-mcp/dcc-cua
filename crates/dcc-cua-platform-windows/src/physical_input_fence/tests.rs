use super::*;
use rstest::rstest;
use std::{cell::RefCell, rc::Rc};

fn fence() -> WindowsPhysicalInputFence {
    WindowsPhysicalInputFence {
        target: UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        native_instance: ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: 7,
            window_thread_id: 8,
            window_class_hash: 9,
            owner_window_handle: 0,
        },
        native_window_bounds: [80, 80, 1500, 1400],
        native_visible_bounds: [80, 80, 1500, 1400],
        window_dpi: 240,
    }
}

type RecordedMutation = (bool, Vec<Event>, Option<(i32, i32)>);

#[rstest]
#[case("uncovered", true)]
#[case("covered", false)]
#[case("unknown_proof", false)]
#[case("changed_instance", false)]
#[case("changed_bounds", false)]
fn partial_occlusion_click_requires_an_observed_and_current_native_target(
    #[case] scenario: &str,
    #[case] allowed: bool,
) {
    let mut backend = FakeBackend::new();
    backend.evidence.unobscured = false;
    backend.evidence.visibility_failure = Some(crate::VisibleWindowCaptureDiagnostic {
        reason: crate::VisibleWindowCaptureReason::RootOverlap,
        target_bounds: Some(fence().native_window_bounds),
        blocker_process_id: Some(123),
        blocker_window_handle: Some(456),
        blocker_bounds: Some([900, 900, 20, 20]),
        cloaked: Some(0),
        os_error: None,
        root_bounds_failure: None,
    });
    match scenario {
        "covered" => backend.point_visible = false,
        "unknown_proof" => backend.evidence.visibility_failure = None,
        "changed_instance" => backend.evidence.instance.process_creation_time_100ns += 1,
        "changed_bounds" => backend.state.bounds.as_mut().unwrap()[0] += 1,
        _ => {}
    }
    let result = run_click(
        &mut backend,
        fence(),
        (100, 100),
        1,
        WindowsPointerButton::Left,
        &[],
    );
    assert_eq!(result.is_ok(), allowed);
    if !allowed {
        assert!(backend.mutations.is_empty());
    }
    assert!(
        validate_physical_metadata(
            fence(),
            backend.evidence,
            backend.state,
            backend.desktop,
            None,
            true
        )
        .is_err()
    );
}

struct FakeBackend {
    rejection: Option<RejectedFenceDiagnostic>,
    evidence: ExactWindowPixelEvidence,
    state: ExactWindowNativeState,
    desktop: [i32; 4],
    locked: bool,
    point_visible: bool,
    check_count: usize,
    fail_check: Option<usize>,
    dwm_change_at_check: Option<usize>,
    foreground_loss_at_check: Option<usize>,
    partial_mutation: Option<(usize, u32)>,
    interrupt_after_mutation: bool,
    interrupt_after_mutations: Option<usize>,
    mutations: Vec<RecordedMutation>,
    trace: Rc<RefCell<Vec<&'static str>>>,
    clock: u64,
}

impl FakeBackend {
    fn new() -> Self {
        let fence = fence();
        Self {
            rejection: None,
            evidence: ExactWindowPixelEvidence {
                process_id: 42,
                window_handle: 99,
                bounds: fence.native_window_bounds,
                visible_bounds: fence.native_visible_bounds,
                dpi: 240,
                visible: true,
                minimized: false,
                unobscured: true,
                visibility_failure: None,
                instance: fence.native_instance,
            },
            state: ExactWindowNativeState {
                process_id: 42,
                window_handle: 99,
                bounds: Some(fence.native_window_bounds),
                visible_bounds: Some(fence.native_visible_bounds),
                dpi: 240,
                visible: true,
                minimized: false,
                maximized: false,
                foreground: true,
                instance: fence.native_instance,
            },
            desktop: [0, 0, 3840, 2400],
            locked: false,
            point_visible: true,
            check_count: 0,
            fail_check: None,
            dwm_change_at_check: None,
            foreground_loss_at_check: None,
            partial_mutation: None,
            interrupt_after_mutation: false,
            interrupt_after_mutations: None,
            mutations: Vec::new(),
            trace: Rc::new(RefCell::new(Vec::new())),
            clock: 0,
        }
    }
}

impl InputBackend for FakeBackend {
    fn rejection_diagnostic(&self) -> Option<RejectedFenceDiagnostic> {
        self.rejection.clone()
    }

    fn check(
        &mut self,
        fence: WindowsPhysicalInputFence,
        point: Option<(i32, i32)>,
    ) -> Result<(), Reason> {
        self.trace.borrow_mut().push("check");
        self.check_count += 1;
        if self.interrupted() {
            return Err(Reason::Interrupted);
        }
        if self.dwm_change_at_check == Some(self.check_count) {
            self.evidence.visible_bounds[0] += 1;
        }
        if self.foreground_loss_at_check == Some(self.check_count) {
            self.state.foreground = false;
        }
        if self.locked {
            return Err(Reason::DesktopUnavailable);
        }
        if self.fail_check == Some(self.check_count) {
            return Err(Reason::InstanceChanged);
        }
        validate_physical_metadata(
            fence,
            self.evidence,
            self.state,
            self.desktop,
            point,
            self.point_visible,
        )
    }
    fn mutate(
        &mut self,
        fence: WindowsPhysicalInputFence,
        point: Option<(i32, i32)>,
        events: &[Event],
        cursor: Option<(i32, i32)>,
        cleanup_only: bool,
    ) -> Result<MutationReceipt, Reason> {
        let trace = Rc::clone(&self.trace);
        let requested = events.len() as u32;
        let inserted = self
            .partial_mutation
            .filter(|(index, _)| *index == self.mutations.len())
            .map_or(requested, |(_, inserted)| inserted);
        let result = final_physical_boundary(
            cleanup_only,
            cursor.is_none() && !events.is_empty() && events.iter().all(|event| event.is_release()),
            || self.check(fence, point),
            || {
                if !cleanup_only {
                    assert_eq!(
                        trace.borrow().last(),
                        Some(&"check"),
                        "the same final boundary must directly precede every mutation"
                    );
                }
                trace.borrow_mut().push(if cleanup_only {
                    "release_cleanup"
                } else if cursor.is_some() {
                    "SetCursorPos"
                } else {
                    "SendInput"
                });
                MutationReceipt {
                    requested,
                    inserted,
                    cursor_attempted: cursor.is_some(),
                    cursor_succeeded: cursor.is_some(),
                }
            },
        );
        if result.is_ok() {
            self.mutations.push((cleanup_only, events.to_vec(), cursor));
        }
        result
    }
    fn interrupted(&mut self) -> bool {
        (self.interrupt_after_mutation && !self.mutations.is_empty())
            || self
                .interrupt_after_mutations
                .is_some_and(|count| self.mutations.len() >= count)
    }
    fn elapsed_ms(&self) -> u64 {
        self.clock
    }
    fn pause(&mut self, milliseconds: u64) {
        self.trace.borrow_mut().push("pause");
        self.clock += milliseconds;
    }
}

#[rstest]
fn physical_drag_preflights_the_whole_path_before_button_down_and_fences_each_move() {
    let mut backend = FakeBackend::new();
    let points = [(100, 100), (120, 110), (140, 120)];
    let outcome = run_drag(
        &mut backend,
        fence(),
        &points,
        40,
        WindowsPointerButton::Left,
    )
    .unwrap();
    assert_eq!(outcome.operation, WindowsGuardedInputOperation::Drag);
    assert!(outcome.delivery_completed && outcome.post_dispatch_validated);
    assert_eq!(outcome.requested_events, 4);
    assert_eq!(outcome.inserted_events, 4);
    assert_eq!(outcome.cleanup_requested_events, 0);
    assert_eq!(backend.mutations[0], (false, vec![], Some(points[0])));
    assert_eq!(
        backend.mutations[1].1,
        vec![Event::Mouse(WindowsPointerButton::Left, false)]
    );
    assert_eq!(backend.mutations[2].1, vec![Event::Move(120, 110)]);
    assert_eq!(backend.mutations[3].1, vec![Event::Move(140, 120)]);
    assert_eq!(
        backend.mutations[4].1,
        vec![Event::Mouse(WindowsPointerButton::Left, true)]
    );
    assert!(
        backend
            .trace
            .borrow()
            .iter()
            .take(points.len())
            .all(|event| *event == "check")
    );
    assert_eq!(backend.clock, 40);
}

#[rstest]
#[case("later_point")]
#[case("outside")]
#[case("too_many")]
#[case("same_point")]
#[case("duration")]
fn physical_drag_invalid_or_unproven_path_never_moves_or_presses(#[case] variant: &str) {
    let mut backend = FakeBackend::new();
    let mut points = vec![(100, 100), (140, 120)];
    let mut duration = 40;
    match variant {
        "later_point" => backend.fail_check = Some(2),
        "outside" => points[1] = (3000, 120),
        "too_many" => points = vec![(100, 100); 202],
        "same_point" => points[1] = points[0],
        "duration" => duration = 10_001,
        _ => unreachable!(),
    }
    let error = run_drag(
        &mut backend,
        fence(),
        &points,
        duration,
        WindowsPointerButton::Left,
    )
    .unwrap_err();
    assert!(error.is_pre_dispatch());
    assert!(backend.mutations.is_empty());
}

#[rstest]
#[case("instance")]
#[case("interrupt")]
#[case("partial_move")]
#[case("partial_up")]
#[case("dwm_motion")]
#[case("foreground_loss")]
fn physical_drag_mid_hold_failure_sends_only_one_owned_button_release(#[case] variant: &str) {
    let mut backend = FakeBackend::new();
    match variant {
        // Two path preflights, cursor move, button-down, then first motion fence.
        "instance" => backend.fail_check = Some(5),
        "interrupt" => backend.interrupt_after_mutations = Some(2),
        "partial_move" => backend.partial_mutation = Some((2, 0)),
        "partial_up" => backend.partial_mutation = Some((3, 0)),
        "dwm_motion" => backend.dwm_change_at_check = Some(5),
        "foreground_loss" => backend.foreground_loss_at_check = Some(5),
        _ => unreachable!(),
    }
    let error = run_drag(
        &mut backend,
        fence(),
        &[(100, 100), (140, 120)],
        1,
        WindowsPointerButton::Left,
    )
    .unwrap_err();
    assert!(!error.outcome.delivery_completed);
    assert_eq!(error.outcome.cleanup_requested_events, 1);
    assert_eq!(error.outcome.cleanup_inserted_events, 1);
    assert_eq!(
        backend.mutations.last().unwrap(),
        &(
            true,
            vec![Event::Mouse(WindowsPointerButton::Left, true)],
            None
        )
    );
    assert_eq!(
        backend
            .mutations
            .iter()
            .filter(|(cleanup, _, _)| *cleanup)
            .count(),
        1
    );
}

#[rstest]
fn physical_drag_failed_button_down_does_not_release_an_unowned_button() {
    let mut backend = FakeBackend::new();
    backend.partial_mutation = Some((1, 0));
    let error = run_drag(
        &mut backend,
        fence(),
        &[(100, 100), (140, 120)],
        1,
        WindowsPointerButton::Left,
    )
    .unwrap_err();
    assert_eq!(error.outcome.cleanup_requested_events, 0);
    assert_eq!(backend.mutations.len(), 2);
}

#[rstest]
fn physical_drag_final_identity_drift_is_not_reported_as_completed() {
    let mut backend = FakeBackend::new();
    backend.fail_check = Some(8);
    let error = run_drag(
        &mut backend,
        fence(),
        &[(100, 100), (140, 120)],
        1,
        WindowsPointerButton::Left,
    )
    .unwrap_err();
    assert_eq!(error.reason, Reason::InstanceChanged);
    assert_eq!(error.outcome.inserted_events, 3);
    assert!(!error.outcome.delivery_completed);
    assert_eq!(error.outcome.cleanup_requested_events, 0);
}

#[rstest]
#[case("creation")]
#[case("thread")]
#[case("class")]
#[case("owner")]
#[case("pid")]
#[case("hwnd")]
#[case("bounds")]
#[case("dwm_bounds")]
#[case("dpi")]
#[case("hidden")]
#[case("minimized")]
#[case("foreground")]
#[case("occluded")]
#[case("locked")]
#[case("outside_desktop")]
#[case("point")]
#[case("late_instance")]
#[case("late_bounds")]
#[case("late_dpi")]
#[case("late_dwm_bounds")]
#[case("unavailable_dwm_bounds")]
fn physical_input_fence_refuses_wrong_metadata_before_any_dispatch(#[case] scenario: &str) {
    let mut backend = FakeBackend::new();
    let mut point = (100, 100);
    match scenario {
        "creation" => backend.evidence.instance.process_creation_time_100ns += 1,
        "thread" => backend.evidence.instance.window_thread_id += 1,
        "class" => backend.evidence.instance.window_class_hash += 1,
        "owner" => backend.evidence.instance.owner_window_handle += 1,
        "pid" => backend.evidence.process_id += 1,
        "hwnd" => backend.evidence.window_handle += 1,
        "bounds" => backend.evidence.bounds[0] += 1,
        "dwm_bounds" => backend.evidence.visible_bounds[0] += 1,
        "dpi" => backend.evidence.dpi += 1,
        "hidden" => backend.evidence.visible = false,
        "minimized" => backend.state.minimized = true,
        "foreground" => backend.state.foreground = false,
        "occluded" => backend.evidence.unobscured = false,
        "locked" => backend.locked = true,
        "outside_desktop" => backend.desktop[2] = 100,
        "point" => point = (1, 1),
        "late_instance" => backend.state.instance.process_creation_time_100ns += 1,
        "late_bounds" => backend.state.bounds = Some([81, 80, 1500, 1400]),
        "late_dpi" => backend.state.dpi += 1,
        "late_dwm_bounds" => backend.state.visible_bounds.as_mut().unwrap()[0] += 1,
        "unavailable_dwm_bounds" => backend.state.visible_bounds = None,
        _ => unreachable!(),
    }
    let error = run_click(
        &mut backend,
        fence(),
        point,
        1,
        WindowsPointerButton::Left,
        &[],
    )
    .unwrap_err();
    assert!(error.is_pre_dispatch());
    assert_eq!(error.outcome.inserted_events, 0);
    assert!(backend.mutations.is_empty());
}

#[rstest]
fn physical_input_fence_click_checks_each_actual_mutation_and_reports_delivery_only() {
    let mut backend = FakeBackend::new();
    let outcome = run_click(
        &mut backend,
        fence(),
        (100, 100),
        2,
        WindowsPointerButton::Left,
        &["Control".into()],
    )
    .unwrap();
    assert_eq!(
        *backend.trace.borrow(),
        vec![
            "check",
            "SetCursorPos",
            "check",
            "SendInput",
            "check",
            "SendInput",
            "pause",
            "check",
            "SendInput",
            "release_cleanup",
            "check"
        ]
    );
    assert!(outcome.delivery_completed && outcome.post_dispatch_validated);
    assert!(outcome.fresh_observation_required);
    assert_eq!(outcome.requested_events, 7);
    assert_eq!(outcome.cleanup_requested_events, 1);
}

#[rstest]
#[case("click")]
#[case("double_click")]
#[case("right_click")]
#[case("toggle")]
#[case("keypress")]
#[case("keyboard_shortcut")]
#[case("type")]
#[case("type_chars")]
fn physical_input_fence_dwm_motion_refuses_every_supported_dispatch_family(#[case] action: &str) {
    let mut backend = FakeBackend::new();
    backend.evidence.visible_bounds[0] += 1;
    let error = match action {
        "click" | "double_click" | "right_click" | "toggle" => run_click(
            &mut backend,
            fence(),
            (100, 100),
            if action == "double_click" { 2 } else { 1 },
            if action == "right_click" {
                WindowsPointerButton::Right
            } else {
                WindowsPointerButton::Left
            },
            &[],
        ),
        "keypress" => run_keypress(&mut backend, fence(), &["A".into()], None),
        "keyboard_shortcut" => run_keypress(
            &mut backend,
            fence(),
            &["Control".into(), "F6".into()],
            None,
        ),
        "type" | "type_chars" => run_text(&mut backend, fence(), "aurora"),
        _ => unreachable!(),
    }
    .unwrap_err();
    assert_eq!(error.reason, Reason::BoundsChanged);
    assert!(error.is_pre_dispatch());
    assert!(backend.mutations.is_empty());
}

#[rstest]
fn physical_input_fence_dwm_motion_after_keyhold_sends_only_owned_release() {
    let mut backend = FakeBackend::new();
    backend.dwm_change_at_check = Some(2);
    let error = run_keypress(&mut backend, fence(), &["A".into()], Some(20)).unwrap_err();
    assert_eq!(error.reason, Reason::BoundsChanged);
    assert_eq!(backend.mutations.len(), 2);
    assert_eq!(backend.mutations[0].1, vec![Event::Key(0x41, false)]);
    assert_eq!(
        backend.mutations[1],
        (true, vec![Event::Key(0x41, true)], None)
    );
    assert_eq!(error.outcome.cleanup_inserted_events, 1);
}

#[rstest]
#[case(false)]
#[case(true)]
fn physical_input_fence_final_dwm_read_after_slow_proof_refuses_motion_or_missing_metadata(
    #[case] unavailable: bool,
) {
    let backend = FakeBackend::new();
    let trace = RefCell::new(Vec::new());
    let dispatched = std::cell::Cell::new(false);
    let result = final_physical_boundary(
        false,
        false,
        || {
            check_physical_fence(
                fence(),
                None,
                || Ok(()),
                || false,
                || {
                    trace.borrow_mut().push("slow occlusion proof");
                    Ok((backend.evidence, true))
                },
                || backend.desktop,
                || {
                    trace.borrow_mut().push("final actual DWM read");
                    let mut final_state = backend.state;
                    if unavailable {
                        final_state.visible_bounds = None;
                    } else {
                        final_state.visible_bounds.as_mut().unwrap()[0] += 1;
                    }
                    Ok(final_state)
                },
            )
        },
        || {
            dispatched.set(true);
        },
    );
    assert_eq!(result, Err(Reason::BoundsChanged));
    assert!(!dispatched.get());
    assert_eq!(
        *trace.borrow(),
        ["slow occlusion proof", "final actual DWM read"]
    );
}

#[rstest]
fn physical_input_fence_changed_after_wait_sends_only_owned_modifier_cleanup() {
    let mut backend = FakeBackend::new();
    backend.fail_check = Some(4);
    let error = run_click(
        &mut backend,
        fence(),
        (100, 100),
        2,
        WindowsPointerButton::Left,
        &["Control".into()],
    )
    .unwrap_err();
    assert!(!error.is_pre_dispatch());
    assert_eq!(error.reason, Reason::InstanceChanged);
    assert_eq!(backend.mutations.len(), 4);
    assert_eq!(
        backend.mutations.last().unwrap().1,
        vec![Event::Key(0x11, true)]
    );
    assert!(backend.mutations.last().unwrap().0);
    assert_eq!(error.outcome.cleanup_inserted_events, 1);
}

#[rstest]
#[case(false)]
#[case(true)]
fn physical_input_fence_partial_key_or_unicode_batch_releases_only_inserted_down(
    #[case] text: bool,
) {
    let mut backend = FakeBackend::new();
    backend.partial_mutation = Some((0, 1));
    let error = if text {
        run_text(&mut backend, fence(), "Hi")
    } else {
        run_keypress(&mut backend, fence(), &["Control".into(), "A".into()], None)
    }
    .unwrap_err();
    assert_eq!(error.reason, Reason::InjectionIncomplete);
    assert_eq!(error.outcome.inserted_events, 1);
    assert_eq!(error.outcome.cleanup_requested_events, 1);
    assert_eq!(backend.mutations.len(), 2);
    assert!(backend.mutations[1].0);
    assert!(
        backend.mutations[1]
            .1
            .iter()
            .all(|event| event.is_release())
    );
    assert_eq!(
        backend.mutations[1].1,
        vec![if text {
            Event::Unicode('H' as u16, true)
        } else {
            Event::Key(0x11, true)
        }]
    );
}

#[rstest]
#[case(false)]
#[case(true)]
fn physical_input_fence_keyhold_failure_cleans_up_without_new_input(#[case] interrupt: bool) {
    let mut backend = FakeBackend::new();
    if interrupt {
        backend.interrupt_after_mutation = true;
    } else {
        backend.fail_check = Some(2);
    }
    let error = run_keypress(
        &mut backend,
        fence(),
        &["Control".into(), "A".into()],
        Some(20),
    )
    .unwrap_err();
    assert_eq!(
        error.reason,
        if interrupt {
            Reason::Interrupted
        } else {
            Reason::InstanceChanged
        }
    );
    assert_eq!(backend.mutations.len(), 2);
    assert_eq!(
        backend.mutations[1].1,
        vec![Event::Key(0x41, true), Event::Key(0x11, true)]
    );
    assert!(backend.mutations[1].0);
}

#[rstest]
fn physical_input_fence_keyhold_and_unicode_text_use_local_balanced_events() {
    let mut backend = FakeBackend::new();
    let outcome = run_keypress(&mut backend, fence(), &["W".into()], Some(35)).unwrap();
    assert_eq!(backend.clock, 35);
    assert_eq!(outcome.inserted_events, 1);
    assert_eq!(outcome.cleanup_inserted_events, 1);
    assert!(outcome.delivery_completed);
    assert_eq!(
        text_events("a\r\n😀").unwrap(),
        vec![
            Event::Unicode(97, false),
            Event::Unicode(97, true),
            Event::Key(VK_RETURN, false),
            Event::Key(VK_RETURN, true),
            Event::Unicode(0xd83d, false),
            Event::Unicode(0xd83d, true),
            Event::Unicode(0xde00, false),
            Event::Unicode(0xde00, true)
        ]
    );
}

#[rstest]
#[case("complete")]
#[case("interrupt")]
#[case("instance")]
#[case("foreground")]
#[case("occluded")]
fn physical_input_fence_shift_movement_releases_every_owned_key(#[case] scenario: &str) {
    let mut backend = FakeBackend::new();
    match scenario {
        "interrupt" => backend.interrupt_after_mutation = true,
        "instance" => backend.fail_check = Some(2),
        "foreground" => backend.foreground_loss_at_check = Some(2),
        "occluded" => backend.evidence.unobscured = false,
        _ => {}
    }
    let result = run_keypress(
        &mut backend,
        fence(),
        &["Shift".into(), "W".into(), "A".into()],
        Some(35),
    );
    assert_eq!(result.is_ok(), scenario == "complete");
    if scenario == "occluded" {
        assert!(backend.mutations.is_empty());
        return;
    }
    assert_eq!(
        backend.mutations[0].1,
        vec![
            Event::Key(0x10, false),
            Event::Key(0x57, false),
            Event::Key(0x41, false),
        ]
    );
    let cleanup = backend.mutations.last().unwrap();
    assert!(cleanup.0);
    assert_eq!(
        cleanup.1,
        vec![
            Event::Key(0x41, true),
            Event::Key(0x57, true),
            Event::Key(0x10, true),
        ]
    );
    if scenario == "complete" {
        assert_eq!(backend.clock, 35);
    }
}

#[rstest]
#[case(1600)]
#[case(2400)]
fn physical_input_fence_long_shift_hold_keeps_twenty_ms_checks_and_key_release(
    #[case] duration: u64,
) {
    let mut backend = FakeBackend::new();
    let outcome = run_keypress(
        &mut backend,
        fence(),
        &["Shift".into(), "W".into()],
        Some(duration),
    )
    .unwrap();
    assert!(outcome.delivery_completed);
    assert_eq!(backend.clock, duration);
    assert!(backend.check_count >= duration as usize / 20);
    assert_eq!(
        backend.mutations[0].1,
        vec![Event::Key(0x10, false), Event::Key(0x57, false)]
    );
    assert_eq!(
        backend.mutations.last().unwrap().1,
        vec![Event::Key(0x57, true), Event::Key(0x10, true)]
    );
}

#[rstest]
fn physical_input_fence_post_dispatch_drift_is_not_reported_as_completed() {
    let mut backend = FakeBackend::new();
    backend.fail_check = Some(2);
    let error = run_text(&mut backend, fence(), "a").unwrap_err();
    assert!(error.outcome.mutation_attempted);
    assert_eq!(error.outcome.inserted_events, 2);
    assert!(!error.outcome.delivery_completed || error.outcome.post_dispatch_validated);
    assert!(!error.outcome.post_dispatch_validated);
    assert_eq!(backend.mutations.len(), 1);
}

#[rstest]
fn physical_input_fence_cleanup_cannot_be_used_to_send_down_move_or_text() {
    let mut backend = FakeBackend::new();
    for event in [
        Event::Key(65, false),
        Event::Unicode(65, false),
        Event::Mouse(WindowsPointerButton::Left, false),
        Event::Move(100, 100),
    ] {
        assert!(matches!(
            backend.mutate(fence(), None, &[event], None, true),
            Err(Reason::InvalidRequest)
        ));
    }
    assert!(matches!(
        backend.mutate(fence(), None, &[], Some((100, 100)), true),
        Err(Reason::InvalidRequest)
    ));
    assert!(backend.mutations.is_empty());
}

#[rstest]
fn physical_input_fence_invalid_requests_cannot_enter_native_boundaries() {
    let mut backend = FakeBackend::new();
    assert!(
        run_click(
            &mut backend,
            fence(),
            (100, 100),
            3,
            WindowsPointerButton::Left,
            &[]
        )
        .is_err()
    );
    assert!(
        run_click(
            &mut backend,
            fence(),
            (100, 100),
            1,
            WindowsPointerButton::Left,
            &["unsupported".into()]
        )
        .is_err()
    );
    assert!(run_keypress(&mut backend, fence(), &["W".into()], Some(60_001)).is_err());
    assert!(
        run_keypress(
            &mut backend,
            fence(),
            &["Control".into(), "ctrl".into()],
            None
        )
        .is_err()
    );
    assert!(run_text(&mut backend, fence(), &"a".repeat(4097)).is_err());
    assert!(backend.mutations.is_empty());
    assert_eq!(backend.check_count, 0);
}

#[rstest]
#[case(false)]
#[case(true)]
fn physical_input_fence_actual_query_sequence_rechecks_lock_after_proof_before_dispatch(
    #[case] lock_after_proof: bool,
) {
    let backend = FakeBackend::new();
    let trace = RefCell::new(Vec::new());
    let gates = std::cell::Cell::new(0);
    let result = final_physical_boundary(
        false,
        false,
        || {
            check_physical_fence(
                fence(),
                Some((100, 100)),
                || {
                    trace.borrow_mut().push("gate");
                    gates.set(gates.get() + 1);
                    if lock_after_proof && gates.get() == 2 {
                        Err(UiaError::OperationFailed("locked".into()))
                    } else {
                        Ok(())
                    }
                },
                || {
                    trace.borrow_mut().push("interrupt");
                    false
                },
                || {
                    trace.borrow_mut().push("proof");
                    Ok((backend.evidence, true))
                },
                || {
                    trace.borrow_mut().push("desktop");
                    backend.desktop
                },
                || {
                    trace.borrow_mut().push("final_native_state");
                    Ok(backend.state)
                },
            )
        },
        || trace.borrow_mut().push("SendInput"),
    );
    if lock_after_proof {
        assert_eq!(result, Err(Reason::DesktopUnavailable));
        assert_eq!(*trace.borrow(), vec!["interrupt", "gate", "proof", "gate"]);
    } else {
        assert_eq!(result, Ok(()));
        assert_eq!(
            *trace.borrow(),
            vec![
                "interrupt",
                "gate",
                "proof",
                "gate",
                "desktop",
                "final_native_state",
                "interrupt",
                "SendInput"
            ]
        );
    }
}

#[rstest]
#[case(false)]
#[case(true)]
fn fence_diagnostic_is_reported_only_on_the_rejected_attempt(#[case] accepted: bool) {
    let mut backend = FakeBackend::new();
    backend.point_visible = accepted;
    backend.rejection = Some(RejectedFenceDiagnostic::new(
        fence(),
        Some((100, 100)),
        backend.evidence,
    ));
    let result = run_click(
        &mut backend,
        fence(),
        (100, 100),
        1,
        WindowsPointerButton::Left,
        &[],
    );
    if accepted {
        assert!(
            serde_json::to_value(result.unwrap())
                .unwrap()
                .get("native_input_fence")
                .is_none()
        );
    } else {
        let error = result.unwrap_err();
        assert_eq!(error.reason, Reason::TargetOccluded);
        assert!(!error.outcome.mutation_attempted);
        assert!(backend.mutations.is_empty());
        let diagnostic = error.native_input_fence.unwrap();
        assert!(!diagnostic.authorizes_input);
        assert_eq!(diagnostic.point, Some([100, 100]));
    }
}
