use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CombineRgn, CreateFontW, CreateRectRgn,
    CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER,
    DT_END_ELLIPSIS, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FW_SEMIBOLD,
    FillRect, HGDIOBJ, OUT_DEFAULT_PRECIS, PAINTSTRUCT, RGN_DIFF, RGN_ERROR, SelectObject,
    SetBkMode, SetTextColor, SetWindowRgn, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
    SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, HWND_TOPMOST, IsIconic,
    IsWindow, IsWindowVisible, LWA_ALPHA, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetLayeredWindowAttributes,
    SetWindowDisplayAffinity, SetWindowPos, ShowWindow, TranslateMessage, WDA_EXCLUDEFROMCAPTURE,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_HOTKEY, WM_PAINT, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::{BannerStatus, BannerTarget, IndicatorError};

const BANNER_CLASS: PCWSTR = w!("DccMcpCuaControlBanner");
const FRAME_CLASS: PCWSTR = w!("DccMcpCuaControlFrame");
const BANNER_COLOR: COLORREF = COLORREF(0x00FF_840A);
const FRAME_COLOR: COLORREF = COLORREF(0x00FA_A560);
const BANNER_ALPHA: u8 = 244;
const FRAME_ALPHA_MIN: u8 = 132;
const FRAME_ALPHA_MAX: u8 = 244;
const FRAME_PULSE_PERIOD: Duration = Duration::from_millis(1_800);
const HOTKEY_ID: i32 = 0x4443;
const FRAME_INTERVAL: Duration = Duration::from_millis(16);
static ACTIVE_BANNER: AtomicBool = AtomicBool::new(false);

pub(super) struct PlatformBanner {
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    interrupted: Arc<AtomicBool>,
    visible: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PlatformBanner {
    pub(super) fn start(target: BannerTarget) -> Result<Self, IndicatorError> {
        if ACTIVE_BANNER
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(IndicatorError::Backend(
                "another control banner is already active".into(),
            ));
        }

        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(false));
        let interrupted = Arc::new(AtomicBool::new(false));
        let visible = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let thread_stop = Arc::clone(&stop);
        let thread_active = Arc::clone(&active);
        let thread_interrupted = Arc::clone(&interrupted);
        let thread_visible = Arc::clone(&visible);
        let thread = thread::Builder::new()
            .name("dcc-mcp-cua-control-banner".into())
            .spawn(move || {
                let result = run_banner(
                    target,
                    &thread_stop,
                    &thread_active,
                    &thread_interrupted,
                    &thread_visible,
                    &ready_tx,
                );
                if let Err(error) = result {
                    let _ = ready_tx.try_send(Err(error.to_string()));
                }
                thread_active.store(false, Ordering::Release);
                thread_visible.store(false, Ordering::Release);
                ACTIVE_BANNER.store(false, Ordering::Release);
            })
            .map_err(|error| {
                ACTIVE_BANNER.store(false, Ordering::Release);
                IndicatorError::Backend(format!("failed to start banner thread: {error}"))
            })?;

        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(Self {
                stop,
                active,
                interrupted,
                visible,
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
        BannerStatus {
            backend: "win32",
            visible: self.visible.load(Ordering::Acquire),
            target_frame_visible: self.visible.load(Ordering::Acquire),
            interrupted: self.interrupted(),
            stop_key: "Escape",
        }
    }

    pub(super) fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Acquire) || !self.active.load(Ordering::Acquire)
    }
}

impl Drop for PlatformBanner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct OverlayWindow(HWND);

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

struct RegisteredHotKey(HWND);

impl Drop for RegisteredHotKey {
    fn drop(&mut self) {
        let _ = unsafe { UnregisterHotKey(Some(self.0), HOTKEY_ID) };
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

fn run_banner(
    target: BannerTarget,
    stop: &AtomicBool,
    active: &AtomicBool,
    interrupted: &AtomicBool,
    visible: &AtomicBool,
    ready: &std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<(), IndicatorError> {
    let _dpi_awareness = ThreadDpiAwareness::enter()?;
    let target_window = HWND(target.window_handle as *mut core::ffi::c_void);
    validate_target(target_window, target.process_id)?;
    let overlay = OverlayWindow(create_overlay(BANNER_CLASS, &target.label, BANNER_ALPHA)?);
    let frame = OverlayWindow(create_overlay(FRAME_CLASS, "", FRAME_ALPHA_MAX)?);
    unsafe { RegisterHotKey(Some(overlay.0), HOTKEY_ID, MOD_NOREPEAT, VK_ESCAPE.0 as u32) }
        .map_err(|error| IndicatorError::Backend(format!("reserve Escape stop key: {error}")))?;
    let _hotkey = RegisteredHotKey(overlay.0);
    let target_geometry = read_target_geometry(target_window)?;
    let mut geometry = banner_geometry(target_geometry);
    let mut frame_geometry = target_frame_geometry(target_geometry);
    let pulse_started = Instant::now();
    let mut frame_alpha = FRAME_ALPHA_MAX;
    position_banner(overlay.0, geometry, true)?;
    position_target_frame(frame.0, frame_geometry, true)?;
    active.store(true, Ordering::Release);
    visible.store(true, Ordering::Release);
    ready
        .try_send(Ok(()))
        .map_err(|error| IndicatorError::Backend(format!("signal banner readiness: {error}")))?;

    let mut message = MSG::default();
    while !stop.load(Ordering::Acquire) {
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            if message.message == WM_HOTKEY && message.wParam.0 == HOTKEY_ID as usize {
                interrupted.store(true, Ordering::Release);
                stop.store(true, Ordering::Release);
                break;
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
        validate_target(target_window, target.process_id)?;
        let target_visible = unsafe {
            IsWindowVisible(target_window).as_bool() && !IsIconic(target_window).as_bool()
        };
        if target_visible {
            let next_frame_alpha = breathing_frame_alpha(pulse_started.elapsed());
            if next_frame_alpha != frame_alpha {
                set_overlay_alpha(frame.0, next_frame_alpha)?;
                frame_alpha = next_frame_alpha;
            }
            let next_target_geometry = read_target_geometry(target_window)?;
            let next_geometry = banner_geometry(next_target_geometry);
            if next_geometry != geometry {
                position_banner(
                    overlay.0,
                    next_geometry,
                    next_geometry.width != geometry.width
                        || next_geometry.height != geometry.height,
                )?;
                geometry = next_geometry;
            }
            let next_frame_geometry = target_frame_geometry(next_target_geometry);
            if next_frame_geometry != frame_geometry {
                position_target_frame(
                    frame.0,
                    next_frame_geometry,
                    next_frame_geometry.width != frame_geometry.width
                        || next_frame_geometry.height != frame_geometry.height
                        || next_frame_geometry.thickness != frame_geometry.thickness,
                )?;
                frame_geometry = next_frame_geometry;
            }
            if !visible.swap(true, Ordering::AcqRel) {
                let _ = unsafe { ShowWindow(overlay.0, SW_SHOWNOACTIVATE) };
                let _ = unsafe { ShowWindow(frame.0, SW_SHOWNOACTIVATE) };
            }
        } else if visible.swap(false, Ordering::AcqRel) {
            let _ = unsafe { ShowWindow(overlay.0, SW_HIDE) };
            let _ = unsafe { ShowWindow(frame.0, SW_HIDE) };
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

fn create_overlay(class_name: PCWSTR, label: &str, alpha: u8) -> Result<HWND, IndicatorError> {
    register_class(class_name)?;
    let instance = unsafe { GetModuleHandleW(None) }
        .map_err(|error| IndicatorError::Backend(format!("resolve module handle: {error}")))?;
    let label = wide(label);
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
            PCWSTR(label.as_ptr()),
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
    if let Err(error) = set_overlay_alpha(window, alpha) {
        let _ = unsafe { DestroyWindow(window) };
        return Err(error);
    }
    if let Err(error) = unsafe { SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE) } {
        let _ = unsafe { DestroyWindow(window) };
        return Err(IndicatorError::Backend(format!(
            "exclude indicator from capture: {error}"
        )));
    }
    Ok(window)
}

fn register_class(class_name: PCWSTR) -> Result<(), IndicatorError> {
    let instance = unsafe { GetModuleHandleW(None) }
        .map_err(|error| IndicatorError::Backend(format!("resolve module handle: {error}")))?;
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance.into(),
        lpszClassName: class_name,
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        let error = windows::core::Error::from_win32();
        if error.code().0 as u32 != 1410 {
            return Err(IndicatorError::Backend(format!(
                "register banner window class: {error}"
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct BannerGeometry {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TargetGeometry {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    dpi: u32,
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

fn banner_geometry(target: TargetGeometry) -> BannerGeometry {
    let height = scale(44, target.dpi);
    let width = scale(480, target.dpi)
        .min((target.width - scale(24, target.dpi)).max(scale(240, target.dpi)));
    BannerGeometry {
        x: target.x + (target.width - width) / 2,
        y: target.y + scale(18, target.dpi),
        width,
        height,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TargetFrameGeometry {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    thickness: i32,
}

fn target_frame_geometry(target: TargetGeometry) -> TargetFrameGeometry {
    let outset = scale(6, target.dpi);
    TargetFrameGeometry {
        x: target.x - outset,
        y: target.y - outset,
        width: target.width + outset * 2,
        height: target.height + outset * 2,
        thickness: scale(7, target.dpi),
    }
}

pub(super) fn breathing_frame_alpha(elapsed: Duration) -> u8 {
    let phase = elapsed.as_secs_f64() / FRAME_PULSE_PERIOD.as_secs_f64();
    let wave = (phase * std::f64::consts::TAU).cos().mul_add(0.5, 0.5);
    f64::from(FRAME_ALPHA_MIN)
        .mul_add(1.0 - wave, f64::from(FRAME_ALPHA_MAX) * wave)
        .round() as u8
}

fn position_banner(
    window: HWND,
    geometry: BannerGeometry,
    update_shape: bool,
) -> Result<(), IndicatorError> {
    if update_shape {
        let region = unsafe {
            CreateRoundRectRgn(
                0,
                0,
                geometry.width,
                geometry.height,
                geometry.height,
                geometry.height,
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
    update_shape: bool,
) -> Result<(), IndicatorError> {
    if update_shape {
        let outer = unsafe { CreateRectRgn(0, 0, geometry.width, geometry.height) };
        let inner = unsafe {
            CreateRectRgn(
                geometry.thickness,
                geometry.thickness,
                geometry.width - geometry.thickness,
                geometry.height - geometry.thickness,
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
    if !unsafe { IsWindowVisible(window).as_bool() } {
        return Err(IndicatorError::Backend(
            "Windows did not make the target frame visible".into(),
        ));
    }
    Ok(())
}

fn set_overlay_alpha(window: HWND, alpha: u8) -> Result<(), IndicatorError> {
    unsafe { SetLayeredWindowAttributes(window, COLORREF(0), alpha, LWA_ALPHA) }
        .map_err(|error| IndicatorError::Backend(format!("set overlay alpha: {error}")))
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message != WM_PAINT {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    let mut paint = PAINTSTRUCT::default();
    let device = unsafe { BeginPaint(window, &raw mut paint) };
    if !device.0.is_null() {
        let mut bounds = RECT::default();
        let _ = unsafe { GetClientRect(window, &raw mut bounds) };
        let text_length = unsafe { GetWindowTextLengthW(window) }.max(0) as usize;
        let color = if text_length == 0 {
            FRAME_COLOR
        } else {
            BANNER_COLOR
        };
        let brush = unsafe { CreateSolidBrush(color) };
        let _ = unsafe { FillRect(device, &bounds, brush) };
        let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
        let mut text = vec![0_u16; text_length + 1];
        let copied = unsafe { GetWindowTextW(window, &mut text) }.max(0) as usize;
        text.truncate(copied);
        let dpi = unsafe { GetDpiForWindow(window) }.max(96);
        let font = unsafe {
            CreateFontW(
                -scale(16, dpi),
                0,
                0,
                0,
                FW_SEMIBOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                u32::from(DEFAULT_PITCH.0),
                w!("Segoe UI Semibold"),
            )
        };
        if !font.0.is_null() {
            let previous = unsafe { SelectObject(device, HGDIOBJ(font.0)) };
            let _ = unsafe { SetBkMode(device, TRANSPARENT) };
            let _ = unsafe { SetTextColor(device, COLORREF(0x00FF_FFFF)) };
            let format = windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(
                DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0,
            );
            let _ = unsafe { DrawTextW(device, &mut text, &raw mut bounds, format) };
            let _ = unsafe { SelectObject(device, previous) };
            let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
        }
    }
    let _ = unsafe { EndPaint(window, &paint) };
    LRESULT(0)
}

fn scale(value: i32, dpi: u32) -> i32 {
    ((i64::from(value) * i64::from(dpi) + 48) / 96) as i32
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
