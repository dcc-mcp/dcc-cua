use super::*;
use rstest::rstest;
use windows_sys::Win32::{
    System::Threading::GetCurrentProcessId,
    UI::WindowsAndMessaging::{MSG, PM_REMOVE, PeekMessageW, WS_POPUP},
};

struct Window(HWND);
impl Window {
    fn new(owner: HWND) -> Self {
        let window = unsafe {
            CreateWindowExW(
                0,
                windows_sys::w!("STATIC"),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!window.is_null());
        Self(window)
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

#[rstest]
fn incomplete_or_dead_target_cannot_open_a_prompt() {
    assert!(Target::new(None, Some(1)).is_none());
    assert!(Target::new(Some(0), None).is_none());
    assert!(Target::new(Some(unsafe { GetCurrentProcessId() }), Some(1)).is_none());
    assert!(Target::new(None, None).unwrap().alive());
}

#[rstest]
fn destroying_the_target_latches_cancellation() {
    let window = Window::new(std::ptr::null_mut());
    let target = Target::new(
        Some(unsafe { GetCurrentProcessId() }),
        Some(window.0 as u64),
    )
    .unwrap();
    let watch = Watch {
        target,
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    assert!(!watch.cancelled());
    drop(window);
    assert!(watch.cancelled());
    assert!(watch.cancelled.load(Ordering::Acquire));
}

#[rstest]
fn cancellation_posts_close_only_to_the_private_owned_popup() {
    let owner = Window::new(std::ptr::null_mut());
    let popup = Window::new(owner.0);
    let unrelated = Window::new(std::ptr::null_mut());
    // Hidden fixtures test ownership without opening an authorization dialog.
    close_owned_popup(owner.0, unrelated.0);
    close_owned_popup(owner.0, popup.0);
    let mut message: MSG = unsafe { std::mem::zeroed() };
    assert_ne!(
        unsafe { PeekMessageW(&mut message, popup.0, WM_CLOSE, WM_CLOSE, PM_REMOVE) },
        0
    );
    assert_eq!(message.message, WM_CLOSE);
    assert_eq!(
        unsafe { PeekMessageW(&mut message, unrelated.0, WM_CLOSE, WM_CLOSE, PM_REMOVE) },
        0
    );
}
