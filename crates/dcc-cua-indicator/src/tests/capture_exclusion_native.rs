//! In-memory Win32/synchronization boundary. The production enumeration,
//! begin, acknowledgement and Drop paths run unchanged; no desktop API runs.
#![allow(non_snake_case)]

use rstest::rstest;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

#[repr(transparent)]
#[derive(Clone, Copy)]
pub(crate) struct HWND(pub(crate) *mut core::ffi::c_void);
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(crate) struct LPARAM(pub(crate) isize);
#[repr(transparent)]
pub(crate) struct WPARAM(pub(crate) usize);
#[repr(transparent)]
pub(crate) struct LRESULT(pub(crate) isize);
pub(crate) const SMTO_ABORTIFHUNG: u32 = 2;
pub(crate) const SMTO_BLOCK: u32 = 1;
pub(crate) const WM_GETTEXT: u32 = 13;
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(crate) struct BOOL(pub(crate) i32);
impl BOOL {
    pub(crate) fn as_bool(self) -> bool {
        self.0 != 0
    }
}
#[derive(Debug)]
pub(crate) enum IndicatorError {
    Backend(String),
}
pub(crate) struct OverlayWindow(pub(crate) HWND);

#[derive(Clone)]
pub(crate) struct Window {
    pub(crate) raw: usize,
    pub(crate) pid: u32,
    pub(crate) class: &'static str,
    pub(crate) title: String,
    pub(crate) visible: bool,
}

pub(crate) static WINDOWS: Mutex<Vec<Window>> = Mutex::new(Vec::new());
pub(crate) static SERIAL: Mutex<()> = Mutex::new(());
pub(crate) static ACTIVE: AtomicBool = AtomicBool::new(true);
pub(crate) static STOP_AFTER_HIDE: AtomicBool = AtomicBool::new(false);
pub(crate) static TITLE_UNRESPONSIVE: AtomicBool = AtomicBool::new(false);
pub(crate) const SW_HIDE: i32 = 0;
pub(crate) const SW_SHOWNOACTIVATE: i32 = 4;
pub(crate) struct CrossProcessGate;
impl CrossProcessGate {
    pub(crate) fn acquire() -> Result<Self, IndicatorError> {
        Ok(Self)
    }
}
pub(crate) struct CrossProcessCaptureRequest;
impl CrossProcessCaptureRequest {
    pub(crate) fn begin() -> Result<Self, IndicatorError> {
        Ok(Self)
    }
}
pub(crate) fn cross_process_capture_requested() -> bool {
    false
}
pub(crate) unsafe fn GetClassNameW(hwnd: HWND, output: &mut [u16]) -> i32 {
    let windows = WINDOWS.lock().unwrap();
    let Some(window) = windows.iter().find(|w| w.raw == hwnd.0 as usize) else {
        return 0;
    };
    let encoded: Vec<_> = window.class.encode_utf16().collect();
    output[..encoded.len()].copy_from_slice(&encoded);
    encoded.len() as i32
}
pub(crate) unsafe fn IsWindowVisible(hwnd: HWND) -> BOOL {
    BOOL(
        WINDOWS
            .lock()
            .unwrap()
            .iter()
            .any(|w| w.raw == hwnd.0 as usize && w.visible) as i32,
    )
}
pub(crate) unsafe fn GetWindowThreadProcessId(hwnd: HWND, pid: Option<*mut u32>) -> u32 {
    let windows = WINDOWS.lock().unwrap();
    let Some(window) = windows.iter().find(|w| w.raw == hwnd.0 as usize) else {
        return 0;
    };
    if let Some(pid) = pid {
        unsafe {
            *pid = window.pid;
        }
    }
    123
}
pub(crate) unsafe fn SendMessageTimeoutW(
    hwnd: HWND,
    message: u32,
    capacity: WPARAM,
    buffer: LPARAM,
    flags: u32,
    timeout: u32,
    result: Option<*mut usize>,
) -> LRESULT {
    assert_eq!(message, WM_GETTEXT);
    assert_eq!(flags, SMTO_ABORTIFHUNG | SMTO_BLOCK);
    assert_eq!(timeout, 20);
    if TITLE_UNRESPONSIVE.load(Ordering::Acquire) {
        return LRESULT(0);
    }
    let windows = WINDOWS.lock().unwrap();
    let Some(window) = windows.iter().find(|w| w.raw == hwnd.0 as usize) else {
        return LRESULT(0);
    };
    let output = unsafe { std::slice::from_raw_parts_mut(buffer.0 as *mut u16, capacity.0) };
    let encoded: Vec<_> = window.title.encode_utf16().collect();
    output[..encoded.len()].copy_from_slice(&encoded);
    unsafe {
        *result.unwrap() = encoded.len();
    }
    LRESULT(1)
}
pub(crate) unsafe fn ShowWindow(hwnd: HWND, mode: i32) -> BOOL {
    let mut windows = WINDOWS.lock().unwrap();
    let window = windows
        .iter_mut()
        .find(|w| w.raw == hwnd.0 as usize)
        .unwrap();
    let old = window.visible;
    window.visible = mode != SW_HIDE;
    if mode == SW_HIDE && STOP_AFTER_HIDE.load(Ordering::Acquire) {
        ACTIVE.store(false, Ordering::Release);
    }
    BOOL(old as i32)
}
pub(crate) unsafe fn EnumWindows(
    callback: Option<unsafe extern "system" fn(HWND, LPARAM) -> BOOL>,
    context: LPARAM,
) -> Result<(), ()> {
    let handles: Vec<_> = WINDOWS.lock().unwrap().iter().map(|w| w.raw).collect();
    for raw in handles {
        if !unsafe { callback.unwrap()(HWND(raw as *mut _), context) }.as_bool() {
            return Err(());
        }
    }
    Ok(())
}
fn setup(pid: u32) {
    let _ = crate::register_cursor_renderer_id("owned-test-runtime".into());
    *WINDOWS.lock().unwrap() = vec![Window {
        raw: 55,
        pid,
        class: "Cua.AgentCursorOverlay",
        title: "Cua.AgentCursorOverlay.owned-test-runtime".into(),
        visible: true,
    }];
    ACTIVE.store(true, Ordering::Release);
    STOP_AFTER_HIDE.store(false, Ordering::Release);
    TITLE_UNRESPONSIVE.store(false, Ordering::Release);
}
fn visible(raw: usize) -> bool {
    unsafe { IsWindowVisible(HWND(raw as *mut _)) }.as_bool()
}

#[rstest]
fn owned_cursor_success_restores_on_guard_drop() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    setup(std::process::id());
    let guard = capture_exclusion::begin(&AtomicBool::new(true)).expect("owned overlay exclusion");
    assert!(!visible(55));
    drop(guard);
    assert!(visible(55));
}

#[rstest]
fn foreign_same_class_window_is_not_hidden_and_capture_is_refused() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    setup(std::process::id().wrapping_add(1));
    let result = capture_exclusion::begin(&AtomicBool::new(true));
    assert!(
        visible(55),
        "foreign same-class ordinary root must never be hidden"
    );
    assert!(
        result.is_err(),
        "unknown matching overlay must refuse capture"
    );
}

#[rstest]
fn same_process_unregistered_cursor_title_is_not_authority() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    setup(std::process::id());
    WINDOWS.lock().unwrap()[0].title = "Cua.AgentCursorOverlay.other-consumer".into();
    let result = capture_exclusion::begin(&AtomicBool::new(true));
    assert!(visible(55));
    assert!(result.is_err());
}

#[rstest]
fn restoration_revalidates_registered_title_on_the_same_window() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    setup(std::process::id());
    let guard = capture_exclusion::begin(&AtomicBool::new(true)).unwrap();
    WINDOWS.lock().unwrap()[0].title = "unregistered".into();
    drop(guard);
    assert!(
        !visible(55),
        "restoration requires the still-registered native identity"
    );
}

fn assert_partial_exclusion_restores_owned_cursor(stop_after_hide: bool) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    setup(std::process::id());
    WINDOWS.lock().unwrap().push(Window {
        raw: 66,
        pid: std::process::id().wrapping_add(1),
        class: "DccCuaControlBanner",
        title: "uncooperative-peer".into(),
        visible: true,
    });
    STOP_AFTER_HIDE.store(stop_after_hide, Ordering::Release);
    let registration = Registration::begin(state());
    let presenter = std::thread::spawn(|| {
        while !state().suppressed.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        state().acknowledged.store(1, Ordering::Release);
    });
    let result = capture_exclusion::begin(&ACTIVE);
    presenter.join().unwrap();
    drop(registration);
    assert!(result.is_err());
    assert!(
        visible(55),
        "partial acquisition must restore its already-hidden cursor on Err"
    );
    assert!(visible(66), "a foreign presenter must not be hidden");
}

#[rstest]
fn timeout_restores_owned_cursor_with_acknowledged_local_and_uncooperative_peer() {
    assert_partial_exclusion_restores_owned_cursor(false);
}

#[rstest]
fn presenter_stopping_after_hide_restores_owned_cursor() {
    assert_partial_exclusion_restores_owned_cursor(true);
}

#[rstest]
fn peer_presenter_suppresses_only_its_registered_cursor_and_restores_on_release() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    setup(std::process::id());
    WINDOWS.lock().unwrap().insert(
        0,
        Window {
            raw: 66,
            pid: std::process::id().wrapping_add(1),
            class: "Cua.AgentCursorOverlay",
            title: "Cua.AgentCursorOverlay.other".into(),
            visible: true,
        },
    );
    WINDOWS.lock().unwrap().push(Window {
        raw: 77,
        pid: std::process::id(),
        class: "DccCuaControlBanner",
        title: "local-presenter".into(),
        visible: true,
    });
    let mut cursors = CursorSuppression::default();
    let mut acknowledged = 0;
    state().requested.fetch_add(1, Ordering::AcqRel);
    state().suppressed.store(true, Ordering::Release);
    assert!(hide_and_acknowledge(
        state(),
        HWND(77 as *mut _),
        &[],
        &AtomicBool::new(true),
        &AtomicBool::new(false),
        &mut acknowledged,
        &mut cursors
    ));
    assert!(!visible(55));
    assert!(
        visible(66),
        "peer must never suppress a foreign class-name match"
    );
    state().suppressed.store(false, Ordering::Release);
    assert!(!hide_and_acknowledge(
        state(),
        HWND(77 as *mut _),
        &[],
        &AtomicBool::new(false),
        &AtomicBool::new(false),
        &mut acknowledged,
        &mut cursors
    ));
    assert!(visible(55));
    assert!(visible(66));
}

#[rstest]
fn peer_presenter_drop_restores_cursor_after_stopping_mid_request() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    setup(std::process::id());
    let mut cursors = CursorSuppression::default();
    assert!(hide_cursor_overlays_and_confirm_all_hidden(&mut cursors.0));
    assert!(!visible(55));
    drop(cursors);
    assert!(visible(55));
}

#[rstest]
fn enumeration_error_after_owned_hide_restores_before_returning_error() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    setup(std::process::id());
    WINDOWS.lock().unwrap().push(Window {
        raw: 88,
        pid: std::process::id().wrapping_add(1),
        class: "",
        title: "native-query-failure".into(),
        visible: true,
    });
    let result = capture_exclusion::begin(&ACTIVE);
    assert!(result.is_err());
    assert!(visible(55));
    assert!(visible(88));
}

#[rstest]
fn unresponsive_registered_title_refuses_capture_without_hiding() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    setup(std::process::id());
    TITLE_UNRESPONSIVE.store(true, Ordering::Release);
    let result = capture_exclusion::begin(&ACTIVE);
    assert!(result.is_err());
    assert!(visible(55));
}
