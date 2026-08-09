use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    COLORREF, ERROR_CLASS_ALREADY_EXISTS, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CombineRgn, CreateEllipticRgn, CreateFontW,
    CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER,
    DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint,
    FW_NORMAL, FW_SEMIBOLD, FillRect, FillRgn, GetMonitorInfoW, HGDIOBJ, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    RGN_DIFF, RGN_ERROR, SelectObject, SetBkMode, SetTextColor, SetWindowRgn, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
    SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DI_NORMAL, DefWindowProcW, DestroyWindow, DispatchMessageW,
    DrawIconEx, GCLP_HICON, GCLP_HICONSM, GetClassLongPtrW, GetClientRect, GetPropW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, HC_ACTION, HHOOK, HICON,
    HTTRANSPARENT, HWND_TOPMOST, ICON_SMALL2, IsIconic, IsWindow, IsWindowVisible, KBDLLHOOKSTRUCT,
    LWA_ALPHA, MA_NOACTIVATE, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, RemovePropW,
    SEND_MESSAGE_TIMEOUT_FLAGS, SMTO_ABORTIFHUNG, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SendMessageTimeoutW, SetLayeredWindowAttributes, SetPropW,
    SetWindowDisplayAffinity, SetWindowPos, SetWindowsHookExW, ShowWindow, TranslateMessage,
    UnhookWindowsHookEx, WDA_EXCLUDEFROMCAPTURE, WH_KEYBOARD_LL, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_GETICON, WM_KEYDOWN, WM_KEYUP, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_PAINT, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WNDCLASSW, WNDPROC, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{HRESULT, PCWSTR, w};

use super::{
    BannerActivity, BannerStatus, BannerTarget, IndicatorError, TARGET_FRAME_ALPHA_MAX,
    TARGET_FRAME_GRADIENT_STEPS, TARGET_FRAME_THICKNESS_DIP, breathing_frame_alpha,
    broadcast_interrupt, interrupt_generation, interrupt_generation_changed,
    target_frame_band_alpha, target_frame_band_insets,
};

const BANNER_CLASS: PCWSTR = w!("DccCuaControlBanner");
const FRAME_CLASS: PCWSTR = w!("DccCuaControlFrame");
const OVERLAY_ACTIVITY_PROP: PCWSTR = w!("DccCuaBannerActivity");
const OVERLAY_ICON_PROP: PCWSTR = w!("DccCuaBannerIcon");
const BANNER_ALPHA: u8 = 248;
const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const SURFACE: COLORREF = rgb(24, 28, 35);
const LINE: COLORREF = rgb(45, 51, 61);
const TEXT: COLORREF = rgb(255, 255, 255);
const MUTED: COLORREF = rgb(184, 193, 199);
const ACCENT: COLORREF = rgb(168, 118, 255);
static ESCAPE_HUB: OnceLock<Result<EscapeHub, String>> = OnceLock::new();
static ESCAPE_DOWN: AtomicBool = AtomicBool::new(false);

const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF((blue as u32) << 16 | (green as u32) << 8 | red as u32)
}

pub(super) fn system_language_tag() -> String {
    let mut locale = [0_u16; 85];
    let length = unsafe { GetUserDefaultLocaleName(&mut locale) };
    if length <= 1 {
        return "en".into();
    }
    String::from_utf16_lossy(&locale[..length as usize - 1])
}

pub(super) struct PlatformBanner {
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    interrupted: Arc<AtomicBool>,
    visible: Arc<AtomicBool>,
    inside_target: Arc<AtomicBool>,
    activity: Arc<AtomicU8>,
    thread: Option<JoinHandle<()>>,
}

impl PlatformBanner {
    pub(super) fn start(target: BannerTarget) -> Result<Self, IndicatorError> {
        let escape_hub = escape_hub()?;
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(false));
        let interrupted = Arc::new(AtomicBool::new(false));
        let visible = Arc::new(AtomicBool::new(false));
        let inside_target = Arc::new(AtomicBool::new(false));
        let activity = Arc::new(AtomicU8::new(BannerActivity::Connecting as u8));
        let runtime = BannerRuntime {
            hub_active: Arc::clone(&escape_hub.active),
            generation: interrupt_generation(),
            activity: Arc::clone(&activity),
        };
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let thread_stop = Arc::clone(&stop);
        let thread_active = Arc::clone(&active);
        let thread_interrupted = Arc::clone(&interrupted);
        let thread_visible = Arc::clone(&visible);
        let thread_inside_target = Arc::clone(&inside_target);
        let thread = thread::Builder::new()
            .name("dcc-cua-control-banner".into())
            .spawn(move || {
                let result = run_banner(
                    target,
                    &thread_stop,
                    &thread_active,
                    &thread_interrupted,
                    &thread_visible,
                    &thread_inside_target,
                    &runtime,
                    &ready_tx,
                );
                if let Err(error) = result {
                    let _ = ready_tx.try_send(Err(error.to_string()));
                }
                thread_active.store(false, Ordering::Release);
                thread_visible.store(false, Ordering::Release);
            })
            .map_err(|error| {
                IndicatorError::Backend(format!("failed to start banner thread: {error}"))
            })?;

        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(Self {
                stop,
                active,
                interrupted,
                visible,
                inside_target,
                activity,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                stop.store(true, Ordering::Release);
                let _ = thread.join();
                Err(IndicatorError::Backend(error))
            }
            Err(error) => {
                stop.store(true, Ordering::Release);
                let _ = thread.join();
                Err(IndicatorError::Backend(format!(
                    "timed out while starting control banner: {error}"
                )))
            }
        }
    }

    pub(super) fn status(&self) -> BannerStatus {
        let activity = BannerActivity::from_code(self.activity.load(Ordering::Acquire));
        BannerStatus {
            backend: "win32",
            visible: self.visible.load(Ordering::Acquire),
            target_frame_visible: self.visible.load(Ordering::Acquire),
            interrupted: self.interrupted(),
            stop_key: "Escape",
            label: String::new(),
            activity,
            activity_label: activity.localized_label(&system_language_tag()).into(),
            placement: if self.inside_target.load(Ordering::Acquire) {
                "target_safe_inset"
            } else {
                "window_edge"
            },
            color: activity.color(),
        }
    }

    pub(super) fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Acquire) || !self.active.load(Ordering::Acquire)
    }

    pub(super) fn set_activity(&self, activity: BannerActivity) {
        self.activity.store(activity as u8, Ordering::Release);
    }
}

impl Drop for PlatformBanner {
    fn drop(&mut self) {
        self.activity
            .store(BannerActivity::Stopping as u8, Ordering::Release);
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct OverlayWindow(HWND);

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        let _ = unsafe { RemovePropW(self.0, OVERLAY_ACTIVITY_PROP) };
        let _ = unsafe { RemovePropW(self.0, OVERLAY_ICON_PROP) };
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

struct RegisteredKeyboardHook(HHOOK);

impl Drop for RegisteredKeyboardHook {
    fn drop(&mut self) {
        let _ = unsafe { UnhookWindowsHookEx(self.0) };
    }
}

struct EscapeHub {
    active: Arc<AtomicBool>,
}

struct BannerRuntime {
    hub_active: Arc<AtomicBool>,
    generation: u64,
    activity: Arc<AtomicU8>,
}

impl EscapeHub {
    fn start() -> Result<Self, String> {
        let active = Arc::new(AtomicBool::new(false));
        let thread_active = Arc::clone(&active);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        thread::Builder::new()
            .name("dcc-cua-escape-hub".into())
            .spawn(move || {
                let result = run_escape_hub(&thread_active, &ready_tx);
                if let Err(error) = result {
                    let _ = ready_tx.try_send(Err(error));
                }
                thread_active.store(false, Ordering::Release);
            })
            .map_err(|error| format!("failed to start Escape hub: {error}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(Self { active }),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(format!("timed out while starting Escape hub: {error}")),
        }
    }
}

fn escape_hub() -> Result<&'static EscapeHub, IndicatorError> {
    match ESCAPE_HUB.get_or_init(EscapeHub::start) {
        Ok(hub) if hub.active.load(Ordering::Acquire) => Ok(hub),
        Ok(_) => Err(IndicatorError::Backend("Escape hub stopped".into())),
        Err(error) => Err(IndicatorError::Backend(error.clone())),
    }
}

fn run_escape_hub(
    active: &AtomicBool,
    ready: &std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_level_keyboard_hook), None, 0) }
        .map_err(|error| format!("install Escape stop hook: {error}"))?;
    let _hook = RegisteredKeyboardHook(hook);
    active.store(true, Ordering::Release);
    ready
        .try_send(Ok(()))
        .map_err(|error| format!("signal Escape hub readiness: {error}"))?;
    let mut message = MSG::default();
    loop {
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        thread::sleep(FRAME_INTERVAL);
    }
}

unsafe extern "system" fn low_level_keyboard_hook(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code == HC_ACTION as i32 {
        let event = wparam.0 as u32;
        let keyboard = unsafe { (lparam.0 as *const KBDLLHOOKSTRUCT).as_ref() };
        if let Some(is_down) =
            keyboard.and_then(|keyboard| escape_key_transition(code, event, keyboard.vkCode))
        {
            if is_down {
                if !ESCAPE_DOWN.swap(true, Ordering::AcqRel) {
                    broadcast_interrupt();
                }
            } else {
                ESCAPE_DOWN.store(false, Ordering::Release);
            }
            // Escape is the operator's stop control while a banner is active;
            // do not also deliver it to the controlled application.
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

pub(super) fn escape_key_transition(code: i32, message: u32, virtual_key: u32) -> Option<bool> {
    if code != HC_ACTION as i32 || virtual_key != VK_ESCAPE.0 as u32 {
        return None;
    }
    match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(true),
        WM_KEYUP | WM_SYSKEYUP => Some(false),
        _ => None,
    }
}

struct ThreadDpiAwareness {
    previous: DPI_AWARENESS_CONTEXT,
}

impl ThreadDpiAwareness {
    fn enter() -> Result<Self, IndicatorError> {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.0.is_null() {
            return Err(IndicatorError::Backend(
                "Windows refused per-monitor-v2 DPI awareness for the indicator thread".into(),
            ));
        }
        Ok(Self { previous })
    }
}

impl Drop for ThreadDpiAwareness {
    fn drop(&mut self) {
        let _ = unsafe { SetThreadDpiAwarenessContext(self.previous) };
    }
}

#[allow(clippy::too_many_arguments)]
fn run_banner(
    target: BannerTarget,
    stop: &AtomicBool,
    active: &AtomicBool,
    interrupted: &AtomicBool,
    visible: &AtomicBool,
    inside_target: &AtomicBool,
    runtime: &BannerRuntime,
    ready: &std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<(), IndicatorError> {
    let _dpi_awareness = ThreadDpiAwareness::enter()?;
    let target_window = HWND(target.window_handle as *mut core::ffi::c_void);
    validate_target(target_window, target.process_id)?;
    let identity = target.identity();
    let overlay = OverlayWindow(create_overlay(
        &identity,
        BannerActivity::Connecting,
        target_icon(target_window),
    )?);
    let frames = (0..TARGET_FRAME_GRADIENT_STEPS)
        .map(|_| create_frame_overlay().map(OverlayWindow))
        .collect::<Result<Vec<_>, _>>()?;
    let target_geometry = read_target_geometry(target_window)?;
    let mut geometry = banner_geometry(target_geometry, read_monitor_geometry(target_window)?);
    let mut frame_geometry = target_frame_geometry(target_geometry);
    position_banner(overlay.0, geometry, true)?;
    for (band, frame) in frames.iter().enumerate() {
        set_overlay_alpha(
            frame.0,
            target_frame_band_alpha(TARGET_FRAME_ALPHA_MAX, band),
        )?;
        position_target_frame(frame.0, frame_geometry, band, true)?;
    }
    inside_target.store(geometry.inside_target, Ordering::Release);
    let mut displayed_activity = BannerActivity::Connecting;
    let pulse_started = Instant::now();
    let mut frame_alpha = TARGET_FRAME_ALPHA_MAX;
    active.store(true, Ordering::Release);
    visible.store(true, Ordering::Release);
    ready
        .try_send(Ok(()))
        .map_err(|error| IndicatorError::Backend(format!("signal banner readiness: {error}")))?;

    let mut message = MSG::default();
    while !stop.load(Ordering::Acquire) {
        if !runtime.hub_active.load(Ordering::Acquire) {
            return Err(IndicatorError::Backend("Escape hub stopped".into()));
        }
        if interrupt_generation_changed(runtime.generation, interrupt_generation()) {
            interrupted.store(true, Ordering::Release);
            stop.store(true, Ordering::Release);
            break;
        }
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let next_activity = BannerActivity::from_code(runtime.activity.load(Ordering::Acquire));
        if next_activity != displayed_activity {
            set_activity_property(overlay.0, next_activity)?;
            let _ = unsafe { InvalidateRect(Some(overlay.0), None, false) };
            displayed_activity = next_activity;
        }
        let next_frame_alpha = breathing_frame_alpha(pulse_started.elapsed());
        if next_frame_alpha != frame_alpha {
            for (band, frame) in frames.iter().enumerate() {
                set_overlay_alpha(frame.0, target_frame_band_alpha(next_frame_alpha, band))?;
            }
            frame_alpha = next_frame_alpha;
        }
        validate_target(target_window, target.process_id)?;
        let target_visible = unsafe {
            IsWindowVisible(target_window).as_bool() && !IsIconic(target_window).as_bool()
        };
        if target_visible {
            let next_target = read_target_geometry(target_window)?;
            let next_geometry = banner_geometry(next_target, read_monitor_geometry(target_window)?);
            if next_geometry != geometry {
                position_banner(
                    overlay.0,
                    next_geometry,
                    next_geometry.width != geometry.width
                        || next_geometry.height != geometry.height,
                )?;
                geometry = next_geometry;
                inside_target.store(geometry.inside_target, Ordering::Release);
            }
            let next_frame_geometry = target_frame_geometry(next_target);
            if next_frame_geometry != frame_geometry {
                for (band, frame) in frames.iter().enumerate() {
                    position_target_frame(
                        frame.0,
                        next_frame_geometry,
                        band,
                        next_frame_geometry.width != frame_geometry.width
                            || next_frame_geometry.height != frame_geometry.height
                            || next_frame_geometry.thickness != frame_geometry.thickness,
                    )?;
                }
                frame_geometry = next_frame_geometry;
            }
            if !visible.swap(true, Ordering::AcqRel) {
                let _ = unsafe { ShowWindow(overlay.0, SW_SHOWNOACTIVATE) };
                for frame in &frames {
                    let _ = unsafe { ShowWindow(frame.0, SW_SHOWNOACTIVATE) };
                }
            }
        } else if visible.swap(false, Ordering::AcqRel) {
            let _ = unsafe { ShowWindow(overlay.0, SW_HIDE) };
            for frame in &frames {
                let _ = unsafe { ShowWindow(frame.0, SW_HIDE) };
            }
        }
        thread::sleep(FRAME_INTERVAL);
    }
    Ok(())
}

fn validate_target(window: HWND, expected_pid: u32) -> Result<(), IndicatorError> {
    if !unsafe { IsWindow(Some(window)).as_bool() } {
        return Err(IndicatorError::InvalidTarget(
            "control target window no longer exists".into(),
        ));
    }
    let mut actual_pid = 0;
    unsafe { GetWindowThreadProcessId(window, Some(&mut actual_pid)) };
    if actual_pid != expected_pid {
        return Err(IndicatorError::InvalidTarget(
            "control target window changed process ownership".into(),
        ));
    }
    Ok(())
}

fn target_icon(window: HWND) -> Option<HICON> {
    for icon_kind in [ICON_SMALL2, 0, 1] {
        let mut value = 0_usize;
        let sent = unsafe {
            SendMessageTimeoutW(
                window,
                WM_GETICON,
                WPARAM(icon_kind as usize),
                LPARAM(0),
                SEND_MESSAGE_TIMEOUT_FLAGS(SMTO_ABORTIFHUNG.0),
                100,
                Some(&mut value),
            )
        };
        if sent.0 != 0 && value != 0 {
            return Some(HICON(value as *mut core::ffi::c_void));
        }
    }
    [GCLP_HICONSM, GCLP_HICON]
        .into_iter()
        .map(|index| unsafe { GetClassLongPtrW(window, index) })
        .find(|value| *value != 0)
        .map(|value| HICON(value as *mut core::ffi::c_void))
}

fn create_overlay(
    identity: &str,
    activity: BannerActivity,
    icon: Option<HICON>,
) -> Result<HWND, IndicatorError> {
    let window = create_window(BANNER_CLASS, Some(window_proc), identity, BANNER_ALPHA)?;
    if let Err(error) = set_activity_property(window, activity) {
        let _ = unsafe { DestroyWindow(window) };
        return Err(error);
    }
    if let Some(icon) = icon
        && let Err(error) = unsafe { SetPropW(window, OVERLAY_ICON_PROP, Some(HANDLE(icon.0))) }
    {
        let _ = unsafe { DestroyWindow(window) };
        return Err(IndicatorError::Backend(format!(
            "set target application icon: {error}"
        )));
    }
    Ok(window)
}

fn create_frame_overlay() -> Result<HWND, IndicatorError> {
    create_window(
        FRAME_CLASS,
        Some(frame_window_proc),
        "",
        TARGET_FRAME_ALPHA_MAX,
    )
}

fn create_window(
    class_name: PCWSTR,
    procedure: WNDPROC,
    title: &str,
    alpha: u8,
) -> Result<HWND, IndicatorError> {
    register_class(class_name, procedure)?;
    let instance = unsafe { GetModuleHandleW(None) }
        .map_err(|error| IndicatorError::Backend(format!("resolve module handle: {error}")))?;
    let title = wide(title);
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(
                WS_EX_TOPMOST.0
                    | WS_EX_TOOLWINDOW.0
                    | WS_EX_NOACTIVATE.0
                    | WS_EX_TRANSPARENT.0
                    | WS_EX_LAYERED.0,
            ),
            class_name,
            PCWSTR(title.as_ptr()),
            WINDOW_STYLE(WS_POPUP.0),
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
    .map_err(|error| IndicatorError::Backend(format!("create indicator window: {error}")))?;
    if let Err(error) = unsafe { SetLayeredWindowAttributes(window, COLORREF(0), alpha, LWA_ALPHA) }
    {
        let _ = unsafe { DestroyWindow(window) };
        return Err(IndicatorError::Backend(format!(
            "set indicator opacity: {error}"
        )));
    }
    if let Err(error) = unsafe { SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE) } {
        let _ = unsafe { DestroyWindow(window) };
        return Err(IndicatorError::Backend(format!(
            "exclude indicator from capture: {error}"
        )));
    }
    Ok(window)
}

fn set_activity_property(window: HWND, activity: BannerActivity) -> Result<(), IndicatorError> {
    unsafe {
        SetPropW(
            window,
            OVERLAY_ACTIVITY_PROP,
            Some(HANDLE(
                (usize::from(activity as u8) + 1) as *mut core::ffi::c_void,
            )),
        )
    }
    .map_err(|error| IndicatorError::Backend(format!("set banner activity: {error}")))
}

fn register_class(class_name: PCWSTR, procedure: WNDPROC) -> Result<(), IndicatorError> {
    let instance = unsafe { GetModuleHandleW(None) }
        .map_err(|error| IndicatorError::Backend(format!("resolve module handle: {error}")))?;
    let class = WNDCLASSW {
        lpfnWndProc: procedure,
        hInstance: instance.into(),
        lpszClassName: class_name,
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        let error = windows::core::Error::from_win32();
        if error.code() != HRESULT::from_win32(ERROR_CLASS_ALREADY_EXISTS.0) {
            return Err(IndicatorError::Backend(format!(
                "register banner window class: {error}"
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct BannerGeometry {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) inside_target: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct TargetGeometry {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) dpi: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct MonitorGeometry {
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) right: i32,
    pub(super) bottom: i32,
    pub(super) work_left: i32,
    pub(super) work_top: i32,
    pub(super) work_right: i32,
    pub(super) work_bottom: i32,
}

fn read_target_geometry(target: HWND) -> Result<TargetGeometry, IndicatorError> {
    let mut target_rect = RECT::default();
    unsafe { GetWindowRect(target, &mut target_rect) }
        .map_err(|error| IndicatorError::Backend(format!("read target bounds: {error}")))?;
    let dpi = unsafe { GetDpiForWindow(target) }.max(96);
    if target_rect.right <= target_rect.left || target_rect.bottom <= target_rect.top {
        return Err(IndicatorError::InvalidTarget(
            "control target window has empty bounds".into(),
        ));
    }
    Ok(TargetGeometry {
        x: target_rect.left,
        y: target_rect.top,
        width: target_rect.right - target_rect.left,
        height: target_rect.bottom - target_rect.top,
        dpi,
    })
}

fn read_monitor_geometry(target: HWND) -> Result<MonitorGeometry, IndicatorError> {
    let monitor = unsafe { MonitorFromWindow(target, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return Err(IndicatorError::Backend(format!(
            "read target monitor: {}",
            windows::core::Error::from_win32()
        )));
    }
    Ok(MonitorGeometry {
        left: info.rcMonitor.left,
        top: info.rcMonitor.top,
        right: info.rcMonitor.right,
        bottom: info.rcMonitor.bottom,
        work_left: info.rcWork.left,
        work_top: info.rcWork.top,
        work_right: info.rcWork.right,
        work_bottom: info.rcWork.bottom,
    })
}

pub(super) fn banner_geometry(target: TargetGeometry, monitor: MonitorGeometry) -> BannerGeometry {
    let height = scale(44, target.dpi);
    let available_width = (monitor.work_right - monitor.work_left - scale(16, target.dpi)).max(1);
    let width = scale(480, target.dpi).min(available_width);
    let gap = scale(8, target.dpi);
    let inset = scale(16, target.dpi);
    let fullscreen = (target.x - monitor.left).abs() <= 2
        && (target.y - monitor.top).abs() <= 2
        && (target.x + target.width - monitor.right).abs() <= 2
        && (target.y + target.height - monitor.bottom).abs() <= 2;
    let inside_target = fullscreen || target.y - height - gap < monitor.work_top;
    let y = if inside_target {
        (target.y + inset).min(monitor.work_bottom - height)
    } else {
        target.y - height - gap
    };
    let x = (target.x + (target.width - width) / 2)
        .clamp(monitor.work_left, monitor.work_right - width);
    BannerGeometry {
        x,
        y,
        width,
        height,
        inside_target,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TargetFrameGeometry {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    thickness: i32,
    corner_radius: i32,
}

fn target_frame_geometry(target: TargetGeometry) -> TargetFrameGeometry {
    TargetFrameGeometry {
        x: target.x,
        y: target.y,
        width: target.width,
        height: target.height,
        thickness: scale(TARGET_FRAME_THICKNESS_DIP, target.dpi),
        corner_radius: scale(10, target.dpi),
    }
}

fn position_banner(
    window: HWND,
    geometry: BannerGeometry,
    update_shape: bool,
) -> Result<(), IndicatorError> {
    if update_shape {
        let radius = scale(12, unsafe { GetDpiForWindow(window) }.max(96));
        let region = unsafe {
            CreateRoundRectRgn(
                0,
                0,
                geometry.width,
                geometry.height,
                radius * 2,
                radius * 2,
            )
        };
        if region.0.is_null() {
            return Err(IndicatorError::Backend(
                "Windows could not create the rounded banner shape".into(),
            ));
        }
        if unsafe { SetWindowRgn(window, Some(region), true) } == 0 {
            let _ = unsafe { DeleteObject(HGDIOBJ(region.0)) };
            return Err(IndicatorError::Backend(
                "Windows rejected the rounded banner shape".into(),
            ));
        }
    }
    unsafe {
        SetWindowPos(
            window,
            Some(HWND_TOPMOST),
            geometry.x,
            geometry.y,
            geometry.width,
            geometry.height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    }
    .map_err(|error| IndicatorError::Backend(format!("position banner: {error}")))?;
    if !unsafe { IsWindowVisible(window).as_bool() } {
        return Err(IndicatorError::Backend(
            "Windows did not make the control banner visible".into(),
        ));
    }
    Ok(())
}

fn position_target_frame(
    window: HWND,
    geometry: TargetFrameGeometry,
    band: usize,
    update_shape: bool,
) -> Result<(), IndicatorError> {
    if update_shape {
        let (outer_inset, inner_inset) = target_frame_band_insets(geometry.thickness, band)
            .ok_or_else(|| IndicatorError::Backend("invalid target frame band".into()))?;
        let outer = unsafe {
            CreateRoundRectRgn(
                outer_inset,
                outer_inset,
                geometry.width - outer_inset,
                geometry.height - outer_inset,
                (geometry.corner_radius - outer_inset).max(1) * 2,
                (geometry.corner_radius - outer_inset).max(1) * 2,
            )
        };
        let inner = unsafe {
            CreateRoundRectRgn(
                inner_inset,
                inner_inset,
                geometry.width - inner_inset,
                geometry.height - inner_inset,
                (geometry.corner_radius - inner_inset).max(1) * 2,
                (geometry.corner_radius - inner_inset).max(1) * 2,
            )
        };
        if outer.0.is_null() || inner.0.is_null() {
            let _ = unsafe { DeleteObject(HGDIOBJ(outer.0)) };
            let _ = unsafe { DeleteObject(HGDIOBJ(inner.0)) };
            return Err(IndicatorError::Backend(
                "Windows could not create the target frame shape".into(),
            ));
        }
        let combined = unsafe { CombineRgn(Some(outer), Some(outer), Some(inner), RGN_DIFF) };
        let _ = unsafe { DeleteObject(HGDIOBJ(inner.0)) };
        if combined == RGN_ERROR || unsafe { SetWindowRgn(window, Some(outer), true) } == 0 {
            let _ = unsafe { DeleteObject(HGDIOBJ(outer.0)) };
            return Err(IndicatorError::Backend(
                "Windows rejected the target frame shape".into(),
            ));
        }
    }
    unsafe {
        SetWindowPos(
            window,
            Some(HWND_TOPMOST),
            geometry.x,
            geometry.y,
            geometry.width,
            geometry.height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    }
    .map_err(|error| IndicatorError::Backend(format!("position target frame: {error}")))?;
    Ok(())
}

fn set_overlay_alpha(window: HWND, alpha: u8) -> Result<(), IndicatorError> {
    unsafe { SetLayeredWindowAttributes(window, COLORREF(0), alpha, LWA_ALPHA) }
        .map_err(|error| IndicatorError::Backend(format!("set indicator opacity: {error}")))
}

unsafe extern "system" fn frame_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(result) = overlay_input_result(message) {
        return result;
    }
    if message != WM_PAINT {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    let mut paint = PAINTSTRUCT::default();
    let device = unsafe { BeginPaint(window, &raw mut paint) };
    if !device.0.is_null() {
        let mut bounds = RECT::default();
        let _ = unsafe { GetClientRect(window, &raw mut bounds) };
        let brush = unsafe { CreateSolidBrush(ACCENT) };
        let _ = unsafe { FillRect(device, &bounds, brush) };
        let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
    }
    let _ = unsafe { EndPaint(window, &paint) };
    LRESULT(0)
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(result) = overlay_input_result(message) {
        return result;
    }
    if message != WM_PAINT {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    let mut paint = PAINTSTRUCT::default();
    let device = unsafe { BeginPaint(window, &raw mut paint) };
    if !device.0.is_null() {
        let mut bounds = RECT::default();
        let _ = unsafe { GetClientRect(window, &raw mut bounds) };
        let dpi = unsafe { GetDpiForWindow(window) }.max(96);
        paint_surface(device, bounds, dpi);
        let icon_value = unsafe { GetPropW(window, OVERLAY_ICON_PROP) };
        if icon_value.0.is_null() {
            paint_fallback_icon(device, dpi);
        } else {
            let icon_size = scale(24, dpi);
            let _ = unsafe {
                DrawIconEx(
                    device,
                    scale(10, dpi),
                    scale(10, dpi),
                    HICON(icon_value.0),
                    icon_size,
                    icon_size,
                    0,
                    None,
                    DI_NORMAL,
                )
            };
        }
        let activity_value = unsafe { GetPropW(window, OVERLAY_ACTIVITY_PROP) };
        let activity = if activity_value.0.is_null() {
            BannerActivity::Ready
        } else {
            BannerActivity::from_code((activity_value.0 as usize - 1) as u8)
        };
        paint_activity(device, activity, dpi);
        let identity = window_text(window);
        paint_copy(device, bounds, dpi, &identity, activity);
    }
    let _ = unsafe { EndPaint(window, &paint) };
    LRESULT(0)
}

pub(super) fn overlay_input_result(message: u32) -> Option<LRESULT> {
    match message {
        WM_NCHITTEST => Some(LRESULT(HTTRANSPARENT as isize)),
        WM_MOUSEACTIVATE => Some(LRESULT(MA_NOACTIVATE as isize)),
        _ => None,
    }
}

fn paint_surface(device: windows::Win32::Graphics::Gdi::HDC, bounds: RECT, dpi: u32) {
    let radius = scale(12, dpi);
    let outer =
        unsafe { CreateRoundRectRgn(0, 0, bounds.right, bounds.bottom, radius * 2, radius * 2) };
    let inner = unsafe {
        CreateRoundRectRgn(
            1,
            1,
            bounds.right - 1,
            bounds.bottom - 1,
            radius * 2,
            radius * 2,
        )
    };
    if !outer.0.is_null() && !inner.0.is_null() {
        let border = unsafe { CreateSolidBrush(LINE) };
        let surface = unsafe { CreateSolidBrush(SURFACE) };
        let _ = unsafe { FillRgn(device, outer, border) };
        let _ = unsafe { FillRgn(device, inner, surface) };
        let _ = unsafe { DeleteObject(HGDIOBJ(border.0)) };
        let _ = unsafe { DeleteObject(HGDIOBJ(surface.0)) };
    } else {
        let surface = unsafe { CreateSolidBrush(SURFACE) };
        let _ = unsafe { FillRect(device, &bounds, surface) };
        let _ = unsafe { DeleteObject(HGDIOBJ(surface.0)) };
    }
    let _ = unsafe { DeleteObject(HGDIOBJ(outer.0)) };
    let _ = unsafe { DeleteObject(HGDIOBJ(inner.0)) };
}

fn paint_fallback_icon(device: windows::Win32::Graphics::Gdi::HDC, dpi: u32) {
    let left = scale(10, dpi);
    let top = scale(10, dpi);
    let size = scale(24, dpi);
    let region = unsafe {
        CreateRoundRectRgn(
            left,
            top,
            left + size,
            top + size,
            scale(7, dpi),
            scale(7, dpi),
        )
    };
    if !region.0.is_null() {
        let brush = unsafe { CreateSolidBrush(ACCENT) };
        let _ = unsafe { FillRgn(device, region, brush) };
        let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
        let _ = unsafe { DeleteObject(HGDIOBJ(region.0)) };
    }
    draw_text(
        device,
        "D",
        RECT {
            left,
            top,
            right: left + size,
            bottom: top + size,
        },
        scale(12, dpi),
        FW_SEMIBOLD.0 as i32,
        TEXT,
        true,
    );
}

fn paint_activity(device: windows::Win32::Graphics::Gdi::HDC, activity: BannerActivity, dpi: u32) {
    let left = scale(43, dpi);
    let top = scale(18, dpi);
    let size = scale(8, dpi);
    let region = unsafe { CreateEllipticRgn(left, top, left + size, top + size) };
    if !region.0.is_null() {
        let color = activity.color();
        let brush = unsafe { CreateSolidBrush(rgb(color.red, color.green, color.blue)) };
        let _ = unsafe { FillRgn(device, region, brush) };
        let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
        let _ = unsafe { DeleteObject(HGDIOBJ(region.0)) };
    }
}

fn paint_copy(
    device: windows::Win32::Graphics::Gdi::HDC,
    bounds: RECT,
    dpi: u32,
    identity: &str,
    activity: BannerActivity,
) {
    let stop_width = scale(78, dpi);
    let divider_x = bounds.right - stop_width - scale(9, dpi);
    let divider = RECT {
        left: divider_x,
        top: scale(10, dpi),
        right: divider_x + 1,
        bottom: bounds.bottom - scale(10, dpi),
    };
    let brush = unsafe { CreateSolidBrush(LINE) };
    let _ = unsafe { FillRect(device, &divider, brush) };
    let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
    let text_left = scale(58, dpi);
    let text_right = divider_x - scale(10, dpi);
    draw_text(
        device,
        identity,
        RECT {
            left: text_left,
            top: scale(3, dpi),
            right: text_right,
            bottom: scale(23, dpi),
        },
        scale(12, dpi),
        FW_SEMIBOLD.0 as i32,
        TEXT,
        false,
    );
    draw_text(
        device,
        activity.localized_label(&system_language_tag()),
        RECT {
            left: text_left,
            top: scale(21, dpi),
            right: text_right,
            bottom: scale(41, dpi),
        },
        scale(11, dpi),
        FW_NORMAL.0 as i32,
        MUTED,
        false,
    );
    let stop_label = if system_language_tag().starts_with("zh") {
        "Esc 停止"
    } else {
        "Esc Stop"
    };
    draw_text(
        device,
        stop_label,
        RECT {
            left: divider_x + scale(6, dpi),
            top: 0,
            right: bounds.right - scale(4, dpi),
            bottom: bounds.bottom,
        },
        scale(11, dpi),
        FW_SEMIBOLD.0 as i32,
        MUTED,
        true,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    device: windows::Win32::Graphics::Gdi::HDC,
    text: &str,
    mut bounds: RECT,
    height: i32,
    weight: i32,
    color: COLORREF,
    centered: bool,
) {
    let font = unsafe {
        CreateFontW(
            -height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            u32::from(DEFAULT_PITCH.0),
            w!("Segoe UI"),
        )
    };
    if font.0.is_null() {
        return;
    }
    let previous = unsafe { SelectObject(device, HGDIOBJ(font.0)) };
    let _ = unsafe { SetBkMode(device, TRANSPARENT) };
    let _ = unsafe { SetTextColor(device, color) };
    let mut text = text.encode_utf16().collect::<Vec<_>>();
    let horizontal = if centered { DT_CENTER.0 } else { DT_LEFT.0 };
    let format = windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(
        horizontal | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0,
    );
    let _ = unsafe { DrawTextW(device, &mut text, &raw mut bounds, format) };
    let _ = unsafe { SelectObject(device, previous) };
    let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
}

fn window_text(window: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(window) }.max(0) as usize;
    let mut text = vec![0_u16; length + 1];
    let copied = unsafe { GetWindowTextW(window, &mut text) }.max(0) as usize;
    String::from_utf16_lossy(&text[..copied])
}

fn scale(value: i32, dpi: u32) -> i32 {
    ((i64::from(value) * i64::from(dpi) + 48) / 96) as i32
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
