//! Test-only Win32 read/mutation boundary. No desktop APIs are linked or run.
#[allow(unused_imports)]
use rstest::rstest;
use std::sync::Mutex;

#[derive(Clone, Copy)]
struct HWND(*mut core::ffi::c_void);
struct HANDLE(*mut core::ffi::c_void);
struct BOOL(bool);
impl BOOL { fn as_bool(self) -> bool { self.0 } }
#[derive(Debug)]
enum IndicatorError { Backend(String) }
const OVERLAY_INSTANCE_PROP: u32 = 1;
const OVERLAY_ACTIVITY_PROP: u32 = 2;
const OVERLAY_RECORDING_PROP: u32 = 3;
const OVERLAY_LIVE_PROP: u32 = 4;
const OVERLAY_ICON_PROP: u32 = 5;
const SW_HIDE: u32 = 0;

struct Native {
    pid: u32, thread: u32, cookie: usize, alive: bool, visible: bool,
    fail_set: bool, replace_during_set: bool,
    sets: usize, removes: usize, destroys: usize, hides: usize,
}
static NATIVE: Mutex<Option<Native>> = Mutex::new(None);
fn reset() {
    *NATIVE.lock().unwrap() = Some(Native { pid: std::process::id(), thread: 7,
        cookie: 0, alive: true, visible: true, fail_set: false,
        replace_during_set: false, sets: 0, removes: 0, destroys: 0, hides: 0 });
}
fn native<R>(read: impl FnOnce(&mut Native) -> R) -> R {
    read(NATIVE.lock().unwrap().as_mut().unwrap())
}
unsafe fn GetWindowThreadProcessId(_: HWND, pid: Option<&mut u32>) -> u32 {
    native(|state| { *pid.unwrap() = if state.alive { state.pid } else { 0 };
        if state.alive { state.thread } else { 0 } })
}
unsafe fn GetCurrentThreadId() -> u32 { 7 }
unsafe fn GetPropW(_: HWND, _: u32) -> HANDLE { HANDLE(native(|state| state.cookie) as *mut _) }
unsafe fn SetPropW(_: HWND, _: u32, handle: Option<HANDLE>) -> Result<(), &'static str> {
    native(|state| {
        state.sets += 1;
        if state.replace_during_set { state.thread = 8; state.pid += 1; state.cookie = 99; return Err("replaced"); }
        if state.fail_set { return Err("failed"); }
        state.cookie = handle.unwrap().0 as usize;
        Ok(())
    })
}
unsafe fn RemovePropW(_: HWND, property: u32) -> Result<(), ()> {
    native(|state| { state.removes += 1; if property == OVERLAY_INSTANCE_PROP { state.cookie = 0; } });
    Ok(())
}
unsafe fn DestroyWindow(_: HWND) -> Result<(), ()> {
    native(|state| { state.destroys += 1; state.alive = false; }); Ok(())
}
unsafe fn ShowWindow(_: HWND, mode: u32) -> BOOL {
    assert_eq!(mode, SW_HIDE);
    native(|state| { state.hides += 1; state.visible = false; }); BOOL(true)
}
unsafe fn IsWindowVisible(_: HWND) -> BOOL { BOOL(native(|state| state.visible)) }
fn raw() -> HWND { HWND(1 as *mut _) }

#[rstest]
fn owned_cookie_allows_exact_hide_and_cleanup() {
    reset();
    let surface = OverlayWindow::new(raw()).unwrap();
    assert!(surface.still_owned());
    assert!(hide_surfaces(surface, vec![]));
    native(|state| { assert_eq!(state.hides, 1); assert_eq!(state.destroys, 1); assert_eq!(state.removes, 5); });
}

#[rstest]
fn a_foreign_new_handle_is_never_bound_or_destroyed() {
    for changed in ["pid", "thread", "existing_cookie", "missing"] {
        reset();
        native(|state| match changed { "pid" => state.pid += 1, "thread" => state.thread += 1,
            "existing_cookie" => state.cookie = 99, _ => state.alive = false });
        assert!(OverlayWindow::new(raw()).is_err());
        native(|state| { assert_eq!(state.sets, 0); assert_eq!(state.destroys, 0); assert_eq!(state.removes, 0); });
    }
}

#[rstest]
fn set_property_failure_cleans_only_the_still_owned_new_window() {
    reset(); native(|state| state.fail_set = true);
    assert!(OverlayWindow::new(raw()).is_err());
    native(|state| assert_eq!(state.destroys, 1));
    reset(); native(|state| state.replace_during_set = true);
    assert!(OverlayWindow::new(raw()).is_err());
    native(|state| { assert_eq!(state.destroys, 0); assert_eq!(state.removes, 0); assert_eq!(state.cookie, 99); });
}

#[rstest]
fn stale_or_reused_instances_never_hide_or_clean_a_replacement() {
    for changed in ["pid", "thread", "cookie", "missing"] {
        reset();
        let surface = OverlayWindow::new(raw()).unwrap();
        native(|state| match changed { "pid" => state.pid += 1, "thread" => state.thread += 1,
            "cookie" => state.cookie += 1, _ => state.alive = false });
        assert!(!surface.still_owned());
        assert!(!hide_surfaces(surface, vec![]));
        native(|state| { assert_eq!(state.hides, 0); assert_eq!(state.destroys, 0); assert_eq!(state.removes, 0); });
    }
}

#[rstest]
fn exhausted_private_tokens_fail_closed_and_clean_the_new_owned_window() {
    reset();
    let previous = NEXT_OVERLAY_INSTANCE.swap(usize::MAX, Ordering::AcqRel);
    let result = OverlayWindow::new(raw());
    NEXT_OVERLAY_INSTANCE.store(previous, Ordering::Release);
    assert!(result.is_err());
    native(|state| {
        assert_eq!(state.sets, 0);
        assert_eq!(state.destroys, 1);
    });
}
