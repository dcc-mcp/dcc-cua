//! Pure native-root geometry policy. Queries are supplied by the Win32 adapter.
//! No process, window style, title or client identity exempts a pixel footprint.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootBoundsClass {
    Positive,
    ZeroArea,
    Inverted,
    Overflow,
}

pub(crate) fn classify(edges: [i32; 4]) -> RootBoundsClass {
    let (Some(width), Some(height)) = (
        edges[2].checked_sub(edges[0]),
        edges[3].checked_sub(edges[1]),
    ) else {
        return RootBoundsClass::Overflow;
    };
    if width < 0 || height < 0 {
        RootBoundsClass::Inverted
    } else if width == 0 || height == 0 {
        RootBoundsClass::ZeroArea
    } else {
        RootBoundsClass::Positive
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RootBoundsContext {
    pub window_handle: u64,
    pub target_root_window_handle: u64,
    pub visible: bool,
    pub cloaked: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EmptyRootProof {
    pub window_handle: u64,
    pub target_root_window_handle: u64,
    pub dwm_initial: [i32; 4],
    pub win32: [i32; 4],
    pub dwm_confirmed: [i32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RootBoundsDecision {
    Positive {
        window_handle: u64,
        bounds: [i32; 4],
    },
    ConfirmedEmptyAboveTarget(EmptyRootProof),
}

impl RootBoundsDecision {
    /// The boolean means contributes pixels, not the raw IsWindowVisible bit.
    /// Even a proven-empty root keeps an entry and consumes enumeration budget.
    pub(crate) fn z_order_entry(self) -> (u64, [i32; 4], bool) {
        match self {
            Self::Positive {
                window_handle,
                bounds,
            } => (window_handle, bounds, true),
            Self::ConfirmedEmptyAboveTarget(proof) => (proof.window_handle, [0; 4], false),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RootBoundsRejection {
    pub win32: Result<[i32; 4], i32>,
    pub dwm_confirmation: Option<Result<[i32; 4], i32>>,
}

pub(crate) fn resolve(
    context: RootBoundsContext,
    dwm_initial: [i32; 4],
    read_win32: impl FnOnce() -> Result<[i32; 4], i32>,
    confirm_dwm: impl FnOnce() -> Result<[i32; 4], i32>,
) -> Result<RootBoundsDecision, RootBoundsRejection> {
    if classify(dwm_initial) == RootBoundsClass::Positive {
        return Ok(RootBoundsDecision::Positive {
            window_handle: context.window_handle,
            bounds: [
                dwm_initial[0],
                dwm_initial[1],
                dwm_initial[2] - dwm_initial[0],
                dwm_initial[3] - dwm_initial[1],
            ],
        });
    }
    let mut rejection = RootBoundsRejection {
        win32: read_win32(),
        dwm_confirmation: None,
    };
    // A target can never be empty. Only an above-target root with complete
    // composition evidence may prove an empty footprint. Raw edges must agree,
    // not merely their area classes: do not hide DPI or geometry transitions.
    if context.window_handle != context.target_root_window_handle
        && context.visible
        && context.cloaked == Some(0)
        && classify(dwm_initial) == RootBoundsClass::ZeroArea
        && rejection.win32 == Ok(dwm_initial)
    {
        // One bounded fresh DWM read is required after the Win32 sample. The old
        // diagnostic-only fallback alone never authorizes ignoring a root.
        let confirmed = confirm_dwm();
        if confirmed == Ok(dwm_initial) {
            return Ok(RootBoundsDecision::ConfirmedEmptyAboveTarget(
                EmptyRootProof {
                    window_handle: context.window_handle,
                    target_root_window_handle: context.target_root_window_handle,
                    dwm_initial,
                    win32: dwm_initial,
                    dwm_confirmed: dwm_initial,
                },
            ));
        }
        rejection.dwm_confirmation = Some(confirmed);
    }
    Err(rejection)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> RootBoundsContext {
        RootBoundsContext {
            window_handle: 1_312_534,
            target_root_window_handle: 1_704_438,
            visible: true,
            cloaked: Some(0),
        }
    }

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
    fn win32_failure_or_disagreement_remains_refused() {
        for win32 in [Err(-5), Ok([0, 0, 1, 1]), Ok([1, 0, 0, 1])] {
            assert!(resolve(context(), [0; 4], || win32, || panic!("no confirmation")).is_err());
        }
    }

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
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
}
