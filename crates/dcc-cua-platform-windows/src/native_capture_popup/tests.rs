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
                style: WS_POPUP,
                enabled: true,
                owner_is_local_or_absent: true,
            })
            .collect(),
        desktop: [0, 0, 1920, 1080],
    }
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
        validate_sample(42, 100, &selected, &sample()).unwrap_err(),
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
        validate_sample(42, 100, &[200, 300], &sample).unwrap_err(),
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
        validate_sample(42, 100, &[200, 300], &sample).unwrap_err(),
        NativeCapturePopupRefusal::UnselectedOccluder
    );
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
}
