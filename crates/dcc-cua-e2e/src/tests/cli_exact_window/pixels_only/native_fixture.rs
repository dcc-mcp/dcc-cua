//! Disposable custom windows only. No UIA/MSAA provider is implemented.
#[allow(unused_imports)] // Required by the repository's separated-test-module gate.
use rstest::rstest;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::{GetStockObject, WHITE_BRUSH};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Default)]
pub(super) struct Probe {
    pub provider_requests: AtomicUsize,
    pub activation_or_input: AtomicUsize,
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(l as *const CREATESTRUCTW) };
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
    }
    let probe = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Probe;
    if !probe.is_null() {
        let probe = unsafe { &*probe };
        if message == WM_GETOBJECT {
            probe.provider_requests.fetch_add(1, Ordering::SeqCst);
            return 0; // Deliberately no provider, including no DefWindowProc proxy.
        }
        if (message == WM_ACTIVATE && w & 0xffff != WA_INACTIVE as usize)
            || matches!(
                message,
                WM_SETFOCUS
                    | WM_KEYDOWN
                    | WM_SYSKEYDOWN
                    | WM_CHAR
                    | WM_LBUTTONDOWN
                    | WM_RBUTTONDOWN
                    | WM_MBUTTONDOWN
            )
        {
            probe.activation_or_input.fetch_add(1, Ordering::SeqCst);
        }
    }
    if message == WM_APP + 1 {
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                80,
                100,
                280,
                220,
                SWP_NOACTIVATE
                    | if w == 1 {
                        SWP_SHOWWINDOW
                    } else {
                        SWP_HIDEWINDOW
                    },
            );
        }
        return 0;
    }
    if message == WM_CLOSE {
        unsafe { PostQuitMessage(0) };
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, message, w, l) }
}

pub(super) struct Fixture {
    pub target: u64,
    pub decoy: u64,
    pub blocker: u64,
    pub probe: Arc<Probe>,
    worker: Option<JoinHandle<()>>,
}

impl Fixture {
    pub fn new() -> Self {
        let probe = Arc::new(Probe::default());
        let window_probe = Arc::clone(&probe);
        let (tx, rx) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || unsafe {
            let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
            let class = wide("DccCuaPixelsOnlyFixture");
            let instance = GetModuleHandleW(std::ptr::null());
            let definition = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                hbrBackground: GetStockObject(WHITE_BRUSH) as _,
                ..std::mem::zeroed()
            };
            assert_ne!(
                RegisterClassW(&definition),
                0,
                "register disposable fixture"
            );
            let create = |title: &str, x, visible| {
                let title = wide(title);
                let hwnd = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                    class.as_ptr(),
                    title.as_ptr(),
                    WS_POPUP,
                    x,
                    100,
                    280,
                    220,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    Arc::as_ptr(&window_probe).cast(),
                );
                assert!(!hwnd.is_null(), "create custom fixture");
                if visible {
                    assert_ne!(
                        SetWindowPos(
                            hwnd,
                            HWND_TOPMOST,
                            x,
                            100,
                            280,
                            220,
                            SWP_NOACTIVATE | SWP_SHOWWINDOW
                        ),
                        0
                    );
                }
                hwnd
            };
            let target = create("pixels-only custom target", 80, true);
            let decoy = create("pixels-only custom decoy", 420, true);
            let blocker = create("pixels-only capture blocker", 80, false);
            tx.send((
                target as usize as u64,
                decoy as usize as u64,
                blocker as usize as u64,
            ))
            .expect("publish exact fixture identities");
            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            for hwnd in [blocker, decoy, target] {
                DestroyWindow(hwnd);
            }
            UnregisterClassW(class.as_ptr(), instance);
        });
        let (target, decoy, blocker) = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("bounded custom fixture startup");
        Self {
            target,
            decoy,
            blocker,
            probe,
            worker: Some(worker),
        }
    }

    pub fn block_capture(&self, blocked: bool) {
        // Message runs on the disposable fixture's owning UI thread.
        unsafe {
            SendMessageW(
                self.blocker as usize as HWND,
                WM_APP + 1,
                usize::from(blocked),
                0,
            )
        };
    }

    pub fn counters(&self) -> (usize, usize) {
        (
            self.probe.provider_requests.load(Ordering::SeqCst),
            self.probe.activation_or_input.load(Ordering::SeqCst),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe { PostMessageW(self.target as usize as HWND, WM_CLOSE, 0, 0) };
        if let Some(worker) = self.worker.take() {
            worker.join().expect("disposable window thread stopped");
        }
    }
}

pub(super) fn foreground() -> usize {
    unsafe { GetForegroundWindow() as usize }
}

pub(super) fn cursor_position() -> (i32, i32) {
    let mut point = POINT { x: 0, y: 0 };
    assert_ne!(unsafe { GetCursorPos(&mut point) }, 0);
    (point.x, point.y)
}

pub(super) fn visible_windows_for(pid: u32) -> usize {
    unsafe extern "system" fn visit(hwnd: HWND, state: LPARAM) -> i32 {
        let (pid, count) = unsafe { &mut *(state as *mut (u32, usize)) };
        let mut owner = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        if owner == *pid && unsafe { IsWindowVisible(hwnd) } != 0 {
            *count += 1;
        }
        1
    }
    let mut state = (pid, 0);
    assert_ne!(
        unsafe { EnumWindows(Some(visit), &raw mut state as isize) },
        0
    );
    state.1
}
