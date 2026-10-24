use super::*;
use rstest::rstest;

fn state(handle: u64, bounds: [i32; 4]) -> ExactWindowNativeState {
    ExactWindowNativeState {
        process_id: 42,
        window_handle: handle,
        bounds: Some(bounds),
        visible_bounds: Some(bounds),
        dpi: 96,
        visible: true,
        minimized: false,
        maximized: false,
        foreground: handle == 100,
        instance: ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: 7,
            window_thread_id: 8,
            window_class_hash: handle + 1,
            owner_window_handle: 0,
        },
    }
}

fn entry(state: ExactWindowNativeState) -> RootProofTraceEntry {
    RootProofTraceEntry {
        window_handle: state.window_handle,
        process_id: state.process_id,
        thread_id: state.instance.window_thread_id,
        root_window_handle: state.window_handle,
        owner_window_handle: state.instance.owner_window_handle,
        extended_style: 0,
        visible: true,
        minimized: false,
        dpi: state.dpi,
        win32_bounds: state.bounds,
        win32_bounds_error: None,
        cloaked: Some(0),
        cloak_query_error: None,
        dwm_bounds: state.visible_bounds,
        dwm_bounds_error: None,
        decision: if state.window_handle == 100 {
            RootProofDecision::TargetReached
        } else {
            RootProofDecision::Overlap
        },
    }
}

fn sample() -> Sample {
    let root = state(100, [100, 100, 800, 600]);
    let overlays = vec![
        state(200, [110, 110, 100, 30]),
        state(300, [110, 135, 180, 120]),
    ];
    Sample {
        root,
        root_is_root: true,
        root_enabled: true,
        focus: FocusState {
            active: 100,
            foreground: 100,
            focus: 101,
            focus_root: 100,
            focus_instance: state(101, [101, 101, 10, 10]).instance,
            capture: 200,
            flags: 0,
            enabled: true,
            visible: true,
        },
        roots: overlays
            .iter()
            .copied()
            .map(entry)
            .chain([entry(root)])
            .collect(),
        overlays: overlays
            .into_iter()
            .map(|state| PopupState {
                root: state.window_handle,
                state,
                style: WS_POPUP | WS_VISIBLE,
                enabled: true,
                owner_is_local_or_absent: true,
            })
            .collect(),
        desktop: [0, 0, 1920, 1080],
    }
}

#[rstest]
#[case(false)]
#[case(true)]
fn completed_sample_can_observe_only_original_capture_release(#[case] root_focus: bool) {
    let mut before = sample();
    let expected = validate_sample(42, 100, &[200, 300], &before).unwrap();
    if root_focus {
        before.focus.focus = before.root.window_handle;
        before.focus.focus_instance = before.root.instance;
    }
    let mut after = before.focus;
    after.capture = 0;
    assert!(!sample_focus_reread_matches(&before, after, None));
    assert!(sample_focus_reread_matches(&before, after, Some(&expected)));
    // This permits only the existing read-only pending checks. It cannot mint
    // a complete observation after capture has ended.
    let mut ended = before.clone();
    ended.focus = after;
    assert!(validate_sample(42, 100, &[200, 300], &ended).is_err());
}

#[rstest]
#[case("root_birth")]
#[case("root_class")]
#[case("root_thread")]
#[case("root_owner")]
#[case("root_pid")]
#[case("root_handle")]
#[case("root_bounds")]
#[case("root_visible_bounds")]
#[case("root_dpi")]
#[case("root_foreground")]
#[case("root_hidden")]
#[case("root_disabled")]
#[case("not_root")]
#[case("desktop")]
#[case("unknown_capture")]
#[case("capture_regained")]
#[case("focus_changed")]
#[case("focus_instance")]
#[case("old_focus_replaced")]
#[case("old_focus_identity")]
#[case("active")]
#[case("foreground")]
#[case("focus_root")]
#[case("flags")]
#[case("disabled")]
#[case("hidden")]
fn completed_sample_refuses_every_other_reread_change(#[case] case: &str) {
    let mut before = sample();
    let expected = validate_sample(42, 100, &[200, 300], &before).unwrap();
    before.focus.focus = before.root.window_handle;
    before.focus.focus_instance = before.root.instance;
    let mut after = before.focus;
    after.capture = 0;
    assert!(sample_focus_reread_matches(&before, after, Some(&expected)));
    match case {
        "root_birth" => before.root.instance.process_creation_time_100ns += 1,
        "root_class" => before.root.instance.window_class_hash += 1,
        "root_thread" => before.root.instance.window_thread_id += 1,
        "root_owner" => before.root.instance.owner_window_handle += 1,
        "root_pid" => before.root.process_id += 1,
        "root_handle" => before.root.window_handle += 1,
        "root_bounds" => before.root.bounds.as_mut().unwrap()[0] += 1,
        "root_visible_bounds" => before.root.visible_bounds.as_mut().unwrap()[0] += 1,
        "root_dpi" => before.root.dpi += 1,
        "root_foreground" => before.root.foreground = false,
        "root_hidden" => before.root.visible = false,
        "root_disabled" => before.root_enabled = false,
        "not_root" => before.root_is_root = false,
        "desktop" => before.desktop[0] += 1,
        "unknown_capture" => before.focus.capture += 1,
        "capture_regained" => {
            before.focus.capture = 0;
            after.capture = expected.capture.window_handle;
        }
        "focus_changed" => after.focus += 1,
        "focus_instance" => after.focus_instance.window_class_hash += 1,
        "old_focus_replaced" => {
            before.focus.focus += 1;
            after.focus = before.focus.focus;
        }
        "old_focus_identity" => {
            before.focus.focus_instance.window_class_hash += 1;
            after.focus_instance = before.focus.focus_instance;
        }
        "active" => after.active += 1,
        "foreground" => after.foreground += 1,
        "focus_root" => after.focus_root += 1,
        "flags" => after.flags += 1,
        "disabled" => after.enabled = false,
        "hidden" => after.visible = false,
        _ => unreachable!(),
    }
    assert!(!sample_focus_reread_matches(
        &before,
        after,
        Some(&expected)
    ));
    assert!(!sample_focus_reread_matches(&before, after, None));
}

// Mint both opaque proofs through the real complete sample validator so the
// final-boundary regression does not invent a different proof contract.
impl NativeCapturePopupProof {
    pub(crate) fn test_geometry_transition() -> (Self, Self) {
        let before = sample();
        let expected = validate_sample(42, 100, &[200, 300], &before).unwrap();
        let mut after = sample();
        for popup in &mut after.overlays {
            popup.state.bounds.as_mut().unwrap()[2] += 10;
            popup.state.visible_bounds = popup.state.bounds;
        }
        for (index, popup) in after.overlays.iter().enumerate() {
            after.roots[index] = entry(popup.state);
        }
        let current = validate_sample(42, 100, &[200, 300], &after).unwrap();
        assert_ne!(expected, current);
        (expected, current)
    }
}

#[rstest]
#[case("hidden_capture", true)]
#[case("remaining_shadow", true)]
#[case("gone", true)]
#[case("foreign", false)]
#[case("new_same_pid", false)]
#[case("unknown_cloaking", false)]
#[case("unknown_bounds", false)]
#[case("changed_birth", false)]
#[case("changed_class", false)]
#[case("changed_thread", false)]
#[case("changed_owner", false)]
#[case("changed_dpi", false)]
#[case("reordered", false)]
#[case("focus", false)]
#[case("focus_birth", false)]
#[case("foreground", false)]
#[case("gui_flags", false)]
#[case("unknown_capture", false)]
#[case("root_birth", false)]
#[case("root_frame", false)]
#[case("root_dpi", false)]
#[case("desktop", false)]
fn completed_teardown_waits_only_for_the_original_exact_episode(
    #[case] case: &str,
    #[case] allowed: bool,
) {
    let mut current = sample();
    let expected = validate_sample(42, 100, &[200, 300], &current).unwrap();
    match case {
        "hidden_capture" => {
            current.overlays[0].state.visible = false;
            current.overlays[0].style &= !WS_VISIBLE;
            current.overlays[0].enabled = false;
            current.roots.remove(0);
        }
        "remaining_shadow" => {
            current.focus.capture = 0;
            current.overlays.remove(0);
            current.roots.remove(0);
        }
        "gone" => {
            current.focus.capture = 0;
            current.overlays.clear();
            current.roots.drain(..2);
        }
        "foreign" | "new_same_pid" => {
            let mut blocker = state(400, [120, 120, 10, 10]);
            if case == "foreign" {
                blocker.process_id += 1;
            }
            current.roots.insert(0, entry(blocker));
        }
        "unknown_cloaking" => current.roots[0].cloaked = None,
        "unknown_bounds" => current.roots[0].dwm_bounds = None,
        "changed_birth" => {
            current.overlays[0]
                .state
                .instance
                .process_creation_time_100ns += 1
        }
        "changed_class" => current.overlays[0].state.instance.window_class_hash += 1,
        "changed_thread" => current.overlays[0].state.instance.window_thread_id += 1,
        "changed_owner" => current.overlays[0].state.instance.owner_window_handle += 1,
        "changed_dpi" => current.overlays[0].state.dpi += 1,
        "reordered" => current.roots.swap(0, 1),
        "focus" => current.focus.focus += 1,
        "focus_birth" => current.focus.focus_instance.process_creation_time_100ns += 1,
        "foreground" => current.focus.foreground = 300,
        "gui_flags" => current.focus.flags = GUI_CARETBLINKING,
        "unknown_capture" => current.focus.capture = 300,
        "root_birth" => current.root.instance.process_creation_time_100ns += 1,
        "root_frame" => current.root.bounds.as_mut().unwrap()[0] += 1,
        "root_dpi" => current.root.dpi += 1,
        "desktop" => current.desktop[2] += 1,
        _ => unreachable!(),
    }
    assert_eq!(completed_teardown_unchanged(&expected, &current), allowed);
}

#[rstest]
#[case("original_capture")]
#[case("remaining_shadow")]
#[case("gone")]
fn completed_teardown_returns_focus_only_to_the_original_proven_root(#[case] stage: &str) {
    let before = sample();
    let expected = validate_sample(42, 100, &[200, 300], &before).unwrap();
    let mut current = before.clone();
    current.focus.focus = before.root.window_handle;
    current.focus.focus_instance = before.root.instance;
    match stage {
        "original_capture" => {
            current.overlays.pop();
            current.roots.remove(1);
        }
        "remaining_shadow" => {
            current.focus.capture = 0;
            current.overlays.remove(0);
            current.roots.remove(0);
        }
        "gone" => {
            current.focus.capture = 0;
            current.overlays.clear();
            current.roots.drain(..2);
        }
        _ => unreachable!(),
    }
    assert!(completed_teardown_unchanged(&expected, &current));
}

#[rstest]
#[case("another_child")]
#[case("old_child_instance")]
#[case("focus_birth")]
#[case("focus_thread")]
#[case("focus_class")]
#[case("focus_owner")]
#[case("active")]
#[case("foreground")]
#[case("focus_root")]
#[case("flags")]
#[case("disabled")]
#[case("hidden")]
#[case("capture")]
#[case("root_birth")]
#[case("root_frame")]
#[case("root_dpi")]
#[case("desktop")]
#[case("popup_birth")]
#[case("new_same_process_occluder")]
#[case("foreign_occluder")]
#[case("reordered")]
fn returned_root_focus_never_changes_the_completed_episode_contract(#[case] changed: &str) {
    let before = sample();
    let expected = validate_sample(42, 100, &[200, 300], &before).unwrap();
    let mut current = before.clone();
    current.focus.focus = before.root.window_handle;
    current.focus.focus_instance = before.root.instance;
    match changed {
        "another_child" => {
            current.focus.focus = 102;
            current.focus.focus_instance = state(102, [101, 101, 10, 10]).instance;
        }
        "old_child_instance" => current.focus.focus_instance = before.focus.focus_instance,
        "focus_birth" => current.focus.focus_instance.process_creation_time_100ns += 1,
        "focus_thread" => current.focus.focus_instance.window_thread_id += 1,
        "focus_class" => current.focus.focus_instance.window_class_hash += 1,
        "focus_owner" => current.focus.focus_instance.owner_window_handle += 1,
        "active" => current.focus.active = 300,
        "foreground" => current.focus.foreground = 300,
        "focus_root" => current.focus.focus_root = 300,
        "flags" => current.focus.flags = GUI_CARETBLINKING,
        "disabled" => current.focus.enabled = false,
        "hidden" => current.focus.visible = false,
        "capture" => current.focus.capture = 300,
        "root_birth" => current.root.instance.process_creation_time_100ns += 1,
        "root_frame" => current.root.bounds.as_mut().unwrap()[0] += 1,
        "root_dpi" => current.root.dpi += 1,
        "desktop" => current.desktop[2] += 1,
        "popup_birth" => {
            current.overlays[0]
                .state
                .instance
                .process_creation_time_100ns += 1
        }
        "new_same_process_occluder" | "foreign_occluder" => {
            let mut blocker = state(400, [120, 120, 10, 10]);
            if changed == "foreign_occluder" {
                blocker.process_id += 1;
            }
            current.roots.insert(0, entry(blocker));
        }
        "reordered" => current.roots.swap(0, 1),
        _ => unreachable!(),
    }
    assert!(!completed_teardown_unchanged(&expected, &current));
}

#[rstest]
fn root_focus_return_is_not_a_matching_composite_or_new_input_proof() {
    let before = sample();
    let expected = validate_sample(42, 100, &[200, 300], &before).unwrap();
    let mut after = before.clone();
    after.focus.focus = before.root.window_handle;
    after.focus.focus_instance = before.root.instance;
    let current = validate_sample(42, 100, &[200, 300], &after).unwrap();
    assert_ne!(expected, current);
    assert!(!expected.matches_completed_keyboard_postcheck(&current));
}

#[rstest]
#[case("enum_then_hidden")]
#[case("removed")]
#[case("remaining_shadow")]
#[case("focus_return")]
#[case("disjoint_change")]
#[case("hidden_only_original_trace")]
#[case("unchanged")]
fn completed_teardown_pair_accepts_only_forward_closure_progress(#[case] progress: &str) {
    let mut before = sample();
    if progress != "focus_return" {
        before.focus.focus = before.root.window_handle;
        before.focus.focus_instance = before.root.instance;
    }
    let expected = validate_sample(42, 100, &[200, 300], &before).unwrap();
    let mut after = before.clone();
    match progress {
        "enum_then_hidden" => {
            after.focus.capture = 0;
            for popup in &mut after.overlays {
                popup.state.visible = false;
                popup.style &= !WS_VISIBLE;
            }
            // EnumWindows ran before the native popup reads: its two original
            // overlap entries still say visible, with identical identity/bounds.
            let mut disjoint = entry(state(900, [1500, 800, 10, 10]));
            disjoint.process_id = 99;
            disjoint.decision = RootProofDecision::Disjoint;
            after.roots.insert(0, disjoint);
        }
        "removed" => {
            after.focus.capture = 0;
            after.overlays.clear();
            after.roots.drain(..2);
        }
        "remaining_shadow" => {
            after.focus.capture = 0;
            after.overlays.remove(0);
            after.roots.remove(0);
        }
        "focus_return" => {
            after.focus.focus = before.root.window_handle;
            after.focus.focus_instance = before.root.instance;
        }
        "disjoint_change" => {
            let mut disjoint = entry(state(900, [1500, 800, 10, 10]));
            disjoint.process_id = 99;
            disjoint.decision = RootProofDecision::Disjoint;
            after.roots.insert(0, disjoint);
        }
        "hidden_only_original_trace" => {
            before.focus.capture = 0;
            before.overlays.remove(0);
            before.roots.remove(0);
            after = before.clone();
            let mut hidden = entry(expected.capture);
            hidden.visible = false;
            hidden.cloaked = None;
            hidden.dwm_bounds = None;
            hidden.decision = RootProofDecision::Hidden;
            after.roots.insert(0, hidden);
            assert!(validate_sample(42, 100, &[200, 300], &after).is_err());
        }
        "unchanged" => {}
        _ => unreachable!(),
    }
    assert!(completed_teardown_progress(&expected, &before, &after));
}

#[rstest]
#[case("visibility_reversed")]
#[case("removed_reappears")]
#[case("capture_regained")]
#[case("focus_returned_to_child")]
#[case("focus_unknown_child")]
#[case("foreground")]
#[case("gui_flags")]
#[case("popup_birth")]
#[case("popup_thread")]
#[case("popup_class")]
#[case("popup_owner")]
#[case("popup_dpi")]
#[case("popup_geometry")]
#[case("popup_style")]
#[case("visibility_style_only")]
#[case("visibility_style_set")]
#[case("visibility_style_not_cleared")]
#[case("hidden_style_only")]
#[case("popup_enabled")]
#[case("popup_reenabled")]
#[case("popup_foreground")]
#[case("popup_maximized")]
#[case("root_birth")]
#[case("root_geometry")]
#[case("root_dpi")]
#[case("desktop")]
#[case("new_same_process_overlap")]
#[case("foreign_overlap")]
#[case("unknown_bounds")]
#[case("unknown_cloaking")]
#[case("disjoint_query_error")]
#[case("hidden_query_error")]
#[case("popup_order")]
#[case("trace_style")]
#[case("root_trace_style")]
fn completed_teardown_pair_never_accepts_reverse_unknown_or_other_state(#[case] changed: &str) {
    let original = sample();
    let expected = validate_sample(42, 100, &[200, 300], &original).unwrap();
    let mut before = original.clone();
    before.focus.focus = before.root.window_handle;
    before.focus.focus_instance = before.root.instance;
    let mut after = before.clone();
    after.focus.capture = 0;
    for popup in &mut after.overlays {
        popup.state.visible = false;
        popup.style &= !WS_VISIBLE;
    }
    assert!(completed_teardown_progress(&expected, &before, &after));
    match changed {
        "visibility_reversed" => {
            before.overlays[0].state.visible = false;
            before.overlays[0].style &= !WS_VISIBLE;
            after.overlays[0].state.visible = true;
            after.overlays[0].style |= WS_VISIBLE;
        }
        "removed_reappears" => {
            before.focus.capture = 0;
            before.overlays.remove(0);
            before.roots.remove(0);
        }
        "capture_regained" => {
            before.focus.capture = 0;
            after.focus.capture = 200;
        }
        "focus_returned_to_child" => {
            after.focus.focus = original.focus.focus;
            after.focus.focus_instance = original.focus.focus_instance;
        }
        "focus_unknown_child" => {
            after.focus.focus = 102;
            after.focus.focus_instance = state(102, [101, 101, 10, 10]).instance;
        }
        "foreground" => after.focus.foreground = 300,
        "gui_flags" => after.focus.flags = GUI_CARETBLINKING,
        "popup_birth" => after.overlays[0].state.instance.process_creation_time_100ns += 1,
        "popup_thread" => after.overlays[0].state.instance.window_thread_id += 1,
        "popup_class" => after.overlays[0].state.instance.window_class_hash += 1,
        "popup_owner" => after.overlays[0].state.instance.owner_window_handle += 1,
        "popup_dpi" => after.overlays[0].state.dpi += 1,
        "popup_geometry" => {
            after.overlays[0].state.bounds.as_mut().unwrap()[2] += 1;
            after.overlays[0].state.visible_bounds = after.overlays[0].state.bounds;
            after.roots[0] = entry(after.overlays[0].state);
        }
        "popup_style" => after.overlays[0].style |= 1,
        "visibility_style_only" => {
            after = before.clone();
            after.overlays[0].style &= !WS_VISIBLE;
        }
        "visibility_style_set" => {
            before.overlays[0].state.visible = false;
            before.overlays[0].style &= !WS_VISIBLE;
            after.overlays[0].style |= WS_VISIBLE;
        }
        "visibility_style_not_cleared" => after.overlays[0].style |= WS_VISIBLE,
        "hidden_style_only" => before.overlays[0].state.visible = false,
        "popup_enabled" => after.overlays[0].enabled = false,
        "popup_reenabled" => {
            before.overlays[0].state.visible = false;
            before.overlays[0].style &= !WS_VISIBLE;
            before.overlays[0].enabled = false;
        }
        "popup_foreground" => after.overlays[0].state.foreground = true,
        "popup_maximized" => after.overlays[0].state.maximized = true,
        "root_birth" => after.root.instance.process_creation_time_100ns += 1,
        "root_geometry" => after.root.bounds.as_mut().unwrap()[2] += 1,
        "root_dpi" => after.root.dpi += 1,
        "desktop" => after.desktop[2] += 1,
        "new_same_process_overlap" | "foreign_overlap" => {
            let mut blocker = entry(state(400, [120, 120, 10, 10]));
            if changed == "foreign_overlap" {
                blocker.process_id = 99;
            }
            after.roots.insert(0, blocker);
        }
        "unknown_bounds" => after.roots[0].dwm_bounds = None,
        "unknown_cloaking" => after.roots[0].cloaked = None,
        "disjoint_query_error" | "hidden_query_error" => {
            let mut irrelevant = entry(state(900, [1500, 800, 10, 10]));
            irrelevant.decision = RootProofDecision::Disjoint;
            if changed == "hidden_query_error" {
                irrelevant.visible = false;
                irrelevant.decision = RootProofDecision::Hidden;
            }
            irrelevant.cloak_query_error = Some(5);
            after.roots.insert(0, irrelevant);
        }
        "popup_order" => after.overlays.swap(0, 1),
        "trace_style" => after.roots[0].extended_style += 1,
        "root_trace_style" => after.roots.last_mut().unwrap().extended_style += 1,
        _ => unreachable!(),
    }
    assert!(!completed_teardown_progress(&expected, &before, &after));
}

#[rstest]
fn completed_keyboard_postcheck_accepts_only_a_fresh_geometry_transition() {
    let (expected, current) = NativeCapturePopupProof::test_geometry_transition();
    assert!(expected.matches_completed_keyboard_postcheck(&current));
}

#[rstest]
#[case("root")]
#[case("root_dpi")]
#[case("desktop")]
#[case("capture_handle")]
#[case("focus")]
#[case("focus_birth")]
#[case("gui_flags")]
#[case("active")]
#[case("foreground")]
#[case("disabled_focus")]
#[case("hidden_focus")]
#[case("popup_birth")]
#[case("popup_thread")]
#[case("popup_class")]
#[case("popup_owner")]
#[case("popup_dpi")]
#[case("popup_visibility")]
#[case("selected_handle")]
#[case("selected_order")]
#[case("selected_count")]
#[case("z_order")]
fn geometry_postcheck_never_accepts_another_episode_or_native_state(#[case] case: &str) {
    let (expected, mut current) = NativeCapturePopupProof::test_geometry_transition();
    match case {
        "root" => current.root.bounds.as_mut().unwrap()[0] += 1,
        "root_dpi" => current.root.dpi += 1,
        "desktop" => current.desktop[2] += 1,
        "capture_handle" => current.focus.capture = 300,
        "focus" => current.focus.focus += 1,
        "focus_birth" => current.focus.focus_instance.process_creation_time_100ns += 1,
        "gui_flags" => current.focus.flags = GUI_CARETBLINKING,
        "active" => current.focus.active = 300,
        "foreground" => current.focus.foreground = 300,
        "disabled_focus" => current.focus.enabled = false,
        "hidden_focus" => current.focus.visible = false,
        "popup_birth" => current.overlays[0].instance.process_creation_time_100ns += 1,
        "popup_thread" => current.overlays[0].instance.window_thread_id += 1,
        "popup_class" => current.overlays[0].instance.window_class_hash += 1,
        "popup_owner" => current.overlays[0].instance.owner_window_handle = 300,
        "popup_dpi" => current.overlays[0].dpi += 1,
        "popup_visibility" => current.overlays[0].visible = false,
        "selected_handle" => current.overlays[0].window_handle += 1,
        "selected_order" => current.overlays.swap(0, 1),
        "selected_count" => {
            current.overlays.pop();
        }
        "z_order" => current.ordered_occluders.swap(0, 1),
        _ => unreachable!(),
    }
    assert!(!expected.matches_completed_keyboard_postcheck(&current));
}

#[rstest]
fn only_explicit_capture_companions_can_prove_main_root_keyboard_observation() {
    let sample = sample();
    let proof = validate_sample(42, 100, &[200, 300], &sample).unwrap();
    assert_eq!(proof.capture.window_handle, 200);
    assert_eq!(proof.overlays.len(), 2);
    assert_eq!(
        proof.redacted_regions,
        vec![[110, 110, 100, 30], [110, 135, 180, 120]]
    );
    // Absence of an owner is accepted only because these exact HWNDs were
    // explicitly selected and the capture/focus/full native instances match.
    assert_eq!(proof.capture.instance.owner_window_handle, 0);
    assert!(validate_sample(42, 100, &[200], &sample).is_err());
}

#[rstest]
#[case("empty")]
#[case("duplicate")]
#[case("too_many")]
#[case("zero")]
#[case("root")]
fn selectors_are_exact_unique_and_bounded(#[case] case: &str) {
    let selected = match case {
        "empty" => vec![],
        "duplicate" => vec![200, 200],
        "too_many" => (200..209).collect(),
        "zero" => vec![0],
        "root" => vec![100],
        _ => unreachable!(),
    };
    assert_eq!(
        validate_sample(42, 100, &selected, &sample())
            .unwrap_err()
            .reason,
        NativeCapturePopupRefusal::InvalidSelection
    );
}

#[rstest]
#[case("root_foreground")]
#[case("active")]
#[case("focus_root")]
#[case("focus_birth")]
#[case("focus_thread")]
#[case("menu")]
#[case("disabled_focus")]
#[case("hidden_focus")]
#[case("capture")]
#[case("pid")]
#[case("birth")]
#[case("thread")]
#[case("class")]
#[case("owner")]
#[case("child")]
#[case("not_popup")]
#[case("minimized")]
#[case("disabled")]
#[case("root_geometry")]
#[case("popup_geometry")]
#[case("cloaked")]
#[case("incomplete")]
fn wrong_focus_identity_geometry_or_popup_shape_never_proves_input(#[case] case: &str) {
    let mut sample = sample();
    match case {
        "root_foreground" => sample.root.foreground = false,
        "active" => sample.focus.active = 200,
        "focus_root" => sample.focus.focus_root = 200,
        "focus_birth" => sample.focus.focus_instance.process_creation_time_100ns += 1,
        "focus_thread" => sample.focus.focus_instance.window_thread_id += 1,
        "menu" => sample.focus.flags = !GUI_CARETBLINKING,
        "disabled_focus" => sample.focus.enabled = false,
        "hidden_focus" => sample.focus.visible = false,
        "capture" => sample.focus.capture = 400,
        "pid" => sample.overlays[0].state.process_id += 1,
        "birth" => {
            sample.overlays[0]
                .state
                .instance
                .process_creation_time_100ns += 1
        }
        "thread" => sample.overlays[0].state.instance.window_thread_id += 1,
        "class" => sample.overlays[0].state.instance.window_class_hash = 0,
        "owner" => sample.overlays[0].owner_is_local_or_absent = false,
        "child" => sample.overlays[0].style |= WS_CHILD,
        "not_popup" => sample.overlays[0].style = 0,
        "minimized" => sample.overlays[0].state.minimized = true,
        "disabled" => sample.overlays[0].enabled = false,
        "root_geometry" => sample.roots.last_mut().unwrap().dwm_bounds = Some([100, 100, 801, 600]),
        "popup_geometry" => sample.roots[0].dwm_bounds = Some([110, 110, 101, 30]),
        "cloaked" => sample.roots[0].cloaked = None,
        "incomplete" => {
            sample.roots.pop();
        }
        _ => unreachable!(),
    }
    assert!(validate_sample(42, 100, &[200, 300], &sample).is_err());
}

#[rstest]
#[case(true)]
#[case(false)]
fn every_unselected_occluder_is_refused_even_same_process(#[case] foreign: bool) {
    let mut sample = sample();
    let mut blocker = state(400, [120, 120, 10, 10]);
    if foreign {
        blocker.process_id = 99;
    }
    sample.roots.insert(0, entry(blocker));
    assert_eq!(
        validate_sample(42, 100, &[200, 300], &sample)
            .unwrap_err()
            .reason,
        NativeCapturePopupRefusal::UnselectedOccluder
    );
}

#[rstest]
fn selected_popup_pixels_outside_the_main_root_also_need_complete_coverage() {
    let mut sample = sample();
    sample.overlays[0].state.bounds = Some([850, 110, 150, 30]);
    sample.overlays[0].state.visible_bounds = sample.overlays[0].state.bounds;
    sample.roots[0] = entry(sample.overlays[0].state);
    let mut blocker = state(400, [930, 110, 20, 20]);
    blocker.process_id = 99;
    sample.roots.insert(0, entry(blocker));
    assert_eq!(
        validate_sample(42, 100, &[200, 300], &sample)
            .unwrap_err()
            .reason,
        NativeCapturePopupRefusal::UnselectedOccluder
    );
}

#[rstest]
fn refused_native_sample_preserves_the_actual_blocker_without_reading_it_again() {
    let mut sample = sample();
    let mut blocker = state(400, [120, 120, 10, 10]);
    blocker.process_id = 99;
    sample.roots.insert(0, entry(blocker));
    let rejected = validate_sample(42, 100, &[200, 300], &sample).unwrap_err();
    let diagnostic = rejected.diagnostic(sample.root);
    assert_eq!(
        diagnostic.reason,
        NativeCapturePopupRefusal::UnselectedOccluder
    );
    assert_eq!(diagnostic.target_process_id, 42);
    assert_eq!(diagnostic.target_window_handle, 100);
    assert_eq!(diagnostic.target_bounds, sample.root.visible_bounds);
    assert_eq!(diagnostic.blocker_process_id, Some(99));
    assert_eq!(diagnostic.blocker_window_handle, Some(400));
    assert_eq!(diagnostic.blocker_bounds, blocker.visible_bounds);
    assert_eq!(
        rejected.source.to_string(),
        refused(NativeCapturePopupRefusal::UnselectedOccluder)
            .source
            .to_string()
    );
    // A blocker does not conceal an earlier, higher-priority focus refusal.
    sample.focus.foreground = 400;
    let rejected = validate_sample(42, 100, &[200, 300], &sample).unwrap_err();
    let diagnostic = rejected.diagnostic(sample.root);
    assert_eq!(
        diagnostic.reason,
        NativeCapturePopupRefusal::FocusOutsideRoot
    );
    assert_eq!(diagnostic.blocker_window_handle, None);
}

#[rstest]
#[case(NativeCapturePopupRefusal::FocusOutsideRoot)]
#[case(NativeCapturePopupRefusal::GuiStateUnavailable)]
#[case(NativeCapturePopupRefusal::EnumerationIncomplete)]
#[case(NativeCapturePopupRefusal::NativeStateChanged)]
#[case(NativeCapturePopupRefusal::InvalidSelection)]
fn inventory_preserves_the_closed_native_refusal_without_exposing_source_text(
    #[case] reason: NativeCapturePopupRefusal,
) {
    let mut error = refused(reason);
    error.source = Box::new(UiaError::OperationFailed(
        "private native error text".into(),
    ));
    let inventory = finish_inventory(
        NativeCapturePopupInventory {
            process_id: 42,
            root_window_handle: 100,
            authorizes_input: false,
            complete: false,
            capture_window_handle: None,
            candidates: vec![],
            refusal_reason: None,
        },
        Err(error),
    );
    assert_eq!(inventory.refusal_reason, Some(reason));
    assert!(!inventory.complete && !inventory.authorizes_input);
    assert!(inventory.candidates.is_empty() && inventory.capture_window_handle.is_none());
    let serialized = serde_json::to_value(inventory).unwrap();
    assert_eq!(
        serialized["refusal_reason"],
        serde_json::to_value(reason).unwrap()
    );
    assert!(!serialized.to_string().contains("private native error text"));
}

#[rstest]
fn proof_equality_pins_private_focus_and_the_full_ordered_occluder_instances() {
    let before = sample();
    let proof = validate_sample(42, 100, &[200, 300], &before).unwrap();
    let mut reordered = sample();
    reordered.roots.swap(0, 1);
    assert_ne!(
        proof,
        validate_sample(42, 100, &[200, 300], &reordered).unwrap()
    );
    let mut refocused = sample();
    refocused.focus.focus += 1;
    refocused.focus.focus_instance.window_class_hash += 1;
    assert_ne!(
        proof,
        validate_sample(42, 100, &[200, 300], &refocused).unwrap()
    );
    let wire = serde_json::to_value(proof).unwrap();
    assert!(wire.get("focus").is_none());
    assert!(wire.get("ordered_occluders").is_none());
    assert!(wire.get("desktop").is_none());
}
