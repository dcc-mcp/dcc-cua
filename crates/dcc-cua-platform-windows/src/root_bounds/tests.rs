use super::*;
use rstest::rstest;

fn context() -> RootBoundsContext {
    RootBoundsContext {
        window_handle: 1_312_534,
        target_root_window_handle: 1_704_438,
        visible: true,
        cloaked: Some(0),
    }
}

#[rstest]
fn captured_non_target_zero_area_needs_confirmed_empty_decision() {
    let decision = resolve(context(), [0; 4], || Ok([0; 4]), || Ok([0; 4]))
        .expect("native-confirmed empty non-target root cannot occlude a pixel");
    assert_eq!(
        decision,
        RootBoundsDecision::ConfirmedEmptyAboveTarget(EmptyRootProof {
            window_handle: 1_312_534,
            target_root_window_handle: 1_704_438,
            dwm_initial: [0; 4],
            win32: [0; 4],
            dwm_confirmed: [0; 4],
        })
    );
    assert_eq!(decision.z_order_entry(), (1_312_534, [0; 4], false));
}

#[rstest]
fn target_zero_area_is_never_exempted() {
    let mut target = context();
    target.window_handle = target.target_root_window_handle;
    assert!(
        resolve(
            target,
            [0; 4],
            || Ok([0; 4]),
            || panic!("no target exemption")
        )
        .is_err()
    );
}

#[rstest]
fn one_pixel_root_remains_a_pixel_contributor_without_extra_queries() {
    let decision = resolve(
        context(),
        [0, 0, 1, 1],
        || panic!("no Win32 fallback for a positive root"),
        || panic!("no confirmation for a positive root"),
    )
    .unwrap();
    assert_eq!(decision.z_order_entry(), (1_312_534, [0, 0, 1, 1], true));
}

#[rstest]
fn invalid_geometry_never_becomes_empty() {
    for edges in [[1, 0, 0, 1], [0, 1, 1, 0], [i32::MIN, 0, i32::MAX, 0]] {
        assert!(
            resolve(
                context(),
                edges,
                || Ok([0; 4]),
                || panic!("no confirmation")
            )
            .is_err()
        );
    }
}

#[rstest]
fn win32_failure_or_disagreement_remains_refused() {
    for win32 in [Err(-5), Ok([0, 0, 1, 1]), Ok([1, 0, 0, 1])] {
        assert!(resolve(context(), [0; 4], || win32, || panic!("no confirmation")).is_err());
    }
}

#[rstest]
fn confirmation_failure_or_geometry_change_remains_refused() {
    for dwm in [
        Err(-5),
        Ok([0, 0, 1, 1]),
        Ok([1, 0, 0, 1]),
        Ok([i32::MIN, 0, i32::MAX, 0]),
    ] {
        assert!(resolve(context(), [0; 4], || Ok([0; 4]), || dwm).is_err());
    }
}

#[rstest]
fn zero_area_at_different_edges_is_not_consistent_geometry() {
    assert!(
        resolve(
            context(),
            [0; 4],
            || Ok([1, 1, 1, 1]),
            || panic!("Win32 mismatch")
        )
        .is_err()
    );
    assert!(resolve(context(), [0; 4], || Ok([0; 4]), || Ok([1, 1, 1, 1])).is_err());
}

#[rstest]
fn empty_footprints_need_one_win32_and_one_confirmation_read() {
    let win32_reads = std::cell::Cell::new(0);
    let confirmations = std::cell::Cell::new(0);
    for edges in [[0; 4], [-10, 20, -10, 35], [10, -20, 15, -20]] {
        let decision = resolve(
            context(),
            edges,
            || {
                win32_reads.set(win32_reads.get() + 1);
                Ok(edges)
            },
            || {
                confirmations.set(confirmations.get() + 1);
                Ok(edges)
            },
        )
        .unwrap();
        assert!(!decision.z_order_entry().2);
    }
    assert_eq!(win32_reads.get(), 3);
    assert_eq!(confirmations.get(), 3);
}

#[rstest]
fn missing_composition_evidence_never_grants_empty_exemption() {
    for (visible, cloaked) in [(false, Some(0)), (true, None), (true, Some(1))] {
        let mut ctx = context();
        ctx.visible = visible;
        ctx.cloaked = cloaked;
        assert!(
            resolve(
                ctx,
                [0; 4],
                || Ok([0; 4]),
                || panic!("no composition proof")
            )
            .is_err()
        );
    }
}
