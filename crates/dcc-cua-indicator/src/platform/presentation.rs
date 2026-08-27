use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOTOWNER, GetAncestor, GetForegroundWindow, IsIconic, IsWindowVisible,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetPresentationPolicy {
    Hidden,
    ExactTargetForeground,
    OwnedModalForeground,
    TargetScopedBehindUnrelatedForeground,
}

impl TargetPresentationPolicy {
    pub(crate) const fn is_visible(self) -> bool {
        !matches!(self, Self::Hidden)
    }
}

pub(crate) struct TargetOverlaySyncState {
    previous: TargetPresentationPolicy,
}

impl TargetOverlaySyncState {
    pub(crate) const fn new(previous: TargetPresentationPolicy) -> Self {
        Self { previous }
    }

    /// Return true only when a presentation transition can put the exact target
    /// above its unowned overlays. Stable polling ticks deliberately do no work.
    pub(crate) fn observe(&mut self, current: TargetPresentationPolicy) -> bool {
        let previous = std::mem::replace(&mut self.previous, current);
        let entered_target_foreground = current != previous
            && matches!(
                current,
                TargetPresentationPolicy::ExactTargetForeground
                    | TargetPresentationPolicy::OwnedModalForeground
            );
        current.is_visible()
            && (matches!(previous, TargetPresentationPolicy::Hidden) || entered_target_foreground)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn target_presentation_policy(
    target_visible: bool,
    target_minimized: bool,
    target_window: u64,
    target_root_owner: u64,
    foreground_window: Option<u64>,
    foreground_root_owner: Option<u64>,
) -> TargetPresentationPolicy {
    if !target_visible || target_minimized {
        return TargetPresentationPolicy::Hidden;
    }
    if foreground_window == Some(target_window) {
        TargetPresentationPolicy::ExactTargetForeground
    } else if foreground_root_owner == Some(target_root_owner) {
        TargetPresentationPolicy::OwnedModalForeground
    } else {
        TargetPresentationPolicy::TargetScopedBehindUnrelatedForeground
    }
}

fn window_handle_value(window: HWND) -> u64 {
    window.0 as usize as u64
}

fn root_owner_value(window: HWND) -> u64 {
    let root_owner = unsafe { GetAncestor(window, GA_ROOTOWNER) };
    if root_owner.0.is_null() {
        window_handle_value(window)
    } else {
        window_handle_value(root_owner)
    }
}

pub(super) fn current_target_presentation(target_window: HWND) -> TargetPresentationPolicy {
    let foreground = unsafe { GetForegroundWindow() };
    let foreground_window = (!foreground.0.is_null()).then(|| window_handle_value(foreground));
    let foreground_root_owner = (!foreground.0.is_null()).then(|| root_owner_value(foreground));
    target_presentation_policy(
        unsafe { IsWindowVisible(target_window).as_bool() },
        unsafe { IsIconic(target_window).as_bool() },
        window_handle_value(target_window),
        root_owner_value(target_window),
        foreground_window,
        foreground_root_owner,
    )
}
