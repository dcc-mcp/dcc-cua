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
mod tests;
