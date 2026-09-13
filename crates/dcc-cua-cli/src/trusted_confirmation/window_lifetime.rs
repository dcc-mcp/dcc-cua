//! Owns only the confirmation window's lifetime; never approves an action.

use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use dcc_cua_host::TrustedActionConfirmationRequest;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, HWND, WAIT_TIMEOUT},
    System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GW_ENABLEDPOPUP, GW_OWNER, GetWindow,
        GetWindowThreadProcessId, KillTimer, PostMessageW, SetTimer, WM_CLOSE,
    },
};

enum Target {
    Desktop,
    Window {
        process: HANDLE,
        pid: u32,
        window: HWND,
    },
}

impl Target {
    fn new(pid: Option<u32>, window: Option<u64>) -> Option<Self> {
        match (pid, window) {
            (None, None) => Some(Self::Desktop),
            (Some(pid), Some(window)) if pid != 0 && window != 0 => {
                let window = usize::try_from(window).ok()? as HWND;
                let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
                if process.is_null() {
                    return None;
                }
                let target = Self::Window {
                    process,
                    pid,
                    window,
                };
                target.alive().then_some(target)
            }
            _ => None,
        }
    }

    fn alive(&self) -> bool {
        match self {
            Self::Desktop => true,
            Self::Window {
                process,
                pid,
                window,
            } => {
                let mut owner = 0;
                unsafe {
                    // Retain the original process handle so PID reuse cannot revive it.
                    WaitForSingleObject(*process, 0) == WAIT_TIMEOUT
                        && GetWindowThreadProcessId(*window, &mut owner) != 0
                        && owner == *pid
                }
            }
        }
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        if let Self::Window { process, .. } = self {
            unsafe {
                CloseHandle(*process);
            }
        }
    }
}

struct Watch {
    target: Target,
    cancelled: Arc<AtomicBool>,
}

impl Watch {
    fn cancelled(&self) -> bool {
        // Latch invalidation: a disappeared target must never become valid again.
        if !self.target.alive() {
            self.cancelled.store(true, Ordering::Release);
        }
        self.cancelled.load(Ordering::Acquire)
    }
}

thread_local! {
    static WATCH: RefCell<Option<Watch>> = const { RefCell::new(None) };
}

pub(super) struct PromptLifetime {
    owner: HWND,
}

impl PromptLifetime {
    pub(super) fn new(
        request: &TrustedActionConfirmationRequest,
        cancelled: Arc<AtomicBool>,
    ) -> Option<Self> {
        let target = Target::new(request.target_process_id, request.target_window_handle)?;
        let watch = Watch { target, cancelled };
        if watch.cancelled() || WATCH.with(|cell| cell.borrow().is_some()) {
            return None;
        }
        // A private invisible owner avoids title searches and unrelated dialogs.
        let owner = unsafe {
            CreateWindowExW(
                0,
                windows_sys::w!("STATIC"),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if owner.is_null() {
            return None;
        }
        WATCH.with(|cell| *cell.borrow_mut() = Some(watch));
        let lifetime = Self { owner };
        if unsafe { SetTimer(owner, 1, 200, Some(check_target)) } == 0 {
            return None;
        }
        Some(lifetime)
    }

    pub(super) fn owner(&self) -> HWND {
        self.owner
    }

    pub(super) fn cancelled(&self) -> bool {
        WATCH.with(|cell| cell.borrow().as_ref().is_none_or(Watch::cancelled))
    }
}

impl Drop for PromptLifetime {
    fn drop(&mut self) {
        unsafe {
            KillTimer(self.owner, 1);
            DestroyWindow(self.owner);
        }
        WATCH.with(|cell| *cell.borrow_mut() = None);
    }
}

unsafe extern "system" fn check_target(owner: HWND, _: u32, _: usize, _: u32) {
    let cancelled = WATCH.with(|cell| cell.borrow().as_ref().is_none_or(Watch::cancelled));
    if cancelled {
        let popup = unsafe { GetWindow(owner, GW_ENABLEDPOPUP) };
        close_owned_popup(owner, popup);
    }
}

fn close_owned_popup(owner: HWND, popup: HWND) {
    if !popup.is_null() && popup != owner && unsafe { GetWindow(popup, GW_OWNER) } == owner {
        // MessageBoxW has a Cancel button, so WM_CLOSE dismisses as cancellation.
        unsafe {
            PostMessageW(popup, WM_CLOSE, 0, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn incomplete_or_dead_target_cannot_open_a_prompt() {
        assert!(Target::new(None, Some(1)).is_none());
        assert!(Target::new(Some(0), None).is_none());
        assert!(Target::new(Some(unsafe { GetCurrentProcessId() }), Some(1)).is_none());
        assert!(Target::new(None, None).unwrap().alive());
    }

    #[test]
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

    #[test]
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
}
