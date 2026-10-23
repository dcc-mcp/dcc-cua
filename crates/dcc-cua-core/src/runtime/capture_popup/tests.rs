use super::*;
use rstest::rstest;

#[rstest]
fn capture_popup_selection_is_explicit_unique_and_bounded() {
    for selected in [vec![], vec![0], vec![1, 1], (1..=9).collect()] {
        assert!(validate_selection(&selected).is_err());
    }
    assert!(validate_selection(&[1]).is_ok());
    assert!(validate_selection(&(1..=8).collect::<Vec<_>>()).is_ok());
}

#[rstest]
fn popup_pixel_coverage_requires_every_root_redaction_to_be_independently_observed() {
    let root = [[10, 10, 20, 20]];
    assert!(require_complete_coverage(&root, &[]).is_err());
    assert!(require_complete_coverage(&root, &[([10, 10, 10, 20], vec![])]).is_err());
    assert!(
        require_complete_coverage(
            &root,
            &[([10, 10, 10, 20], vec![]), ([20, 10, 10, 20], vec![])]
        )
        .is_ok()
    );
    assert!(require_complete_coverage(&root, &[([10, 10, 20, 20], vec![[15, 15, 1, 1]])]).is_err());
    assert!(
        require_complete_coverage(
            &root,
            &[
                ([10, 10, 20, 20], vec![[15, 15, 1, 1]]),
                ([15, 15, 1, 1], vec![])
            ]
        )
        .is_ok()
    );
}

#[rstest]
fn popup_coverage_never_turns_an_invalid_or_redacted_frame_into_root_pixels() {
    assert!(require_complete_coverage(&[[0, 0, 0, 1]], &[]).is_err());
    assert!(require_complete_coverage(&[[i32::MAX, 0, 1, 1]], &[]).is_err());
    assert!(
        require_complete_coverage(&[[0, 0, 1, 1]], &[([0, 0, 1, 1], vec![[0, 0, 1, 1]])]).is_err()
    );
    assert!(
        require_complete_coverage(&[[0, 0, 2, 1]], &[([0, 0, 2, 1], vec![[1, 0, 1, 1]])]).is_err()
    );
}

#[rstest]
fn coverage_preserves_holes_and_accepts_overlap_without_double_counting() {
    let root = [[0, 0, 10, 10], [4, 4, 2, 2]];
    assert!(
        require_complete_coverage(&root, &[([0, 0, 5, 10], vec![]), ([4, 0, 6, 10], vec![])])
            .is_ok()
    );
    let frames = [([0, 0, 10, 10], vec![[4, 4, 2, 2]]), ([4, 4, 2, 1], vec![])];
    assert!(require_complete_coverage(&root, &frames).is_err());
}

#[rstest]
fn composite_keyboard_binding_rejects_expiry_cross_session_and_wrong_root_or_observation() {
    let observation = ComputerUseObservation {
        observation_id: "obs".into(),
        window_handle: 100,
        process_id: 77,
        window_title: String::new(),
        width: 10,
        height: 10,
        source_rect: [0, 0, 10, 10],
        capture_backend: "dcc-cua-visible-exact-window".into(),
        capture_provenance: json!({}),
        session_id: "session".into(),
    };
    let action: ComputerUseAction =
        serde_json::from_value(json!({"action":"keypress","observation_id":"obs"})).unwrap();
    let scope = ComputerUseTargetScope {
        process_id: Some(77),
        window_handle: Some(100),
        window_title: None,
    };
    assert!(
        validate_keyboard_binding(
            "obs",
            "session",
            (77, 100),
            &action,
            &observation,
            &scope,
            ("session", Duration::from_secs(5))
        )
        .is_ok()
    );
    for (id, session, root, current, age) in [
        ("old", "session", (77, 100), "session", Duration::ZERO),
        ("obs", "other", (77, 100), "session", Duration::ZERO),
        ("obs", "session", (77, 101), "session", Duration::ZERO),
        ("obs", "session", (78, 100), "session", Duration::ZERO),
        ("obs", "session", (77, 100), "other", Duration::ZERO),
        (
            "obs",
            "session",
            (77, 100),
            "session",
            Duration::from_millis(5001),
        ),
    ] {
        assert!(
            validate_keyboard_binding(
                id,
                session,
                root,
                &action,
                &observation,
                &scope,
                (current, age)
            )
            .is_err()
        );
    }
}
