use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{CreateEventW, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, IsWindowVisible, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow,
};
use windows::core::{BOOL, w};

use super::OverlayWindow;
use super::cursor_registration::{OwnedCursorWindow, registered_window};
use crate::IndicatorError;

const CAPTURE_EXCLUSION_TIMEOUT: Duration = Duration::from_secs(1);
const CAPTURE_REQUEST_NAME: windows::core::PCWSTR =
    w!("Local\\DccCuaControlBannerCaptureExclusionV1");
const BANNER_CLASS_NAME: &str = "DccCuaControlBanner";
const FRAME_CLASS_NAME: &str = "DccCuaControlFrame";
const CURSOR_CLASS_NAME: &str = "Cua.AgentCursorOverlay";
const CROSS_PROCESS_GATE_FILE: &str = "dcc-cua-control-banner-capture-v1.lock";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DccCuaOverlayKind {
    Presenter,
    AgentCursor,
}

pub(crate) fn dcc_cua_overlay_kind(class_name: &str) -> Option<DccCuaOverlayKind> {
    match class_name {
        BANNER_CLASS_NAME | FRAME_CLASS_NAME => Some(DccCuaOverlayKind::Presenter),
        CURSOR_CLASS_NAME => Some(DccCuaOverlayKind::AgentCursor),
        _ => None,
    }
}

/// A crash-safe, cross-process lease serializing banner registration with
/// exact-window capture. Windows closes the exclusive file handle when a Host
/// exits, unlike a semaphore whose count can remain consumed after a crash.
pub(super) struct CrossProcessGate {
    _file: File,
}

impl CrossProcessGate {
    pub(super) fn acquire() -> Result<Self, IndicatorError> {
        let lock_path = std::env::temp_dir().join(CROSS_PROCESS_GATE_FILE);
        let deadline = Instant::now() + CAPTURE_EXCLUSION_TIMEOUT;
        loop {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .share_mode(0)
                .open(&lock_path)
            {
                Ok(file) => return Ok(Self { _file: file }),
                Err(error) if Instant::now() < deadline => {
                    let _ = error;
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => {
                    let kind = error.kind();
                    return Err(IndicatorError::Backend(format!(
                        "timed out while serializing cross-process DCC-CUA overlay capture exclusion ({kind:?})"
                    )));
                }
            }
        }
    }
}

struct CrossProcessCaptureRequest(usize);

impl CrossProcessCaptureRequest {
    fn begin() -> Result<Self, IndicatorError> {
        let event =
            unsafe { CreateEventW(None, true, false, CAPTURE_REQUEST_NAME) }.map_err(|error| {
                IndicatorError::Backend(format!(
                    "create cross-process control-banner capture request: {error}"
                ))
            })?;
        Ok(Self(event.0 as usize))
    }
}

impl Drop for CrossProcessCaptureRequest {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(windows::Win32::Foundation::HANDLE(self.0 as *mut _)) };
    }
}

fn cross_process_capture_requested() -> bool {
    let Ok(event) =
        (unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, CAPTURE_REQUEST_NAME) })
    else {
        return false;
    };
    let _ = unsafe { CloseHandle(event) };
    true
}

struct VisibleOverlayEnumeration {
    visible_overlay_found: bool,
    failed: bool,
    hidden_cursor_windows: *mut Vec<OwnedCursorWindow>,
}

unsafe extern "system" fn find_visible_overlay(window: HWND, context: LPARAM) -> BOOL {
    let enumeration = unsafe { &mut *(context.0 as *mut VisibleOverlayEnumeration) };
    let mut class_name = [0_u16; 128];
    let length = unsafe { GetClassNameW(window, &mut class_name) };
    if length == 0 {
        enumeration.failed = true;
        return BOOL(0);
    }
    let class_name = String::from_utf16_lossy(&class_name[..length as usize]);
    if unsafe { IsWindowVisible(window) }.as_bool() {
        if dcc_cua_overlay_kind(&class_name) == Some(DccCuaOverlayKind::AgentCursor) {
            let Some(owned) = registered_window(window) else {
                enumeration.visible_overlay_found = true;
                return BOOL(1);
            };
            let hidden_cursor_windows = unsafe { &mut *enumeration.hidden_cursor_windows };
            if !hidden_cursor_windows.contains(&owned) {
                hidden_cursor_windows.push(owned);
            }
            let _ = unsafe { ShowWindow(window, SW_HIDE) };
            if unsafe { IsWindowVisible(window) }.as_bool() {
                enumeration.visible_overlay_found = true;
            }
        } else if dcc_cua_overlay_kind(&class_name) == Some(DccCuaOverlayKind::Presenter) {
            enumeration.visible_overlay_found = true;
        }
    }
    BOOL(1)
}

fn hide_cursor_overlays_and_confirm_all_hidden(
    hidden_cursor_windows: &mut Vec<OwnedCursorWindow>,
) -> bool {
    let mut enumeration = VisibleOverlayEnumeration {
        visible_overlay_found: false,
        failed: false,
        hidden_cursor_windows,
    };
    let result = unsafe {
        EnumWindows(
            Some(find_visible_overlay),
            LPARAM(&mut enumeration as *mut VisibleOverlayEnumeration as isize),
        )
    };
    !enumeration.failed && !enumeration.visible_overlay_found && result.is_ok()
}

pub(super) struct CaptureExclusionState {
    registration_gate: AtomicBool,
    registered: AtomicUsize,
    suppressed: AtomicBool,
    requested: AtomicU64,
    acknowledged: AtomicUsize,
}

impl CaptureExclusionState {
    const fn new() -> Self {
        Self {
            registration_gate: AtomicBool::new(false),
            registered: AtomicUsize::new(0),
            suppressed: AtomicBool::new(false),
            requested: AtomicU64::new(0),
            acknowledged: AtomicUsize::new(0),
        }
    }
}

pub(super) fn state() -> &'static CaptureExclusionState {
    static STATE: OnceLock<CaptureExclusionState> = OnceLock::new();
    STATE.get_or_init(CaptureExclusionState::new)
}

pub(super) struct Registration(&'static CaptureExclusionState);

impl Registration {
    pub(super) fn begin(state: &'static CaptureExclusionState) -> Self {
        state.registered.fetch_add(1, Ordering::AcqRel);
        Self(state)
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.0.registered.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) struct RegistrationGate(&'static CaptureExclusionState);

impl RegistrationGate {
    pub(super) fn acquire() -> Result<Self, IndicatorError> {
        let state = state();
        let deadline = Instant::now() + CAPTURE_EXCLUSION_TIMEOUT;
        while state
            .registration_gate
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            if Instant::now() >= deadline {
                return Err(IndicatorError::Backend(
                    "timed out while serializing control-banner capture exclusion".into(),
                ));
            }
            thread::sleep(Duration::from_millis(1));
        }
        Ok(Self(state))
    }
}

impl Drop for RegistrationGate {
    fn drop(&mut self) {
        self.0.registration_gate.store(false, Ordering::Release);
    }
}

pub(crate) struct Guard {
    state: &'static CaptureExclusionState,
    hidden_cursor_windows: Vec<OwnedCursorWindow>,
    _registration_gate: RegistrationGate,
    _cross_process_request: CrossProcessCaptureRequest,
    _cross_process_gate: CrossProcessGate,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.state.suppressed.store(false, Ordering::Release);
        restore_cursors(&mut self.hidden_cursor_windows);
    }
}

pub(super) fn begin(active: &AtomicBool) -> Result<Guard, IndicatorError> {
    let state = state();
    let cross_process_gate = CrossProcessGate::acquire()?;
    let registration_gate = RegistrationGate::acquire()?;
    let cross_process_request = CrossProcessCaptureRequest::begin()?;
    // Own all restoration before publishing suppression or hiding any cursor.
    // Every early return below drops this owner while its serialization gates
    // still prevent a concurrent exclusion/registration.
    let mut guard = Guard {
        state,
        hidden_cursor_windows: Vec::new(),
        _registration_gate: registration_gate,
        _cross_process_request: cross_process_request,
        _cross_process_gate: cross_process_gate,
    };
    state.requested.fetch_add(1, Ordering::AcqRel);
    state.acknowledged.store(0, Ordering::Release);
    state.suppressed.store(true, Ordering::Release);
    let deadline = Instant::now() + CAPTURE_EXCLUSION_TIMEOUT;
    loop {
        let registered = state.registered.load(Ordering::Acquire);
        let acknowledged = state.acknowledged.load(Ordering::Acquire);
        if acknowledged >= registered
            && hide_cursor_overlays_and_confirm_all_hidden(&mut guard.hidden_cursor_windows)
        {
            break;
        }
        if !active.load(Ordering::Acquire) {
            return Err(IndicatorError::Backend(
                "control-banner presenter stopped before DCC-CUA overlay capture exclusion".into(),
            ));
        }
        if Instant::now() >= deadline {
            return Err(IndicatorError::Backend(
                "timed out while excluding DCC-CUA overlays from exact-window capture".into(),
            ));
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(guard)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn hide_and_acknowledge(
    state: &CaptureExclusionState,
    overlay: HWND,
    frames: &[OverlayWindow],
    visible: &AtomicBool,
    frame_visible: &AtomicBool,
    acknowledged_request: &mut u64,
    cursors: &mut CursorSuppression,
) -> bool {
    if !state.suppressed.load(Ordering::Acquire) && !cross_process_capture_requested() {
        restore_cursors(&mut cursors.0);
        return false;
    }
    // Each peer cooperates only on its own registered renderer. Enumeration
    // may find an unknown/visible peer; the capturing process then refuses.
    hide_cursor_overlays_and_confirm_all_hidden(&mut cursors.0);
    let requested = state.requested.load(Ordering::Acquire);
    let _ = unsafe { ShowWindow(overlay, SW_HIDE) };
    for frame in frames {
        let _ = unsafe { ShowWindow(frame.0, SW_HIDE) };
    }
    visible.store(false, Ordering::Release);
    frame_visible.store(false, Ordering::Release);
    if !unsafe { IsWindowVisible(overlay) }.as_bool()
        && frames
            .iter()
            .all(|frame| !unsafe { IsWindowVisible(frame.0) }.as_bool())
        && *acknowledged_request != requested
    {
        state.acknowledged.fetch_add(1, Ordering::AcqRel);
        *acknowledged_request = requested;
    }
    true
}

fn restore_cursors(windows: &mut Vec<OwnedCursorWindow>) {
    for owned in windows.drain(..).rev() {
        if owned.still_registered() {
            let _ = unsafe { ShowWindow(HWND(owned.raw as *mut _), SW_SHOWNOACTIVATE) };
        }
    }
}

#[derive(Default)]
pub(super) struct CursorSuppression(Vec<OwnedCursorWindow>);

impl Drop for CursorSuppression {
    fn drop(&mut self) {
        restore_cursors(&mut self.0);
    }
}
