//! Content-free native focus/ownership diagnostics, never input authority.
//!
//! A GUI thread may keep its main window foreground while a transient owned
//! popup receives keyboard focus. This query records that relationship without
//! activating, capturing, following, or sending input to either window.
use std::mem::size_of;

use serde::Serialize;
use windows_sys::Win32::{
    Foundation::HWND,
    UI::{
        Input::KeyboardAndMouse::IsWindowEnabled,
        WindowsAndMessaging::{
            GA_ROOT, GA_ROOTOWNER, GUITHREADINFO, GWL_STYLE, GetAncestor, GetForegroundWindow,
            GetGUIThreadInfo, GetWindowLongPtrW, GetWindowThreadProcessId, IsChild, IsIconic,
            IsWindow, IsWindowVisible, WS_CHILD, WS_POPUP,
        },
    },
};

use crate::visible_capture::{ExactWindowPixelInstanceEvidence, exact_window_instance_evidence};

const MAX_OWNER_CHAIN: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativeFocusedPopupDiagnostic {
    pub schema_version: u32,
    pub process_id: u32,
    pub bound_window_handle: u64,
    /// Compare with the independently read exact window state's instance.
    pub bound_root_native_instance: Option<ExactWindowPixelInstanceEvidence>,
    /// Even a stable, proven relationship cannot authorize or retarget input.
    pub authorizes_input: bool,
    pub stable: bool,
    pub proven: bool,
    pub refusal_reason: Option<NativeFocusedPopupRefusal>,
    pub before: NativeFocusedPopupSample,
    pub after: NativeFocusedPopupSample,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeFocusedPopupRefusal {
    InvalidTarget,
    BoundRootUnavailable,
    BoundWindowNotRoot,
    BoundRootNotUsable,
    GuiThreadUnavailable,
    ForegroundOutsideBoundRoots,
    FocusUnavailable,
    FocusNotPopup,
    FocusOutsidePopup,
    PopupNotUsable,
    ProcessInstanceMismatch,
    ThreadMismatch,
    RootOwnerMismatch,
    OwnerChainUnproven,
    ActiveWindowOutsideBoundRoots,
    CaptureOutsideBoundRoots,
    NativeStateChanged,
}

/// No titles, text, accessibility data, or foreign window identities.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct NativeFocusedPopupSample {
    pub bound_root: NativePopupWindowRelation,
    pub gui_thread_flags: Option<u32>,
    pub foreground: NativePopupWindowRelation,
    pub active: NativePopupWindowRelation,
    pub focus: NativePopupWindowRelation,
    pub focus_root: NativePopupWindowRelation,
    pub focus_root_owner: NativePopupWindowRelation,
    pub focus_is_root_or_child: Option<bool>,
    pub capture: NativePopupWindowRelation,
    pub capture_root: NativePopupWindowRelation,
    pub owner_chain: Vec<NativePopupWindow>,
    pub owner_chain_status: NativePopupOwnerChainStatus,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "relation", rename_all = "snake_case")]
pub enum NativePopupWindowRelation {
    #[default]
    None,
    Unavailable,
    ForeignProcess,
    ForeignOwner,
    SameProcess {
        window: NativePopupWindow,
    },
}

impl NativePopupWindowRelation {
    fn window(&self) -> Option<&NativePopupWindow> {
        match self {
            Self::SameProcess { window } => Some(window),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativePopupWindow {
    pub window_handle: u64,
    pub instance: ExactWindowPixelInstanceEvidence,
    pub is_root: bool,
    pub style: u32,
    pub visible: bool,
    pub enabled: bool,
    pub minimized: bool,
    /// None means DWM did not supply a verifiable cloaking state.
    pub cloaked: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePopupOwnerChainStatus {
    #[default]
    NotRead,
    ReachedBoundRoot,
    NoOwner,
    Unavailable,
    ForeignProcess,
    ForeignOwner,
    Cycle,
    LimitExceeded,
}

fn same_identity(left: &NativePopupWindow, right: &NativePopupWindow) -> bool {
    left.window_handle == right.window_handle && left.instance == right.instance
}

fn usable(window: &NativePopupWindow) -> bool {
    window.visible && window.enabled && !window.minimized && window.cloaked == Some(0)
}

/// Validate facts only. The returned relationship is not a capability or a
/// substitute for explicit task grants, observations, or the input fence.
fn validate_sample(
    process_id: u32,
    window_handle: u64,
    sample: &NativeFocusedPopupSample,
) -> Result<(), NativeFocusedPopupRefusal> {
    use NativeFocusedPopupRefusal::*;
    if process_id == 0 || window_handle == 0 {
        return Err(InvalidTarget);
    }
    let bound = sample.bound_root.window().ok_or(BoundRootUnavailable)?;
    if bound.window_handle != window_handle || !bound.is_root {
        return Err(BoundWindowNotRoot);
    }
    if !usable(bound) {
        return Err(BoundRootNotUsable);
    }
    if sample.gui_thread_flags.is_none() {
        return Err(GuiThreadUnavailable);
    }
    let focus = sample.focus.window().ok_or(FocusUnavailable)?;
    let popup = sample.focus_root.window().ok_or(FocusNotPopup)?;
    if popup.window_handle == bound.window_handle
        || !popup.is_root
        || popup.style & WS_POPUP == 0
        || popup.style & WS_CHILD != 0
    {
        return Err(FocusNotPopup);
    }
    if sample.focus_is_root_or_child != Some(true) {
        return Err(FocusOutsidePopup);
    }
    if !usable(popup) || !focus.visible || !focus.enabled {
        return Err(PopupNotUsable);
    }
    let expected = bound.instance;
    if expected.process_creation_time_100ns == 0
        || expected.window_thread_id == 0
        || expected.window_class_hash == 0
    {
        return Err(BoundRootUnavailable);
    }
    // Every reported same-process relationship must belong to this exact
    // native process birth and GUI thread, including intermediary owners.
    for window in [
        &sample.foreground,
        &sample.active,
        &sample.focus,
        &sample.focus_root,
        &sample.focus_root_owner,
        &sample.capture,
        &sample.capture_root,
    ]
    .into_iter()
    .filter_map(NativePopupWindowRelation::window)
    .chain(sample.owner_chain.iter())
    {
        if window.instance.process_creation_time_100ns != expected.process_creation_time_100ns
            || window.instance.window_class_hash == 0
        {
            return Err(ProcessInstanceMismatch);
        }
        if window.instance.window_thread_id != expected.window_thread_id {
            return Err(ThreadMismatch);
        }
    }
    let is_bound_or_popup = |relation: &NativePopupWindowRelation| {
        relation
            .window()
            .is_some_and(|window| same_identity(window, bound) || same_identity(window, popup))
    };
    if !is_bound_or_popup(&sample.foreground) {
        return Err(ForegroundOutsideBoundRoots);
    }
    if !is_bound_or_popup(&sample.active) {
        return Err(ActiveWindowOutsideBoundRoots);
    }
    if !sample
        .focus_root_owner
        .window()
        .is_some_and(|window| same_identity(window, bound))
    {
        return Err(RootOwnerMismatch);
    }
    if sample.owner_chain_status != NativePopupOwnerChainStatus::ReachedBoundRoot
        || sample.owner_chain.is_empty()
        || sample.owner_chain.len() > MAX_OWNER_CHAIN
    {
        return Err(OwnerChainUnproven);
    }
    let mut previous = popup;
    let mut visited = vec![popup.window_handle];
    for owner in &sample.owner_chain {
        if owner.window_handle != previous.instance.owner_window_handle
            || visited.contains(&owner.window_handle)
        {
            return Err(OwnerChainUnproven);
        }
        visited.push(owner.window_handle);
        previous = owner;
    }
    if !same_identity(previous, bound) {
        return Err(OwnerChainUnproven);
    }
    if !matches!(sample.capture, NativePopupWindowRelation::None)
        && (sample.capture.window().is_none() || !is_bound_or_popup(&sample.capture_root))
    {
        return Err(CaptureOutsideBoundRoots);
    }
    Ok(())
}

fn diagnostic_from_reads(
    process_id: u32,
    window_handle: u64,
    mut read: impl FnMut() -> NativeFocusedPopupSample,
) -> NativeFocusedPopupDiagnostic {
    let before = read();
    let after = read();
    let stable = before == after;
    let refusal_reason = if stable {
        validate_sample(process_id, window_handle, &before).err()
    } else {
        Some(NativeFocusedPopupRefusal::NativeStateChanged)
    };
    NativeFocusedPopupDiagnostic {
        schema_version: 1,
        process_id,
        bound_window_handle: window_handle,
        bound_root_native_instance: before.bound_root.window().map(|window| window.instance),
        authorizes_input: false,
        stable,
        proven: refusal_reason.is_none(),
        refusal_reason,
        before,
        after,
    }
}

fn hwnd(raw: u64) -> HWND {
    raw as usize as HWND
}

fn own_window(process_id: u32, window: HWND) -> bool {
    let mut observed_pid = 0;
    !window.is_null()
        && unsafe { IsWindow(window) } != 0
        && unsafe { GetWindowThreadProcessId(window, &mut observed_pid) } != 0
        && observed_pid == process_id
}

fn owner_refusal(process_id: u32, owner: u64) -> Option<NativePopupWindowRelation> {
    if owner == 0 {
        return None;
    }
    let mut observed_pid = 0;
    if unsafe { IsWindow(hwnd(owner)) } == 0
        || unsafe { GetWindowThreadProcessId(hwnd(owner), &mut observed_pid) } == 0
    {
        return Some(NativePopupWindowRelation::Unavailable);
    }
    (observed_pid != process_id).then_some(NativePopupWindowRelation::ForeignOwner)
}

fn read_window(process_id: u32, raw: HWND) -> NativePopupWindowRelation {
    use NativePopupWindowRelation::*;
    if raw.is_null() {
        return None;
    }
    let mut observed_pid = 0;
    if unsafe { IsWindow(raw) } == 0
        || unsafe { GetWindowThreadProcessId(raw, &mut observed_pid) } == 0
    {
        return Unavailable;
    }
    if observed_pid != process_id {
        return ForeignProcess;
    }
    let window_handle = raw as usize as u64;
    let Ok(before) = exact_window_instance_evidence(process_id, window_handle) else {
        return Unavailable;
    };
    // The existing native instance contains GW_OWNER. Never serialize that
    // handle when it identifies a foreign process, including on failed proof.
    if let Some(refusal) = owner_refusal(process_id, before.owner_window_handle) {
        return refusal;
    }
    let mut cloaked = 0_u32;
    let cloak_ok = unsafe {
        windows::Win32::Graphics::Dwm::DwmGetWindowAttribute(
            windows::Win32::Foundation::HWND(raw),
            windows::Win32::Graphics::Dwm::DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            size_of::<u32>() as u32,
        )
    }
    .is_ok();
    let window = NativePopupWindow {
        window_handle,
        instance: before,
        is_root: unsafe { GetAncestor(raw, GA_ROOT) } == raw,
        style: unsafe { GetWindowLongPtrW(raw, GWL_STYLE) } as u32,
        visible: unsafe { IsWindowVisible(raw) } != 0,
        enabled: unsafe { IsWindowEnabled(raw) } != 0,
        minimized: unsafe { IsIconic(raw) } != 0,
        cloaked: cloak_ok.then_some(cloaked),
    };
    let Ok(after) = exact_window_instance_evidence(process_id, window_handle) else {
        return Unavailable;
    };
    if before != after || !own_window(process_id, raw) {
        return Unavailable;
    }
    if let Some(refusal) = owner_refusal(process_id, after.owner_window_handle) {
        return refusal;
    }
    SameProcess { window }
}

fn read_owner_chain(
    process_id: u32,
    bound: u64,
    popup: &NativePopupWindow,
) -> (Vec<NativePopupWindow>, NativePopupOwnerChainStatus) {
    use NativePopupOwnerChainStatus::*;
    let mut chain = Vec::new();
    let mut visited = vec![popup.window_handle];
    let mut next = popup.instance.owner_window_handle;
    for _ in 0..MAX_OWNER_CHAIN {
        if next == 0 {
            return (chain, NoOwner);
        }
        if visited.contains(&next) {
            return (chain, Cycle);
        }
        match read_window(process_id, hwnd(next)) {
            NativePopupWindowRelation::SameProcess { window } => {
                visited.push(next);
                let reached = window.window_handle == bound;
                next = window.instance.owner_window_handle;
                chain.push(window);
                if reached {
                    return (chain, ReachedBoundRoot);
                }
            }
            NativePopupWindowRelation::ForeignProcess => return (chain, ForeignProcess),
            NativePopupWindowRelation::ForeignOwner => return (chain, ForeignOwner),
            _ => return (chain, Unavailable),
        }
    }
    (chain, LimitExceeded)
}

fn read_sample(process_id: u32, window_handle: u64) -> NativeFocusedPopupSample {
    let mut sample = NativeFocusedPopupSample::default();
    if process_id == 0 || window_handle == 0 {
        return sample;
    }
    sample.bound_root = read_window(process_id, hwnd(window_handle));
    let Some(bound) = sample.bound_root.window() else {
        return sample;
    };
    let mut info = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetGUIThreadInfo(bound.instance.window_thread_id, &mut info) } == 0 {
        return sample;
    }
    sample.gui_thread_flags = Some(info.flags);
    sample.foreground = read_window(process_id, unsafe { GetForegroundWindow() });
    sample.active = read_window(process_id, info.hwndActive);
    sample.focus = read_window(process_id, info.hwndFocus);
    sample.capture = read_window(process_id, info.hwndCapture);
    if sample.capture.window().is_some() {
        sample.capture_root = read_window(process_id, unsafe {
            GetAncestor(info.hwndCapture, GA_ROOT)
        });
    }
    if sample.focus.window().is_some() {
        let root = unsafe { GetAncestor(info.hwndFocus, GA_ROOT) };
        sample.focus_root = read_window(process_id, root);
        if let Some(popup) = sample.focus_root.window() {
            sample.focus_is_root_or_child =
                Some(root == info.hwndFocus || unsafe { IsChild(root, info.hwndFocus) } != 0);
            sample.focus_root_owner = read_window(process_id, unsafe {
                GetAncestor(info.hwndFocus, GA_ROOTOWNER)
            });
            (sample.owner_chain, sample.owner_chain_status) =
                read_owner_chain(process_id, window_handle, popup);
        }
    }
    sample
}

/// Two fresh native reads of the bound root's GUI thread. This query performs
/// no UIA/pixel observation, activation, thread-input attachment, or input. A
/// proven relationship is diagnostic only and cannot bypass an exact-root fence.
pub fn native_focused_popup_diagnostic(
    process_id: u32,
    window_handle: u64,
) -> NativeFocusedPopupDiagnostic {
    diagnostic_from_reads(process_id, window_handle, || {
        read_sample(process_id, window_handle)
    })
}

#[cfg(test)]
mod tests;
