use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{CreateEventW, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, IsWindow, IsWindowVisible, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow,
    ShowWindowAsync,
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
const CURSOR_WATCH_ACTIVE_INTERVAL: Duration = Duration::from_millis(10);
const CURSOR_WATCH_IDLE_INTERVAL: Duration = Duration::from_millis(100);

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
                    return Err(IndicatorError::CaptureExclusion(format!(
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

struct OwnedCursorEnumeration {
    hidden_cursor_windows: *mut Vec<OwnedCursorWindow>,
    failed: bool,
}

unsafe extern "system" fn hide_registered_cursor(window: HWND, context: LPARAM) -> BOOL {
    if !unsafe { IsWindowVisible(window) }.as_bool() {
        return BOOL(1);
    }
    let Some(owned) = registered_window(window) else {
        return BOOL(1);
    };
    let enumeration = unsafe { &mut *(context.0 as *mut OwnedCursorEnumeration) };
    let hidden_cursor_windows = unsafe { &mut *enumeration.hidden_cursor_windows };
    if !hidden_cursor_windows.contains(&owned) {
        hidden_cursor_windows.push(owned);
    }
    let requested = unsafe { ShowWindowAsync(window, SW_HIDE) }.as_bool();
    enumeration.failed |= !requested || unsafe { IsWindowVisible(window) }.as_bool();
    BOOL(1)
}

fn hide_registered_cursors(hidden_cursor_windows: &mut Vec<OwnedCursorWindow>) -> bool {
    let mut enumeration = OwnedCursorEnumeration {
        hidden_cursor_windows,
        failed: false,
    };
    let result = unsafe {
        EnumWindows(
            Some(hide_registered_cursor),
            LPARAM(&mut enumeration as *mut OwnedCursorEnumeration as isize),
        )
    };
    result.is_ok() && !enumeration.failed
}

fn synchronize_registered_cursors(requested: bool, cursors: &mut CursorSuppression) {
    let state = state();
    let mut pending_restores = state
        .cursor_gate
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if requested || state.input_cursor_requested.load(Ordering::Acquire) {
        if cursor_restores_acknowledged(&mut pending_restores.pending) {
            let _ = hide_registered_cursors(&mut cursors.0);
        }
    } else {
        restore_cursors_locked(&mut cursors.0, &mut pending_restores);
    }
}

/// The upstream cursor renderer can outlive its last control-banner session.
/// Keep process-local cursor ownership responsive to a peer Host's capture
/// request even while no banner presenter is running. Foreign windows are
/// observed but never hidden by this worker.
pub(crate) fn ensure_cursor_watcher() {
    static WATCHER: OnceLock<bool> = OnceLock::new();
    WATCHER.get_or_init(|| {
        thread::Builder::new()
            .name("dcc-cua-cursor-capture-exclusion".into())
            .spawn(|| {
                let mut cursors = CursorSuppression::default();
                loop {
                    let requested = cross_process_capture_requested();
                    synchronize_registered_cursors(requested, &mut cursors);
                    thread::sleep(
                        if requested || state().input_cursor_requested.load(Ordering::Acquire) {
                            CURSOR_WATCH_ACTIVE_INTERVAL
                        } else {
                            CURSOR_WATCH_IDLE_INTERVAL
                        },
                    );
                }
            })
            .is_ok()
    });
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
            let _ = unsafe { ShowWindowAsync(window, SW_HIDE) };
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
    let mut pending_restores = state()
        .cursor_gate
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if !cursor_restores_acknowledged(&mut pending_restores.pending) {
        return false;
    }
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

struct CursorRestores {
    pending: Vec<OwnedCursorWindow>,
    deferred: Vec<OwnedCursorWindow>,
}

pub(super) struct CaptureExclusionState {
    registration_gate: AtomicBool,
    registered: AtomicUsize,
    suppressed: AtomicBool,
    input_cursor_requested: AtomicBool,
    cursor_gate: std::sync::Mutex<CursorRestores>,
    requested: AtomicU64,
    acknowledged: AtomicUsize,
}

impl CaptureExclusionState {
    const fn new() -> Self {
        Self {
            registration_gate: AtomicBool::new(false),
            registered: AtomicUsize::new(0),
            suppressed: AtomicBool::new(false),
            input_cursor_requested: AtomicBool::new(false),
            cursor_gate: std::sync::Mutex::new(CursorRestores {
                pending: Vec::new(),
                deferred: Vec::new(),
            }),
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
                return Err(IndicatorError::CaptureExclusion(
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
            return Err(IndicatorError::CaptureExclusion(
                "control-banner presenter stopped before DCC-CUA overlay capture exclusion".into(),
            ));
        }
        if Instant::now() >= deadline {
            return Err(IndicatorError::CaptureExclusion(
                "timed out while excluding DCC-CUA overlays from exact-window capture".into(),
            ));
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(guard)
}

// Input owns only this runtime's opaque registered cursor, never capture's
// global presenter suppression or cross-process request. Reuse serialization
// so a capture cannot restore the cursor during native release/postcheck.
pub(crate) struct InputCursorGuard {
    state: &'static CaptureExclusionState,
    cursors: CursorSuppression,
    _registration_gate: RegistrationGate,
    _cross_process_gate: CrossProcessGate,
}

impl Drop for InputCursorGuard {
    fn drop(&mut self) {
        let mut pending_restores = self
            .state
            .cursor_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.state
            .input_cursor_requested
            .store(false, Ordering::Release);
        restore_cursors_locked(&mut self.cursors.0, &mut pending_restores);
    }
}

pub(super) fn begin_input_cursor(active: &AtomicBool) -> Result<InputCursorGuard, IndicatorError> {
    let cross_process_gate = CrossProcessGate::acquire()?;
    let registration_gate = RegistrationGate::acquire()?;
    let state = state();
    let mut guard = InputCursorGuard {
        state,
        cursors: CursorSuppression::default(),
        _registration_gate: registration_gate,
        _cross_process_gate: cross_process_gate,
    };
    let deadline = Instant::now() + CAPTURE_EXCLUSION_TIMEOUT;
    loop {
        let hidden = {
            let mut pending_restores = state
                .cursor_gate
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.input_cursor_requested.store(true, Ordering::Release);
            active.load(Ordering::Acquire)
                && cursor_restores_acknowledged(&mut pending_restores.pending)
                && hide_registered_cursors(&mut guard.cursors.0)
                && active.load(Ordering::Acquire)
        };
        if hidden && Instant::now() < deadline {
            break;
        }
        if !active.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err(IndicatorError::CaptureExclusion(
                "registered cursor input exclusion was not acknowledged".into(),
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
        synchronize_registered_cursors(false, cursors);
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
    let mut pending_restores = state()
        .cursor_gate
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    restore_cursors_locked(windows, &mut pending_restores);
}

fn cursor_restores_acknowledged(pending_restores: &mut Vec<OwnedCursorWindow>) -> bool {
    pending_restores.retain(|owned| {
        let window = HWND(owned.raw as *mut _);
        if !unsafe { IsWindow(Some(window)) }.as_bool() {
            return false;
        }
        match registered_window(window) {
            Some(current) if current != *owned => false,
            Some(_) => !unsafe { IsWindowVisible(window) }.as_bool(),
            // A timed-out identity query does not prove the old show has
            // finished. Retain its lease until a positive ACK/invalidation.
            None => true,
        }
    });
    pending_restores.is_empty()
}

fn restore_cursors_locked(windows: &mut Vec<OwnedCursorWindow>, restores: &mut CursorRestores) {
    cursor_restores_acknowledged(&mut restores.pending);
    for owned in windows.drain(..) {
        if !restores.deferred.contains(&owned) {
            restores.deferred.push(owned);
        }
    }
    if state().input_cursor_requested.load(Ordering::Acquire) {
        return;
    }
    for owned in std::mem::take(&mut restores.deferred) {
        let window = HWND(owned.raw as *mut _);
        if !unsafe { IsWindow(Some(window)) }.as_bool() {
            continue;
        }
        match registered_window(window) {
            Some(current) if current == owned => {
                if !unsafe { IsWindowVisible(window) }.as_bool()
                    && !restores.pending.contains(&owned)
                {
                    // Async show can outlive this lease. Keep its exact
                    // identity until visible ACK before another hide.
                    restores.pending.push(owned);
                    let _ = unsafe { ShowWindowAsync(window, SW_SHOWNOACTIVATE) };
                }
            }
            Some(_) => {}
            None => restores.deferred.push(owned),
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
