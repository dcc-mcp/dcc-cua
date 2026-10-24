use super::*;
use rstest::rstest;

fn window(handle: u64, owner: u64, style: u32) -> NativePopupWindow {
    NativePopupWindow {
        window_handle: handle,
        instance: ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: 123456789,
            window_thread_id: 12,
            window_class_hash: handle + 100,
            owner_window_handle: owner,
        },
        is_root: true,
        style,
        visible: true,
        enabled: true,
        minimized: false,
        cloaked: Some(0),
    }
}

fn relation(window: NativePopupWindow) -> NativePopupWindowRelation {
    NativePopupWindowRelation::SameProcess { window }
}

fn sample() -> NativeFocusedPopupSample {
    let bound = window(100, 0, 0);
    let popup = window(200, 100, WS_POPUP);
    let mut focus = window(201, 0, WS_CHILD);
    focus.is_root = false;
    NativeFocusedPopupSample {
        bound_root: relation(bound.clone()),
        gui_thread_flags: Some(0),
        foreground: relation(bound.clone()),
        active: relation(bound.clone()),
        focus: relation(focus),
        focus_root: relation(popup),
        focus_root_owner: relation(bound.clone()),
        focus_is_root_or_child: Some(true),
        capture: NativePopupWindowRelation::None,
        capture_root: NativePopupWindowRelation::None,
        owner_chain: vec![bound],
        owner_chain_status: NativePopupOwnerChainStatus::ReachedBoundRoot,
    }
}

fn mutate_window(
    relation: &mut NativePopupWindowRelation,
    update: impl FnOnce(&mut NativePopupWindow),
) {
    let NativePopupWindowRelation::SameProcess { window } = relation else {
        panic!("expected own window fixture")
    };
    update(window);
}

#[rstest]
fn focused_owned_popup_with_foreground_main_is_proven_but_never_authorized() {
    let original = sample();
    let mut reads = 0;
    let diagnostic = diagnostic_from_reads(77, 100, || {
        reads += 1;
        original.clone()
    });
    assert_eq!(reads, 2);
    assert!(diagnostic.stable && diagnostic.proven);
    assert!(!diagnostic.authorizes_input);
    assert_eq!(diagnostic.refusal_reason, None);
    assert_eq!(
        diagnostic.bound_root_native_instance,
        original.bound_root.window().map(|window| window.instance)
    );
}

#[rstest]
fn direct_focus_and_foreground_popup_are_supported_without_selecting_a_sibling() {
    let mut facts = sample();
    facts.focus = facts.focus_root.clone();
    facts.foreground = facts.focus_root.clone();
    facts.active = facts.focus_root.clone();
    assert_eq!(validate_sample(77, 100, &facts), Ok(()));
    // A completion-list sibling is not the keyboard recipient merely because
    // it shares PID, owner, visibility, style, and native GUI thread.
    facts.focus_is_root_or_child = Some(false);
    assert_eq!(
        validate_sample(77, 100, &facts),
        Err(NativeFocusedPopupRefusal::FocusOutsidePopup)
    );
}

#[rstest]
fn foreign_focus_and_foreground_are_refused_without_serializing_foreign_identity() {
    for field in ["focus", "foreground", "owner"] {
        let mut facts = sample();
        match field {
            "focus" => facts.focus = NativePopupWindowRelation::ForeignProcess,
            "foreground" => facts.foreground = NativePopupWindowRelation::ForeignProcess,
            _ => facts.focus_root_owner = NativePopupWindowRelation::ForeignOwner,
        }
        let diagnostic = diagnostic_from_reads(77, 100, || facts.clone());
        assert!(!diagnostic.proven && !diagnostic.authorizes_input);
        let value = serde_json::to_value(&diagnostic).unwrap();
        let key = if field == "owner" {
            "focus_root_owner"
        } else {
            field
        };
        let rejected = &value["before"][key];
        assert!(rejected.get("window").is_none());
        assert!(rejected.get("process_id").is_none());
        assert!(rejected.get("window_handle").is_none());
    }
}

#[rstest]
fn missing_gui_thread_or_focus_and_ordinary_main_focus_are_diagnostic_failures() {
    let mut facts = sample();
    facts.gui_thread_flags = None;
    assert_eq!(
        validate_sample(77, 100, &facts),
        Err(NativeFocusedPopupRefusal::GuiThreadUnavailable)
    );
    facts = sample();
    facts.focus = NativePopupWindowRelation::None;
    assert_eq!(
        validate_sample(77, 100, &facts),
        Err(NativeFocusedPopupRefusal::FocusUnavailable)
    );
    facts = sample();
    facts.focus_root = facts.bound_root.clone();
    assert_eq!(
        validate_sample(77, 100, &facts),
        Err(NativeFocusedPopupRefusal::FocusNotPopup)
    );
}

#[rstest]
fn popup_style_visibility_enabled_and_cloaking_must_all_be_proven() {
    let updates: [fn(&mut NativePopupWindow); 7] = [
        |window| window.style = 0,
        |window| window.style |= WS_CHILD,
        |window| window.is_root = false,
        |window| window.visible = false,
        |window| window.enabled = false,
        |window| window.minimized = true,
        |window| window.cloaked = None,
    ];
    for update in updates {
        let mut facts = sample();
        mutate_window(&mut facts.focus_root, update);
        assert!(validate_sample(77, 100, &facts).is_err());
    }
    let mut facts = sample();
    mutate_window(&mut facts.focus_root, |window| window.cloaked = Some(1));
    assert_eq!(
        validate_sample(77, 100, &facts),
        Err(NativeFocusedPopupRefusal::PopupNotUsable)
    );
}

#[rstest]
fn exact_process_birth_and_thread_are_required_for_every_reported_relation() {
    for field in 0..8 {
        let mut facts = sample();
        let relation = match field {
            0 => &mut facts.foreground,
            1 => &mut facts.active,
            2 => &mut facts.focus,
            3 => &mut facts.focus_root,
            4 => &mut facts.focus_root_owner,
            5 => {
                facts.capture = facts.focus.clone();
                &mut facts.capture
            }
            6 => {
                facts.capture_root = facts.focus_root.clone();
                &mut facts.capture_root
            }
            _ => {
                facts.owner_chain[0].instance.process_creation_time_100ns += 1;
                assert_eq!(
                    validate_sample(77, 100, &facts),
                    Err(NativeFocusedPopupRefusal::ProcessInstanceMismatch)
                );
                continue;
            }
        };
        mutate_window(relation, |window| {
            window.instance.process_creation_time_100ns += 1
        });
        assert_eq!(
            validate_sample(77, 100, &facts),
            Err(NativeFocusedPopupRefusal::ProcessInstanceMismatch)
        );
    }
    let mut facts = sample();
    mutate_window(&mut facts.focus, |window| {
        window.instance.window_thread_id += 1
    });
    assert_eq!(
        validate_sample(77, 100, &facts),
        Err(NativeFocusedPopupRefusal::ThreadMismatch)
    );
}

#[rstest]
fn owner_chain_requires_exact_links_and_bound_root_identity() {
    let mut facts = sample();
    let intermediate = window(150, 100, WS_POPUP);
    mutate_window(&mut facts.focus_root, |window| {
        window.instance.owner_window_handle = 150
    });
    facts.owner_chain.insert(0, intermediate);
    assert_eq!(validate_sample(77, 100, &facts), Ok(()));
    for status in [
        NativePopupOwnerChainStatus::NoOwner,
        NativePopupOwnerChainStatus::Unavailable,
        NativePopupOwnerChainStatus::ForeignProcess,
        NativePopupOwnerChainStatus::ForeignOwner,
        NativePopupOwnerChainStatus::Cycle,
        NativePopupOwnerChainStatus::LimitExceeded,
    ] {
        facts.owner_chain_status = status;
        assert_eq!(
            validate_sample(77, 100, &facts),
            Err(NativeFocusedPopupRefusal::OwnerChainUnproven)
        );
    }
    facts.owner_chain_status = NativePopupOwnerChainStatus::ReachedBoundRoot;
    facts.owner_chain[0].window_handle = 199;
    assert_eq!(
        validate_sample(77, 100, &facts),
        Err(NativeFocusedPopupRefusal::OwnerChainUnproven)
    );
    facts = sample();
    facts.owner_chain[0].instance.window_class_hash += 1;
    assert_eq!(
        validate_sample(77, 100, &facts),
        Err(NativeFocusedPopupRefusal::OwnerChainUnproven)
    );
}

#[rstest]
fn root_owner_active_and_capture_cannot_silently_follow_another_own_window() {
    for field in ["owner", "active", "capture"] {
        let mut facts = sample();
        match field {
            "owner" => facts.focus_root_owner = relation(window(300, 0, 0)),
            "active" => facts.active = relation(window(300, 0, 0)),
            _ => {
                facts.capture = relation(window(301, 0, WS_CHILD));
                facts.capture_root = relation(window(300, 0, WS_POPUP));
            }
        }
        assert!(validate_sample(77, 100, &facts).is_err());
    }
    let mut facts = sample();
    facts.capture = facts.focus.clone();
    facts.capture_root = facts.focus_root.clone();
    assert_eq!(validate_sample(77, 100, &facts), Ok(()));
    facts.capture = NativePopupWindowRelation::ForeignProcess;
    assert_eq!(
        validate_sample(77, 100, &facts),
        Err(NativeFocusedPopupRefusal::CaptureOutsideBoundRoots)
    );
}

#[rstest]
fn every_native_sample_difference_including_partial_failure_refuses_proof() {
    let original = sample();
    let mut changed = original.clone();
    changed.gui_thread_flags = Some(1);
    let mut reads = vec![original.clone(), changed].into_iter();
    let diagnostic = diagnostic_from_reads(77, 100, || reads.next().unwrap());
    assert!(!diagnostic.stable && !diagnostic.proven && !diagnostic.authorizes_input);
    assert_eq!(
        diagnostic.refusal_reason,
        Some(NativeFocusedPopupRefusal::NativeStateChanged)
    );
    let mut reads = vec![original, NativeFocusedPopupSample::default()].into_iter();
    let diagnostic = diagnostic_from_reads(77, 100, || reads.next().unwrap());
    assert_eq!(
        diagnostic.refusal_reason,
        Some(NativeFocusedPopupRefusal::NativeStateChanged)
    );
    assert_eq!(diagnostic.after.bound_root, NativePopupWindowRelation::None);
}

#[rstest]
fn stable_failed_proof_retains_partial_native_relationships() {
    let mut facts = sample();
    facts.focus = NativePopupWindowRelation::ForeignProcess;
    let diagnostic = diagnostic_from_reads(77, 100, || facts.clone());
    assert!(diagnostic.stable);
    assert!(!diagnostic.proven && !diagnostic.authorizes_input);
    assert_eq!(diagnostic.before, facts);
    assert_eq!(diagnostic.after, facts);
    assert_eq!(
        diagnostic.refusal_reason,
        Some(NativeFocusedPopupRefusal::FocusUnavailable)
    );
}

#[rstest]
fn invalid_target_never_has_a_proven_relationship() {
    for target in [(0, 100), (77, 0), (0, 0)] {
        let diagnostic =
            diagnostic_from_reads(target.0, target.1, NativeFocusedPopupSample::default);
        assert_eq!(
            diagnostic.refusal_reason,
            Some(NativeFocusedPopupRefusal::InvalidTarget)
        );
        assert!(!diagnostic.proven && !diagnostic.authorizes_input);
        assert_eq!(diagnostic.bound_root_native_instance, None);
    }
}
