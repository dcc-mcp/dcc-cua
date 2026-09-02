//! Exact-window Windows UI Automation fallback.
//!
//! CUA remains the primary cross-platform backend. This crate owns the
//! Windows-only semantic fallback used when an application's UIA provider is
//! usable but CUA's combined window snapshot path is not.

#[cfg(windows)]
mod capture_identity;
mod contracts;
#[cfg(windows)]
mod input;
#[cfg(any(windows, test))]
mod snapshot;

#[cfg(windows)]
mod visible_capture;
#[cfg(windows)]
mod wgc;
#[cfg(windows)]
mod windows;

pub use contracts::{
    UiaAction, UiaError, UiaTarget, WindowsForegroundRelation, WindowsPointerButton,
    WindowsRawInputSnapshot, WindowsWindowIdentity,
};

#[cfg(windows)]
pub use capture_identity::{
    ExactWindowCaptureIdentityError, ExactWindowCaptureRoute, exact_window_capture_route,
};
#[cfg(windows)]
pub use input::{
    RelativeMoveInjection, WindowsForegroundClickError, WindowsForegroundClickOutcome,
    WindowsForegroundClickPreflightFailure, WindowsForegroundClickPreflightReason,
    WindowsHeldKeyError, WindowsInputCount, WindowsOverlayCommand, WindowsPostButtonUpSnapshot,
    animate_cursor_to, cursor_position, inject_absolute_mouse_move,
    inject_combined_source_move_and_left_down, inject_consumable_mouse_move, inject_drag_screen,
    inject_mouse_button, inject_relative_mouse_move, move_cursor_desktop, post_click_screen,
    post_message_blocked_by_uipi, post_scroll_screen, post_text, send_click_exact_foreground_mods,
    send_held_keys_exact_foreground, send_key_synthesized, send_overlay_command,
    send_text_synthesized, snapshot_left_button_after_up,
};

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsDesktopState {
    pub input_desktop_name: Option<String>,
    pub input_desktop_error: Option<String>,
    pub has_foreground_window: bool,
}

#[cfg(windows)]
#[must_use]
pub fn desktop_state() -> WindowsDesktopState {
    let state = platform_windows::diagnostics::desktop_state();
    let has_foreground_window = state.has_foreground_window();
    WindowsDesktopState {
        input_desktop_name: state.input_desktop_name,
        input_desktop_error: state.input_desktop_error,
        has_foreground_window,
    }
}
#[cfg(windows)]
pub use visible_capture::{
    ExactWindowPixelEvidence, ExactWindowPixelInstanceEvidence, VisibleWindowCapture,
    capture_visible_window, capture_window_content, exact_window_pixel_evidence,
};
#[cfg(windows)]
pub use wgc::{
    PersistentWgcCapture, PersistentWgcFrame, WgcCaptureError, WgcCompositorTiming,
    WgcCompositorTimingUnavailable, WgcFrameMeasurement, WgcPublishedFrameMeasurement,
};
#[cfg(windows)]
pub use windows::{
    UiaSession, activate_window, post_close_window, restore_and_activate_window, set_window_frame,
    snapshot_raw_pointer_input_after_down,
};

#[cfg(not(windows))]
pub struct UiaSession;

#[cfg(not(windows))]
impl UiaSession {
    pub fn new(_target: UiaTarget) -> Self {
        Self
    }

    pub fn snapshot(
        &mut self,
        _max_nodes: u32,
        _max_depth: u32,
        _allow_owned_standard_menu_popup: bool,
    ) -> Result<serde_json::Value, UiaError> {
        Err(UiaError::Unsupported)
    }

    pub fn perform(&mut self, _action: &UiaAction) -> Result<serde_json::Value, UiaError> {
        Err(UiaError::Unsupported)
    }
}

#[cfg(test)]
mod tests;
