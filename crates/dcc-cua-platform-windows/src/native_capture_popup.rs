//! Explicit native mouse-capture companions for an unchanged main-root keyboard focus.
//! Metadata is never permission to capture pixels or inject input by itself.
use serde::Serialize;
use std::mem::size_of;
use windows::Win32::Foundation::{HWND as CaptureHwnd, RECT};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::{
        Input::KeyboardAndMouse::IsWindowEnabled,
        WindowsAndMessaging::{
            GA_ROOT, GUI_CARETBLINKING, GUITHREADINFO, GWL_STYLE, GetAncestor, GetForegroundWindow,
            GetGUIThreadInfo, GetWindowLongPtrW, GetWindowThreadProcessId, IsChild,
            IsWindowVisible, WS_CHILD, WS_POPUP,
        },
    },
};

use crate::{
    ExactWindowNativeState, ExactWindowPixelInstanceEvidence, UiaError, exact_window_native_state,
    occlusion::intersection,
    visible_capture::{
        RootProofDecision, RootProofTraceEntry, ThreadDpiAwarenessGuard,
        VisibleWindowCaptureReason, enumerate_target_root_proof, exact_window_instance_evidence,
        physical_rectangle_within_desktop,
    },
};

const MAX_SELECTED: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeCapturePopupRefusal {
    InvalidSelection,
    RootUnavailable,
    RootNotUsable,
    GuiStateUnavailable,
    FocusOutsideRoot,
    CaptureNotSelected,
    PopupUnavailable,
    PopupIdentityMismatch,
    PopupNotUsable,
    EnumerationIncomplete,
    UnselectedOccluder,
    NativeStateChanged,
    NativeStateUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativeCapturePopupInventory {
    pub process_id: u32,
    pub root_window_handle: u64,
    pub authorizes_input: bool,
    pub complete: bool,
    pub capture_window_handle: Option<u64>,
    pub candidates: Vec<ExactWindowNativeState>,
    pub refusal_reason: Option<NativeCapturePopupRefusal>,
}

/// Only the native reader can construct this type; wire JSON cannot mint it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativeCapturePopupProof {
    pub root: ExactWindowNativeState,
    pub capture: ExactWindowNativeState,
    pub overlays: Vec<ExactWindowNativeState>,
    pub redacted_regions: Vec<[i32; 4]>,
    #[serde(skip)]
    focus: FocusState,
    #[serde(skip)]
    ordered_occluders: Vec<ExactWindowNativeState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FocusState {
    active: u64,
    foreground: u64,
    focus: u64,
    focus_root: u64,
    focus_instance: ExactWindowPixelInstanceEvidence,
    capture: u64,
    flags: u32,
    enabled: bool,
    visible: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PopupState {
    state: ExactWindowNativeState,
    root: u64,
    style: u32,
    enabled: bool,
    owner_is_local_or_absent: bool,
}

#[derive(Clone, Debug)]
struct Sample {
    root: ExactWindowNativeState,
    root_is_root: bool,
    root_enabled: bool,
    focus: FocusState,
    overlays: Vec<PopupState>,
    roots: Vec<RootProofTraceEntry>,
    desktop: [i32; 4],
}

fn refused(reason: NativeCapturePopupRefusal) -> UiaError {
    UiaError::OperationFailed(format!("native capture popup proof refused: {reason:?}"))
}

fn valid_selection(root: u64, selected: &[u64]) -> bool {
    !selected.is_empty()
        && selected.len() <= MAX_SELECTED
        && selected.iter().enumerate().all(|(index, handle)| {
            *handle != 0 && *handle != root && !selected[..index].contains(handle)
        })
}

fn usable(state: &ExactWindowNativeState, desktop: [i32; 4]) -> bool {
    state.visible
        && !state.minimized
        && state.dpi != 0
        && state.instance.process_creation_time_100ns != 0
        && state.instance.window_thread_id != 0
        && state.instance.window_class_hash != 0
        && state
            .bounds
            .is_some_and(|bounds| physical_rectangle_within_desktop(bounds, desktop))
        && state
            .visible_bounds
            .is_some_and(|bounds| physical_rectangle_within_desktop(bounds, desktop))
}

fn root_focus(root: &ExactWindowNativeState, focus: FocusState) -> bool {
    root.foreground
        && focus.foreground == root.window_handle
        && focus.active == root.window_handle
        && focus.focus_root == root.window_handle
        && focus.focus != 0
        && focus.enabled
        && focus.visible
        && focus.flags & !GUI_CARETBLINKING == 0
        && focus.focus_instance.process_creation_time_100ns
            == root.instance.process_creation_time_100ns
        && focus.focus_instance.window_thread_id == root.instance.window_thread_id
        && focus.focus_instance.window_class_hash != 0
}

fn validate_sample(
    process_id: u32,
    root_handle: u64,
    selected: &[u64],
    sample: &Sample,
) -> Result<NativeCapturePopupProof, NativeCapturePopupRefusal> {
    use NativeCapturePopupRefusal::*;
    if !valid_selection(root_handle, selected) {
        return Err(InvalidSelection);
    }
    let root = sample.root;
    if root.process_id != process_id || root.window_handle != root_handle || !sample.root_is_root {
        return Err(RootUnavailable);
    }
    if !sample.root_enabled || !usable(&root, sample.desktop) {
        return Err(RootNotUsable);
    }
    if !root_focus(&root, sample.focus) {
        return Err(FocusOutsideRoot);
    }
    if !selected.contains(&sample.focus.capture) {
        return Err(CaptureNotSelected);
    }
    if sample.overlays.len() != selected.len()
        || sample
            .roots
            .last()
            .is_none_or(|entry| entry.window_handle != root_handle)
    {
        return Err(EnumerationIncomplete);
    }
    let root_entry = sample.roots.last().ok_or(EnumerationIncomplete)?;
    if root_entry.process_id != process_id
        || root_entry.thread_id != root.instance.window_thread_id
        || root_entry.dwm_bounds != root.visible_bounds
        || root_entry.win32_bounds != root.bounds
        || root_entry.dpi != root.dpi
        || !root_entry.visible
        || root_entry.cloaked != Some(0)
        || root_entry.decision != RootProofDecision::TargetReached
    {
        return Err(RootNotUsable);
    }
    if sample.roots.iter().any(|entry| {
        entry.visible
            && (entry.cloaked.is_none()
                || (entry.cloaked == Some(0)
                    && entry.dwm_bounds.is_none()
                    && entry.decision != RootProofDecision::Empty))
    }) {
        return Err(EnumerationIncomplete);
    }
    let bounds = root.visible_bounds.ok_or(RootNotUsable)?;
    let mut overlays = Vec::new();
    for (index, popup) in sample.overlays.iter().enumerate() {
        let state = popup.state;
        if state.window_handle != selected[index]
            || state.process_id != process_id
            || state.instance.process_creation_time_100ns
                != root.instance.process_creation_time_100ns
            || state.instance.window_thread_id != root.instance.window_thread_id
        {
            return Err(PopupIdentityMismatch);
        }
        if popup.root != state.window_handle
            || popup.style & WS_POPUP == 0
            || popup.style & WS_CHILD != 0
            || !popup.enabled
            || !popup.owner_is_local_or_absent
            || !usable(&state, sample.desktop)
        {
            return Err(PopupNotUsable);
        }
        let position = sample
            .roots
            .iter()
            .position(|entry| entry.window_handle == state.window_handle)
            .ok_or(PopupUnavailable)?;
        let entry = &sample.roots[position];
        if !entry.visible
            || entry.cloaked != Some(0)
            || entry.dwm_bounds != state.visible_bounds
            || entry.win32_bounds != state.bounds
            || entry.dpi != state.dpi
            || entry.minimized
            || entry.root_window_handle != state.window_handle
            || entry.process_id != process_id
            || entry.thread_id != state.instance.window_thread_id
            || state
                .visible_bounds
                .and_then(|popup| intersection(bounds, popup))
                .is_none()
        {
            return Err(PopupNotUsable);
        }
        // Each selected popup's entire visible frame must also be free of every
        // foreign or unselected higher root, including pixels outside main.
        for higher in &sample.roots[..position] {
            if higher.visible
                && higher.cloaked == Some(0)
                && higher
                    .dwm_bounds
                    .zip(state.visible_bounds)
                    .is_some_and(|(a, b)| intersection(a, b).is_some())
                && !selected.contains(&higher.window_handle)
            {
                return Err(UnselectedOccluder);
            }
        }
        overlays.push(state);
    }
    let mut ordered_occluders = Vec::new();
    let mut redacted_regions = Vec::new();
    for entry in &sample.roots[..sample.roots.len() - 1] {
        if entry.visible
            && entry.cloaked == Some(0)
            && let Some(region) = entry.dwm_bounds.and_then(|rect| intersection(bounds, rect))
        {
            let state = overlays
                .iter()
                .find(|state| state.window_handle == entry.window_handle)
                .ok_or(UnselectedOccluder)?;
            ordered_occluders.push(*state);
            redacted_regions.push(region);
        }
    }
    if ordered_occluders.len() != selected.len() {
        return Err(EnumerationIncomplete);
    }
    redacted_regions.sort_unstable();
    redacted_regions.dedup();
    let capture = *overlays
        .iter()
        .find(|state| state.window_handle == sample.focus.capture)
        .ok_or(CaptureNotSelected)?;
    Ok(NativeCapturePopupProof {
        root,
        capture,
        overlays,
        redacted_regions,
        focus: sample.focus,
        ordered_occluders,
    })
}

fn hwnd(handle: u64) -> HWND {
    handle as usize as HWND
}

fn read_focus(root: &ExactWindowNativeState) -> Result<FocusState, UiaError> {
    let mut gui = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetGUIThreadInfo(root.instance.window_thread_id, &mut gui) } == 0
        || gui.hwndFocus.is_null()
    {
        return Err(refused(NativeCapturePopupRefusal::GuiStateUnavailable));
    }
    let focus = gui.hwndFocus as usize as u64;
    let focus_instance = exact_window_instance_evidence(root.process_id, focus)
        .map_err(|_| refused(NativeCapturePopupRefusal::FocusOutsideRoot))?;
    if gui.hwndFocus != hwnd(root.window_handle)
        && unsafe { IsChild(hwnd(root.window_handle), gui.hwndFocus) } == 0
    {
        return Err(refused(NativeCapturePopupRefusal::FocusOutsideRoot));
    }
    Ok(FocusState {
        active: gui.hwndActive as usize as u64,
        foreground: unsafe { GetForegroundWindow() } as usize as u64,
        focus,
        focus_root: unsafe { GetAncestor(gui.hwndFocus, GA_ROOT) } as usize as u64,
        focus_instance,
        capture: gui.hwndCapture as usize as u64,
        flags: gui.flags,
        enabled: unsafe { IsWindowEnabled(gui.hwndFocus) } != 0,
        visible: unsafe { IsWindowVisible(gui.hwndFocus) } != 0,
    })
}

fn read_popup(process_id: u32, handle: u64) -> Result<PopupState, UiaError> {
    let state = exact_window_native_state(process_id, handle)?;
    let owner = state.instance.owner_window_handle;
    let mut owner_pid = 0;
    let owner_is_local_or_absent = owner == 0
        || (unsafe { GetWindowThreadProcessId(hwnd(owner), &mut owner_pid) } != 0
            && owner_pid == process_id);
    Ok(PopupState {
        state,
        root: unsafe { GetAncestor(hwnd(handle), GA_ROOT) } as usize as u64,
        style: unsafe { GetWindowLongPtrW(hwnd(handle), GWL_STYLE) } as u32,
        enabled: unsafe { IsWindowEnabled(hwnd(handle)) } != 0,
        owner_is_local_or_absent,
    })
}

fn read_sample(process_id: u32, root_handle: u64, selected: &[u64]) -> Result<Sample, UiaError> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };
    let root = exact_window_native_state(process_id, root_handle)?;
    let bounds = root
        .visible_bounds
        .ok_or_else(|| refused(NativeCapturePopupRefusal::RootUnavailable))?;
    let right = bounds[0]
        .checked_add(bounds[2])
        .ok_or_else(|| refused(NativeCapturePopupRefusal::RootNotUsable))?;
    let bottom = bounds[1]
        .checked_add(bounds[3])
        .ok_or_else(|| refused(NativeCapturePopupRefusal::RootNotUsable))?;
    let focus = read_focus(&root)?;
    let rect = RECT {
        left: bounds[0],
        top: bounds[1],
        right,
        bottom,
    };
    let (result, roots) =
        unsafe { enumerate_target_root_proof(CaptureHwnd(hwnd(root_handle)), rect, true) };
    if result
        .as_ref()
        .is_err_and(|error| error.diagnostic.reason != VisibleWindowCaptureReason::RootOverlap)
        || roots
            .last()
            .is_none_or(|entry| entry.window_handle != root_handle)
        || roots.iter().any(|entry| {
            matches!(
                entry.decision,
                RootProofDecision::RejectBounds
                    | RootProofDecision::RejectBoundsQuery
                    | RootProofDecision::RejectCloakingQuery
            )
        })
    {
        return Err(refused(NativeCapturePopupRefusal::EnumerationIncomplete));
    }
    let overlays = selected
        .iter()
        .map(|handle| read_popup(process_id, *handle))
        .collect::<Result<Vec<_>, _>>()?;
    let sample = Sample {
        root,
        root_is_root: unsafe { GetAncestor(hwnd(root_handle), GA_ROOT) } == hwnd(root_handle),
        root_enabled: unsafe { IsWindowEnabled(hwnd(root_handle)) } != 0,
        focus,
        overlays,
        roots,
        desktop: unsafe {
            [
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            ]
        },
    };
    if exact_window_native_state(process_id, root_handle)? != root || read_focus(&root)? != focus {
        return Err(refused(NativeCapturePopupRefusal::NativeStateChanged));
    }
    Ok(sample)
}

/// Metadata only. The caller must separately acquire and verify each actual
/// selected popup image and prove their union covers every root redaction.
pub fn read_native_capture_popup_proof(
    process_id: u32,
    root: u64,
    selected: &[u64],
) -> Result<NativeCapturePopupProof, UiaError> {
    if !valid_selection(root, selected) {
        return Err(refused(NativeCapturePopupRefusal::InvalidSelection));
    }
    let _dpi = ThreadDpiAwarenessGuard::per_monitor_v2()
        .map_err(|_| refused(NativeCapturePopupRefusal::RootUnavailable))?;
    let before = validate_sample(
        process_id,
        root,
        selected,
        &read_sample(process_id, root, selected)?,
    )
    .map_err(refused)?;
    let after = validate_sample(
        process_id,
        root,
        selected,
        &read_sample(process_id, root, selected)?,
    )
    .map_err(refused)?;
    if before != after {
        return Err(refused(NativeCapturePopupRefusal::NativeStateChanged));
    }
    Ok(after)
}

/// Bounded content-free candidate IDs. Never selects, follows, captures or
/// authorizes a popup; a caller must explicitly request every chosen HWND.
pub fn native_capture_popup_inventory(process_id: u32, root: u64) -> NativeCapturePopupInventory {
    let mut result = NativeCapturePopupInventory {
        process_id,
        root_window_handle: root,
        authorizes_input: false,
        complete: false,
        capture_window_handle: None,
        candidates: Vec::new(),
        refusal_reason: None,
    };
    let read = || -> Result<_, UiaError> {
        let _dpi = ThreadDpiAwarenessGuard::per_monitor_v2()
            .map_err(|_| refused(NativeCapturePopupRefusal::RootUnavailable))?;
        let sample = read_sample(process_id, root, &[])?;
        let mut candidates = Vec::new();
        for entry in &sample.roots {
            if entry.window_handle == root
                || entry.process_id != process_id
                || entry.thread_id != sample.root.instance.window_thread_id
                || !entry.visible
                || entry.cloaked != Some(0)
                || entry
                    .dwm_bounds
                    .zip(sample.root.visible_bounds)
                    .is_none_or(|(a, b)| intersection(a, b).is_none())
            {
                continue;
            }
            let popup = read_popup(process_id, entry.window_handle)?;
            if popup.root == entry.window_handle
                && popup.style & WS_POPUP != 0
                && popup.style & WS_CHILD == 0
                && popup.owner_is_local_or_absent
                && usable(&popup.state, sample.desktop)
                && popup.state.instance.process_creation_time_100ns
                    == sample.root.instance.process_creation_time_100ns
            {
                candidates.push(popup.state);
            }
            if candidates.len() > MAX_SELECTED {
                return Err(refused(NativeCapturePopupRefusal::InvalidSelection));
            }
        }
        if exact_window_native_state(process_id, root)? != sample.root
            || read_focus(&sample.root)? != sample.focus
        {
            return Err(refused(NativeCapturePopupRefusal::NativeStateChanged));
        }
        let capture = candidates
            .iter()
            .find(|window| window.window_handle == sample.focus.capture)
            .map(|window| window.window_handle);
        Ok((candidates, capture))
    };
    match read() {
        Ok((candidates, capture)) => {
            result.candidates = candidates;
            result.capture_window_handle = capture;
            result.complete = true;
        }
        Err(_) => result.refusal_reason = Some(NativeCapturePopupRefusal::NativeStateUnavailable),
    }
    result
}

pub(crate) fn native_capture_ended_with_root_focus(
    process_id: u32,
    root: u64,
) -> Result<bool, UiaError> {
    let state = exact_window_native_state(process_id, root)?;
    let focus = read_focus(&state)?;
    Ok(focus.capture == 0
        && root_focus(&state, focus)
        && exact_window_native_state(process_id, root)? == state
        && read_focus(&state)? == focus)
}

#[cfg(test)]
mod tests;
