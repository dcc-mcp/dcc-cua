//! A real hidden tray-style owner and OS-created #32768 popup, CI only.
#[allow(unused_imports)]
use rstest::rstest;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::WindowsAndMessaging::*,
};

#[derive(Default)]
pub(super) struct Journal {
    pub cancel: AtomicUsize,
    pub commands: AtomicUsize,
    pub input_or_close: AtomicUsize,
    pub returned: AtomicUsize,
    pub selected: AtomicUsize,
}

unsafe extern "system" fn procedure(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(l as *const CREATESTRUCTW) };
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize) };
    }
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const Journal;
    if !pointer.is_null() {
        let journal = unsafe { &*pointer };
        if message == WM_CANCELMODE {
            journal.cancel.fetch_add(1, Ordering::SeqCst);
        }
        if message == WM_COMMAND {
            journal.commands.fetch_add(1, Ordering::SeqCst);
        }
        if matches!(
            message,
            WM_KEYDOWN
                | WM_KEYUP
                | WM_SYSKEYDOWN
                | WM_SYSKEYUP
                | WM_CHAR
                | WM_LBUTTONDOWN
                | WM_RBUTTONDOWN
                | WM_MBUTTONDOWN
                | WM_CLOSE
        ) {
            journal.input_or_close.fetch_add(1, Ordering::SeqCst);
        }
        if message == WM_APP + 40 {
            unsafe {
                let menu = CreatePopupMenu();
                assert!(!menu.is_null());
                let label: Vec<u16> = "No action may be invoked"
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                assert_ne!(AppendMenuW(menu, MF_STRING, 101, label.as_ptr()), 0);
                // Safety cleanup for a failed CI assertion; never a production
                // retry. The successful path finishes long before this timer.
                SetTimer(window, 1, 60_000, None);
                let selected = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_NONOTIFY,
                    180,
                    180,
                    0,
                    window,
                    std::ptr::null(),
                );
                journal.selected.store(selected as usize, Ordering::SeqCst);
                journal.returned.fetch_add(1, Ordering::SeqCst);
                KillTimer(window, 1);
                DestroyMenu(menu);
            }
            return 0;
        }
        if message == WM_TIMER {
            unsafe { SendMessageW(window, WM_CANCELMODE, 0, 0) };
            return 0;
        }
    }
    unsafe { DefWindowProcW(window, message, w, l) }
}

pub(super) struct MenuFixture {
    pub owner: u64,
    pub thread: u32,
    pub journal: Arc<Journal>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl MenuFixture {
    pub fn new() -> Self {
        let journal = Arc::new(Journal::default());
        let worker_journal = Arc::clone(&journal);
        let (tx, rx) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || unsafe {
            let class: Vec<u16> = "DccCuaNativeMenuDismissFixture"
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let instance = GetModuleHandleW(std::ptr::null());
            let definition = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            };
            assert_ne!(RegisterClassW(&definition), 0);
            let owner = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                Arc::as_ptr(&worker_journal).cast(),
            );
            assert!(!owner.is_null());
            tx.send((owner as usize as u64, GetCurrentThreadId()))
                .unwrap();
            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            DestroyWindow(owner);
            UnregisterClassW(class.as_ptr(), instance);
        });
        let (owner, thread) = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        Self {
            owner,
            thread,
            journal,
            worker: Some(worker),
        }
    }

    pub fn open(&self) {
        assert_ne!(
            unsafe { PostMessageW(self.owner as usize as HWND, WM_APP + 40, 0, 0) },
            0
        );
    }

    pub fn popup(&self) -> Option<u64> {
        unsafe extern "system" fn visit(window: HWND, parameter: LPARAM) -> i32 {
            let (wanted_thread, roots) = unsafe { &mut *(parameter as *mut (u32, Vec<u64>)) };
            let mut pid = 0;
            let thread = unsafe { GetWindowThreadProcessId(window, &mut pid) };
            let mut class = [0_u16; 32];
            let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), 32) };
            if pid == std::process::id()
                && thread == *wanted_thread
                && unsafe { IsWindowVisible(window) } != 0
                && length == 6
                && class[..6] == [35, 51, 50, 55, 54, 56]
            {
                roots.push(window as usize as u64);
            }
            1
        }
        let mut state = (self.thread, Vec::new());
        assert_ne!(
            unsafe { EnumWindows(Some(visit), &raw mut state as isize) },
            0
        );
        match state.1.as_slice() {
            [only] => Some(*only),
            [] => None,
            _ => panic!("fixture unexpectedly opened ambiguous native menus"),
        }
    }

    pub fn counts(&self) -> (usize, usize, usize, usize, usize) {
        let j = &self.journal;
        (
            j.cancel.load(Ordering::SeqCst),
            j.commands.load(Ordering::SeqCst),
            j.input_or_close.load(Ordering::SeqCst),
            j.returned.load(Ordering::SeqCst),
            j.selected.load(Ordering::SeqCst),
        )
    }
}

impl Drop for MenuFixture {
    fn drop(&mut self) {
        // Only fixture-owned teardown, after all route assertions. It cannot
        // count as successful production dismissal.
        if self.journal.returned.load(Ordering::SeqCst) == 0 {
            unsafe {
                SendMessageTimeoutW(
                    self.owner as usize as HWND,
                    WM_CANCELMODE,
                    0,
                    0,
                    SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT,
                    250,
                    std::ptr::null_mut(),
                )
            };
        }
        unsafe { PostThreadMessageW(self.thread, WM_QUIT, 0, 0) };
        if let Some(worker) = self.worker.take() {
            worker.join().expect("fixture UI thread exited");
        }
    }
}
