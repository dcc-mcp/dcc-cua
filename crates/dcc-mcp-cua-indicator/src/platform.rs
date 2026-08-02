use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    COLORREF, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CombineRgn, CreateFontW, CreatePolygonRgn,
    CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER,
    DT_END_ELLIPSIS, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FW_SEMIBOLD,
    FillRect, HGDIOBJ, OUT_DEFAULT_PRECIS, PAINTSTRUCT, RGN_DIFF, RGN_ERROR, SelectObject,
    SetBkMode, SetTextColor, SetWindowRgn, TRANSPARENT, WINDING,
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
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClassNameW, GetClientRect,
    GetCursorPos, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    HWND_TOPMOST, IsIconic, IsWindow, IsWindowVisible, LWA_ALPHA, MSG, PM_REMOVE, PeekMessageW,
    RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowPos, ShowWindow,
    TranslateMessage, WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_HOTKEY, WM_PAINT,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP,
};
use windows::core::{HRESULT, PCWSTR, w};

use super::{BannerStatus, BannerTarget, IndicatorError};

const BANNER_CLASS: PCWSTR = w!("DccMcpCuaControlBanner");
const FRAME_CLASS: PCWSTR = w!("DccMcpCuaControlFrame");
const CURSOR_CLASS: PCWSTR = w!("DccMcpCuaControlCursor");
const BANNER_COLOR: COLORREF = COLORREF(0x00FF_840A);
const FRAME_COLOR: COLORREF = COLORREF(0x00FA_A560);
const CURSOR_COLOR: COLORREF = FRAME_COLOR;
const BANNER_ALPHA: u8 = 200;
pub(super) const CURSOR_SIZE: i32 = 52;
pub(super) const FRAME_LAYER_MAX_ALPHA: [u8; 8] = [210, 181, 151, 121, 91, 61, 31, 4];
const FRAME_ALPHA_MIN: u8 = 132;
const FRAME_ALPHA_MAX: u8 = 244;
const FRAME_PULSE_PERIOD: Duration = Duration::from_millis(1_800);
const FRAME_PULSE_INTERVAL: Duration = Duration::from_millis(50);
const HOTKEY_ID: i32 = 0x4443;
const FRAME_INTERVAL: Duration = Duration::from_millis(16);
static ESCAPE_GENERATION: AtomicU64 = AtomicU64::new(0);
static ESCAPE_HUB: OnceLock<Result<EscapeHub, String>> = OnceLock::new();

pub(super) struct PlatformBanner {
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    interrupted: Arc<AtomicBool>,
    visible: Arc<AtomicBool>,
    cursor_position: Arc<Mutex<Option<(f64, f64)>>>,
    thread: Option<JoinHandle<()>>,
}

impl PlatformBanner {
    pub(super) fn start(target: BannerTarget) -> Result<Self, IndicatorError> {
        let escape_hub = escape_hub()?;
        let cursor_position = Arc::new(Mutex::new(None));
        let runtime = BannerRuntime {
            hub_active: Arc::clone(&escape_hub.active),
            generation: ESCAPE_GENERATION.load(Ordering::Acquire),
            cursor_position: Arc::clone(&cursor_position),
        };
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
                cursor_position,
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

    pub(super) fn set_cursor_position(&self, x: f64, y: f64) {
        *self
            .cursor_position
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((x, y));
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

struct RegisteredHotKey;

impl Drop for RegisteredHotKey {
    fn drop(&mut self) {
        let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
    }
}

struct EscapeHub {
    active: Arc<AtomicBool>,
}

struct BannerRuntime {
    hub_active: Arc<AtomicBool>,
    generation: u64,
    cursor_position: Arc<Mutex<Option<(f64, f64)>>>,
}

impl EscapeHub {
    fn start() -> Result<Self, String> {
        let active = Arc::new(AtomicBool::new(false));
        let thread_active = Arc::clone(&active);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        thread::Builder::new()
            .name("dcc-mcp-cua-escape-hub".into())
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
    unsafe { RegisterHotKey(None, HOTKEY_ID, MOD_NOREPEAT, VK_ESCAPE.0 as u32) }
        .map_err(|error| format!("reserve Escape stop key: {error}"))?;
    let _hotkey = RegisteredHotKey;
    active.store(true, Ordering::Release);
    ready
        .try_send(Ok(()))
        .map_err(|error| format!("signal Escape hub readiness: {error}"))?;
    let mut message = MSG::default();
    loop {
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            if message.message == WM_HOTKEY && message.wParam.0 == HOTKEY_ID as usize {
                ESCAPE_GENERATION.fetch_add(1, Ordering::AcqRel);
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        thread::sleep(FRAME_INTERVAL);
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
    runtime: &BannerRuntime,
    ready: &std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<(), IndicatorError> {
    let _dpi_awareness = ThreadDpiAwareness::enter()?;
    let target_window = HWND(target.window_handle as *mut core::ffi::c_void);
    validate_target(target_window, target.process_id)?;
    let overlay = OverlayWindow(create_overlay(BANNER_CLASS, &target.label, BANNER_ALPHA)?);
    let frames = FRAME_LAYER_MAX_ALPHA
        .iter()
        .map(|alpha| create_overlay(FRAME_CLASS, "", *alpha).map(OverlayWindow))
        .collect::<Result<Vec<_>, _>>()?;
    let cursors = FRAME_LAYER_MAX_ALPHA
        .iter()
        .map(|alpha| create_overlay(CURSOR_CLASS, "", *alpha).map(OverlayWindow))
        .collect::<Result<Vec<_>, _>>()?;
    let target_geometry = read_target_geometry(target_window)?;
    let mut geometry = banner_geometry(target_geometry);
    let mut frame_geometries = target_frame_geometries(target_geometry);
    let mut cursor_state = cursor_geometry(target_geometry, None);
    let pulse_started = Instant::now();
    let mut pulse_updated = Duration::ZERO;
    let mut frame_alpha = FRAME_ALPHA_MAX;
    position_banner(overlay.0, geometry, true)?;
    for (frame, geometry) in frames.iter().zip(frame_geometries) {
        position_target_frame(frame.0, geometry, true)?;
    }
    for (layer, cursor) in cursors.iter().enumerate() {
        position_cursor_pointer(cursor.0, cursor_state, layer, true)?;
    }
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
        if escape_generation_changed(
            runtime.generation,
            ESCAPE_GENERATION.load(Ordering::Acquire),
        ) {
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
        if stop.load(Ordering::Acquire) {
            break;
        }
        validate_target(target_window, target.process_id)?;
        let target_visible = unsafe {
            IsWindowVisible(target_window).as_bool() && !IsIconic(target_window).as_bool()
        };
        if target_visible {
            let pulse_elapsed = pulse_started.elapsed();
            let next_frame_alpha = breathing_frame_alpha(pulse_elapsed);
            if pulse_elapsed.saturating_sub(pulse_updated) >= FRAME_PULSE_INTERVAL
                && next_frame_alpha != frame_alpha
            {
                for (frame, maximum) in frames.iter().zip(FRAME_LAYER_MAX_ALPHA) {
                    set_overlay_alpha(frame.0, gradient_frame_alpha(maximum, next_frame_alpha))?;
                }
                for (cursor, maximum) in cursors.iter().zip(FRAME_LAYER_MAX_ALPHA) {
                    set_overlay_alpha(cursor.0, gradient_frame_alpha(maximum, next_frame_alpha))?;
                }
                frame_alpha = next_frame_alpha;
                pulse_updated = pulse_elapsed;
            }
            let next_target_geometry = read_target_geometry(target_window)?;
            let requested_cursor = *runtime
                .cursor_position
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let next_cursor_geometry = cursor_geometry(next_target_geometry, requested_cursor);
            if next_cursor_geometry != cursor_state {
                for (layer, cursor) in cursors.iter().enumerate() {
                    position_cursor_pointer(
                        cursor.0,
                        next_cursor_geometry,
                        layer,
                        cursor_shape_needs_update(
                            cursor_state.visible,
                            next_cursor_geometry.visible,
                            cursor_state.size,
                            next_cursor_geometry.size,
                        ),
                    )?;
                }
                cursor_state = next_cursor_geometry;
            }
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
            let next_frame_geometries = target_frame_geometries(next_target_geometry);
            if next_frame_geometries != frame_geometries {
                for ((frame, next), previous) in frames
                    .iter()
                    .zip(next_frame_geometries)
                    .zip(frame_geometries)
                {
                    position_target_frame(
                        frame.0,
                        next,
                        next.width != previous.width
                            || next.height != previous.height
                            || next.thickness != previous.thickness
                            || next.corner_radius != previous.corner_radius,
                    )?;
                }
                frame_geometries = next_frame_geometries;
            }
            if !visible.swap(true, Ordering::AcqRel) {
                let _ = unsafe { ShowWindow(overlay.0, SW_SHOWNOACTIVATE) };
                for frame in &frames {
                    let _ = unsafe { ShowWindow(frame.0, SW_SHOWNOACTIVATE) };
                }
                if cursor_state.visible {
                    for cursor in &cursors {
                        let _ = unsafe { ShowWindow(cursor.0, SW_SHOWNOACTIVATE) };
                    }
                }
            }
        } else if visible.swap(false, Ordering::AcqRel) {
            let _ = unsafe { ShowWindow(overlay.0, SW_HIDE) };
            for frame in &frames {
                let _ = unsafe { ShowWindow(frame.0, SW_HIDE) };
            }
            for cursor in &cursors {
                let _ = unsafe { ShowWindow(cursor.0, SW_HIDE) };
            }
        }
        thread::sleep(FRAME_INTERVAL);
    }
    Ok(())
}

pub(super) fn escape_generation_changed(started: u64, current: u64) -> bool {
    started != current
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
        if error.code() != HRESULT::from_win32(ERROR_CLASS_ALREADY_EXISTS.0) {
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

#[derive(Clone, Copy, PartialEq, Eq)]
struct CursorGeometry {
    x: i32,
    y: i32,
    size: i32,
    visible: bool,
}

pub(super) fn cursor_shape_needs_update(
    was_visible: bool,
    is_visible: bool,
    previous_size: i32,
    current_size: i32,
) -> bool {
    previous_size != current_size || (!was_visible && is_visible)
}

fn cursor_geometry(target: TargetGeometry, requested: Option<(f64, f64)>) -> CursorGeometry {
    let point = if let Some((x, y)) = requested {
        POINT {
            x: target.x + x.round().clamp(0.0, f64::from(target.width - 1)) as i32,
            y: target.y + y.round().clamp(0.0, f64::from(target.height - 1)) as i32,
        }
    } else {
        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point) }.is_ok() {
            point
        } else {
            POINT {
                x: target.x - 1,
                y: target.y - 1,
            }
        }
    };
    let size = scale(CURSOR_SIZE, target.dpi);
    CursorGeometry {
        x: point.x - size * 8 / 100,
        y: point.y - size * 4 / 100,
        size,
        visible: point.x >= target.x
            && point.x < target.x + target.width
            && point.y >= target.y
            && point.y < target.y + target.height,
    }
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
    let width = (target.width / 4)
        .clamp(scale(300, target.dpi), scale(600, target.dpi))
        .min((target.width - scale(24, target.dpi)).max(scale(240, target.dpi)));
    let height = (width * 44 / 480).max(scale(28, target.dpi));
    BannerGeometry {
        x: target.x + (target.width - width) / 2,
        y: target.y + scale(37, target.dpi),
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
    corner_radius: i32,
}

fn target_frame_geometries(
    target: TargetGeometry,
) -> [TargetFrameGeometry; FRAME_LAYER_MAX_ALPHA.len()] {
    let total_thickness = scale(35, target.dpi);
    let corner_radius = scale(10, target.dpi);
    std::array::from_fn(|layer| {
        let inset = total_thickness * layer as i32 / FRAME_LAYER_MAX_ALPHA.len() as i32;
        let next_inset = total_thickness * (layer as i32 + 1) / FRAME_LAYER_MAX_ALPHA.len() as i32;
        TargetFrameGeometry {
            x: target.x + inset,
            y: target.y + inset,
            width: target.width - inset * 2,
            height: target.height - inset * 2,
            thickness: next_inset - inset,
            corner_radius: (corner_radius - inset).max(1),
        }
    })
}

pub(super) fn breathing_frame_alpha(elapsed: Duration) -> u8 {
    let phase = elapsed.as_secs_f64() / FRAME_PULSE_PERIOD.as_secs_f64();
    let wave = (phase * std::f64::consts::TAU).cos().mul_add(0.5, 0.5);
    f64::from(FRAME_ALPHA_MIN)
        .mul_add(1.0 - wave, f64::from(FRAME_ALPHA_MAX) * wave)
        .round() as u8
}

pub(super) fn gradient_frame_alpha(maximum: u8, breathing: u8) -> u8 {
    ((u16::from(maximum) * u16::from(breathing) + u16::from(FRAME_ALPHA_MAX) / 2)
        / u16::from(FRAME_ALPHA_MAX)) as u8
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
        let outer = unsafe {
            CreateRoundRectRgn(
                0,
                0,
                geometry.width,
                geometry.height,
                geometry.corner_radius * 2,
                geometry.corner_radius * 2,
            )
        };
        let inner_radius = (geometry.corner_radius - geometry.thickness).max(1);
        let inner = unsafe {
            CreateRoundRectRgn(
                geometry.thickness,
                geometry.thickness,
                geometry.width - geometry.thickness,
                geometry.height - geometry.thickness,
                inner_radius * 2,
                inner_radius * 2,
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

fn position_cursor_pointer(
    window: HWND,
    geometry: CursorGeometry,
    layer: usize,
    update_shape: bool,
) -> Result<(), IndicatorError> {
    if !geometry.visible {
        let _ = unsafe { ShowWindow(window, SW_HIDE) };
        return Ok(());
    }
    if update_shape {
        let outer_points = cursor_pointer_polygon(geometry.size, layer);
        let inner_points = cursor_pointer_polygon(geometry.size, layer + 1);
        let outer = unsafe { CreatePolygonRgn(&outer_points, WINDING) };
        let inner = unsafe { CreatePolygonRgn(&inner_points, WINDING) };
        if outer.0.is_null() || inner.0.is_null() {
            let _ = unsafe { DeleteObject(HGDIOBJ(outer.0)) };
            let _ = unsafe { DeleteObject(HGDIOBJ(inner.0)) };
            return Err(IndicatorError::Backend(
                "Windows could not create the mouse marker shape".into(),
            ));
        }
        let combined = unsafe { CombineRgn(Some(outer), Some(outer), Some(inner), RGN_DIFF) };
        let _ = unsafe { DeleteObject(HGDIOBJ(inner.0)) };
        if combined == RGN_ERROR || unsafe { SetWindowRgn(window, Some(outer), true) } == 0 {
            let _ = unsafe { DeleteObject(HGDIOBJ(outer.0)) };
            return Err(IndicatorError::Backend(
                "Windows rejected the mouse marker shape".into(),
            ));
        }
    }
    unsafe {
        SetWindowPos(
            window,
            Some(HWND_TOPMOST),
            geometry.x,
            geometry.y,
            geometry.size,
            geometry.size,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    }
    .map_err(|error| IndicatorError::Backend(format!("position mouse marker: {error}")))?;
    Ok(())
}

fn cursor_pointer_polygon(size: i32, layer: usize) -> [POINT; 7] {
    const SHAPE: [(f64, f64); 7] = [
        (0.08, 0.04),
        (0.08, 0.78),
        (0.29, 0.60),
        (0.46, 0.95),
        (0.63, 0.86),
        (0.46, 0.55),
        (0.76, 0.55),
    ];
    const CENTER: (f64, f64) = (0.34, 0.53);
    let factor = 1.0 - layer as f64 * 0.06;
    SHAPE.map(|(x, y)| POINT {
        x: ((CENTER.0 + (x - CENTER.0) * factor) * f64::from(size)).round() as i32,
        y: ((CENTER.1 + (y - CENTER.1) * factor) * f64::from(size)).round() as i32,
    })
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
        let mut class_name = [0_u16; 64];
        let class_length = unsafe { GetClassNameW(window, &mut class_name) }.max(0) as usize;
        let color = match String::from_utf16_lossy(&class_name[..class_length]).as_str() {
            "DccMcpCuaControlCursor" => CURSOR_COLOR,
            _ if text_length == 0 => FRAME_COLOR,
            _ => BANNER_COLOR,
        };
        let brush = unsafe { CreateSolidBrush(color) };
        let _ = unsafe { FillRect(device, &bounds, brush) };
        let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
        let mut text = vec![0_u16; text_length + 1];
        let copied = unsafe { GetWindowTextW(window, &mut text) }.max(0) as usize;
        text.truncate(copied);
        let dpi = unsafe { GetDpiForWindow(window) }.max(96);
        let font_height = ((bounds.bottom - bounds.top) * 16 / 44).max(scale(11, dpi));
        let font = unsafe {
            CreateFontW(
                -font_height,
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
