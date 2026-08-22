use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    COLORREF, ERROR_CLASS_ALREADY_EXISTS, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CombineRgn, CreateEllipticRgn, CreateFontW,
    CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER,
    DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint,
    FW_NORMAL, FW_SEMIBOLD, FillRect, FillRgn, GetMonitorInfoW, HGDIOBJ, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    RGN_DIFF, RGN_ERROR, SelectObject, SetBkMode, SetTextColor, SetWindowRgn, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
    SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DI_NORMAL, DefWindowProcW, DestroyWindow, DispatchMessageW,
    DrawIconEx, GA_ROOTOWNER, GCLP_HICON, GCLP_HICONSM, GW_HWNDPREV, GetAncestor, GetClassLongPtrW,
    GetClientRect, GetForegroundWindow, GetMessageW, GetPropW, GetWindow, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, HC_ACTION, HHOOK, HICON,
    HTTRANSPARENT, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, ICON_SMALL2, IsIconic, IsWindow,
    IsWindowVisible, KBDLLHOOKSTRUCT, KillTimer, LLKHF_INJECTED, LWA_ALPHA, MA_NOACTIVATE, MSG,
    PM_NOREMOVE, PM_REMOVE, PeekMessageW, PostThreadMessageW, RegisterClassW, RemovePropW,
    SEND_MESSAGE_TIMEOUT_FLAGS, SET_WINDOW_POS_FLAGS, SMTO_ABORTIFHUNG, SPI_GETCLIENTAREAANIMATION,
    SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
    SWP_NOZORDER, SWP_SHOWWINDOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageTimeoutW,
    SetLayeredWindowAttributes, SetPropW, SetTimer, SetWindowPos, SetWindowsHookExW, ShowWindow,
    SystemParametersInfoW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_GETICON, WM_KEYDOWN, WM_KEYUP, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_PAINT,
    WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WNDCLASSW, WNDPROC, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{BOOL, HRESULT, PCWSTR, w};

mod escape_hub_lifecycle;
pub(super) use escape_hub_lifecycle::{
    EscapeHubAcquireAction, EscapeHubReleaseAction, acquire_action as escape_hub_acquire_action,
    release_action as escape_hub_release_action,
};

use super::{
    BannerActivity, BannerActivitySignal, BannerFailure, BannerIndicators, BannerStatus,
    BannerTarget, IndicatorError, IndicatorMotionPolicy, IndicatorMotionStatus,
    TARGET_FRAME_ALPHA_MAX, TARGET_FRAME_GRADIENT_STEPS, TARGET_FRAME_THICKNESS_DIP,
    broadcast_interrupt, indicator_frame_alpha, interrupt_generation, interrupt_generation_changed,
    target_frame_band_alpha, target_frame_has_visible_band, theme_tokens,
    visible_target_frame_band,
};

const BANNER_CLASS: PCWSTR = w!("DccCuaControlBanner");
const FRAME_CLASS: PCWSTR = w!("DccCuaControlFrame");
const OVERLAY_ACTIVITY_PROP: PCWSTR = w!("DccCuaBannerActivity");
const OVERLAY_RECORDING_PROP: PCWSTR = w!("DccCuaBannerRecording");
const OVERLAY_LIVE_PROP: PCWSTR = w!("DccCuaBannerLiveObservation");
const OVERLAY_ICON_PROP: PCWSTR = w!("DccCuaBannerIcon");
const OVERLAY_THEME_PROP: PCWSTR = w!("DccCuaBannerTheme");
const BANNER_ALPHA: u8 = 248;
const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const ESCAPE_HOOK_WATCHDOG_INTERVAL_MS: u32 = 1_000;
// Starting the Win32 overlay can be delayed by a saturated interactive
// desktop (for example while a DCC is cooking and UIA providers are hung).
// Keep the wait bounded, but give the dedicated banner thread enough time to
// create and present every frame before refusing the control session.
const BANNER_START_TIMEOUT: Duration = Duration::from_secs(8);
const SURFACE: COLORREF = rgb(
    theme_tokens::SURFACE.0,
    theme_tokens::SURFACE.1,
    theme_tokens::SURFACE.2,
);
const LINE: COLORREF = rgb(
    theme_tokens::LINE.0,
    theme_tokens::LINE.1,
    theme_tokens::LINE.2,
);
const TEXT: COLORREF = rgb(
    theme_tokens::TEXT.0,
    theme_tokens::TEXT.1,
    theme_tokens::TEXT.2,
);
const MUTED: COLORREF = rgb(
    theme_tokens::MUTED.0,
    theme_tokens::MUTED.1,
    theme_tokens::MUTED.2,
);
const ACCENT: COLORREF = rgb(
    theme_tokens::ACCENT.0,
    theme_tokens::ACCENT.1,
    theme_tokens::ACCENT.2,
);
const RECORDING: COLORREF = rgb(
    theme_tokens::RECORDING.0,
    theme_tokens::RECORDING.1,
    theme_tokens::RECORDING.2,
);
const SURFACE_VARIANTS: [COLORREF; 4] =
    [SURFACE, rgb(24, 36, 58), rgb(42, 28, 58), rgb(20, 48, 43)];
const LINE_VARIANTS: [COLORREF; 4] = [
    LINE,
    rgb(73, 143, 214),
    rgb(167, 91, 220),
    rgb(67, 183, 135),
];
static ESCAPE_HUB_STATE: OnceLock<Mutex<EscapeHubState>> = OnceLock::new();
static ESCAPE_DOWN: AtomicBool = AtomicBool::new(false);
static ACTIVE_BANNERS: AtomicUsize = AtomicUsize::new(0);

const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF((blue as u32) << 16 | (green as u32) << 8 | red as u32)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TargetPresentationPolicy {
    Hidden,
    ExactTargetForeground,
    OwnedModalForeground,
    TargetScopedBehindUnrelatedForeground,
}

impl TargetPresentationPolicy {
    pub(super) const fn is_visible(self) -> bool {
        !matches!(self, Self::Hidden)
    }
}

pub(super) struct TargetOverlaySyncState {
    previous: TargetPresentationPolicy,
}

impl TargetOverlaySyncState {
    pub(super) const fn new(previous: TargetPresentationPolicy) -> Self {
        Self { previous }
    }

    /// Return true only when a presentation transition can put the exact target
    /// above its unowned overlays. Stable polling ticks deliberately do no work.
    pub(super) fn observe(&mut self, current: TargetPresentationPolicy) -> bool {
        let previous = std::mem::replace(&mut self.previous, current);
        let entered_target_foreground = current != previous
            && matches!(
                current,
                TargetPresentationPolicy::ExactTargetForeground
                    | TargetPresentationPolicy::OwnedModalForeground
            );
        current.is_visible()
            && (matches!(previous, TargetPresentationPolicy::Hidden) || entered_target_foreground)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn target_presentation_policy(
    target_visible: bool,
    target_minimized: bool,
    target_window: u64,
    target_root_owner: u64,
    foreground_window: Option<u64>,
    foreground_root_owner: Option<u64>,
) -> TargetPresentationPolicy {
    if !target_visible || target_minimized {
        return TargetPresentationPolicy::Hidden;
    }
    if foreground_window == Some(target_window) {
        TargetPresentationPolicy::ExactTargetForeground
    } else if foreground_root_owner == Some(target_root_owner) {
        TargetPresentationPolicy::OwnedModalForeground
    } else {
        TargetPresentationPolicy::TargetScopedBehindUnrelatedForeground
    }
}

fn window_handle_value(window: HWND) -> u64 {
    window.0 as usize as u64
}

fn root_owner_value(window: HWND) -> u64 {
    let root_owner = unsafe { GetAncestor(window, GA_ROOTOWNER) };
    if root_owner.0.is_null() {
        window_handle_value(window)
    } else {
        window_handle_value(root_owner)
    }
}

fn current_target_presentation(target_window: HWND) -> TargetPresentationPolicy {
    let foreground = unsafe { GetForegroundWindow() };
    let foreground_window = (!foreground.0.is_null()).then(|| window_handle_value(foreground));
    let foreground_root_owner = (!foreground.0.is_null()).then(|| root_owner_value(foreground));
    target_presentation_policy(
        unsafe { IsWindowVisible(target_window).as_bool() },
        unsafe { IsIconic(target_window).as_bool() },
        window_handle_value(target_window),
        root_owner_value(target_window),
        foreground_window,
        foreground_root_owner,
    )
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
    frame_visible: Arc<AtomicBool>,
    inside_target: Arc<AtomicBool>,
    activity: Arc<BannerActivitySignal>,
    recording: Arc<AtomicBool>,
    live_observation: Arc<AtomicBool>,
    motion: IndicatorMotionStatus,
    backend_failure: Arc<Mutex<Option<BannerFailure>>>,
    thread: Option<JoinHandle<()>>,
}

impl PlatformBanner {
    pub(super) fn start(
        target: BannerTarget,
        requested_motion: IndicatorMotionPolicy,
        generation: u64,
    ) -> Result<Self, IndicatorError> {
        let escape_hub = EscapeHubLease::acquire()?;
        let motion = IndicatorMotionStatus::resolve_from_system(
            requested_motion,
            system_animation_preference(),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(false));
        let interrupted = Arc::new(AtomicBool::new(false));
        let visible = Arc::new(AtomicBool::new(false));
        let frame_visible = Arc::new(AtomicBool::new(false));
        let inside_target = Arc::new(AtomicBool::new(false));
        let activity = Arc::new(BannerActivitySignal::new(BannerActivity::Connecting));
        let recording = Arc::new(AtomicBool::new(false));
        let live_observation = Arc::new(AtomicBool::new(false));
        let backend_failure = Arc::new(Mutex::new(None));
        let runtime = BannerRuntime {
            hub_active: Arc::clone(&escape_hub.active),
            generation,
            activity: Arc::clone(&activity),
            recording: Arc::clone(&recording),
            live_observation: Arc::clone(&live_observation),
        };
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let thread_stop = Arc::clone(&stop);
        let thread_active = Arc::clone(&active);
        let thread_interrupted = Arc::clone(&interrupted);
        let thread_visible = Arc::clone(&visible);
        let thread_frame_visible = Arc::clone(&frame_visible);
        let thread_inside_target = Arc::clone(&inside_target);
        let thread_backend_failure = Arc::clone(&backend_failure);
        let thread_escape_hub = escape_hub;
        let thread = thread::Builder::new()
            .name("dcc-cua-control-banner".into())
            .spawn(move || {
                let _escape_hub = thread_escape_hub;
                let result = run_banner(
                    target,
                    &thread_stop,
                    &thread_active,
                    &thread_interrupted,
                    &thread_visible,
                    &thread_frame_visible,
                    &thread_inside_target,
                    &runtime,
                    motion,
                    &ready_tx,
                );
                if let Err(error) = result {
                    if let Ok(mut stored) = thread_backend_failure.lock() {
                        *stored = Some(BannerFailure::from(&error));
                    }
                    let _ = ready_tx.try_send(Err(error));
                }
                thread_active.store(false, Ordering::Release);
                thread_visible.store(false, Ordering::Release);
                thread_frame_visible.store(false, Ordering::Release);
            })
            .map_err(|error| {
                IndicatorError::Backend(format!("failed to start banner thread: {error}"))
            })?;

        match ready_rx.recv_timeout(BANNER_START_TIMEOUT) {
            Ok(Ok(())) => Ok(Self {
                stop,
                active,
                interrupted,
                visible,
                frame_visible,
                inside_target,
                activity,
                recording,
                live_observation,
                motion,
                backend_failure,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                stop.store(true, Ordering::Release);
                let _ = thread.join();
                Err(error)
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
        let indicators = BannerIndicators {
            recording: self.recording.load(Ordering::Acquire),
            live_observation: self.live_observation.load(Ordering::Acquire),
        };
        let activity = self.activity.load().presented_with(indicators);
        let failure = self.backend_failure.lock().map_or_else(
            |_| {
                Some(BannerFailure::from(&IndicatorError::Backend(
                    "control-banner error state is unavailable".into(),
                )))
            },
            |failure| failure.clone(),
        );
        let last_error = failure.as_ref().map(|failure| failure.message.clone());
        BannerStatus {
            backend: "win32",
            healthy: last_error.is_none(),
            running: self.active.load(Ordering::Acquire),
            last_error,
            failure,
            visible: self.visible.load(Ordering::Acquire),
            target_frame_visible: self.frame_visible.load(Ordering::Acquire),
            interrupted: self.interrupted(),
            stop_key: "Escape",
            label: String::new(),
            activity,
            activity_label: activity.localized_label(&system_language_tag()).into(),
            recording: indicators.recording,
            live_observation: indicators.live_observation,
            placement: if self.inside_target.load(Ordering::Acquire) {
                "target_safe_inset"
            } else {
                "window_edge"
            },
            color: activity.color(),
            motion: self.motion,
        }
    }

    pub(super) fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Acquire)
    }

    pub(super) fn set_activity(&self, activity: BannerActivity) {
        self.activity.set(activity);
    }

    pub(super) fn activity_handle(&self) -> Arc<BannerActivitySignal> {
        Arc::clone(&self.activity)
    }

    pub(super) fn set_recording(&self, recording: bool) {
        self.recording.store(recording, Ordering::Release);
    }

    pub(super) fn set_live_observation(&self, live_observation: bool) {
        self.live_observation
            .store(live_observation, Ordering::Release);
    }
}

impl Drop for PlatformBanner {
    fn drop(&mut self) {
        self.activity.set(BannerActivity::Stopping);
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
        let _ = unsafe { RemovePropW(self.0, OVERLAY_RECORDING_PROP) };
        let _ = unsafe { RemovePropW(self.0, OVERLAY_LIVE_PROP) };
        let _ = unsafe { RemovePropW(self.0, OVERLAY_ICON_PROP) };
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

/// Hidden PMv2 window used to resolve the selected monitor's effective DPI.
///
/// The foreign target can legitimately report only 96 or the system DPI. A
/// controlled window created on the indicator thread instead follows monitor
/// migration while remaining ownerless, non-activating, and invisible.
pub(super) struct DpiProbeWindow(HWND);

impl DpiProbeWindow {
    pub(super) fn create(_awareness: &ThreadDpiAwareness) -> Result<Self, IndicatorError> {
        create_window(FRAME_CLASS, Some(frame_window_proc), "", 0).map(Self)
    }

    pub(super) const fn handle(&self) -> HWND {
        self.0
    }

    fn dpi_for_target(&self, target: HWND, target_rect: RECT) -> Option<u32> {
        if unsafe { IsWindowVisible(self.handle()).as_bool() } {
            return None;
        }
        let monitor = unsafe { MonitorFromWindow(target, MONITOR_DEFAULTTONEAREST) };
        if monitor.0.is_null() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            return None;
        }
        let (x, y) = dpi_probe_point(target_rect, info.rcMonitor);
        unsafe {
            SetWindowPos(
                self.handle(),
                None,
                x,
                y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE,
            )
        }
        .ok()?;
        if unsafe { IsWindowVisible(self.handle()).as_bool() } {
            return None;
        }
        let dpi = unsafe { GetDpiForWindow(self.handle()) };
        (dpi > 0).then_some(dpi)
    }
}

impl Drop for DpiProbeWindow {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.handle()) };
    }
}

struct RegisteredKeyboardHook(HHOOK);

impl RegisteredKeyboardHook {
    fn install() -> Result<Self, String> {
        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_level_keyboard_hook), None, 0) }
            .map(Self)
            .map_err(|error| format!("install Escape stop hook: {error}"))
    }
}

impl Drop for RegisteredKeyboardHook {
    fn drop(&mut self) {
        let _ = unsafe { UnhookWindowsHookEx(self.0) };
    }
}

struct RegisteredThreadTimer(usize);

impl RegisteredThreadTimer {
    fn install() -> Result<Self, String> {
        let timer = unsafe { SetTimer(None, 0, ESCAPE_HOOK_WATCHDOG_INTERVAL_MS, None) };
        if timer == 0 {
            return Err(format!(
                "install Escape hook watchdog timer: {}",
                windows::core::Error::from_win32()
            ));
        }
        Ok(Self(timer))
    }
}

impl Drop for RegisteredThreadTimer {
    fn drop(&mut self) {
        let _ = unsafe { KillTimer(None, self.0) };
    }
}

struct EscapeHub {
    active: Arc<AtomicBool>,
    stop_requested: Arc<AtomicBool>,
    thread_id: Arc<AtomicU32>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct EscapeHubState {
    hub: Option<EscapeHub>,
    leases: usize,
}

struct EscapeHubLease {
    active: Arc<AtomicBool>,
}

struct BannerRuntime {
    hub_active: Arc<AtomicBool>,
    generation: u64,
    activity: Arc<BannerActivitySignal>,
    recording: Arc<AtomicBool>,
    live_observation: Arc<AtomicBool>,
}

impl EscapeHub {
    fn start() -> Result<Self, String> {
        let active = Arc::new(AtomicBool::new(false));
        let stop_requested = Arc::new(AtomicBool::new(false));
        let thread_id = Arc::new(AtomicU32::new(0));
        let thread_active = Arc::clone(&active);
        let worker_stop_requested = Arc::clone(&stop_requested);
        let worker_thread_id = Arc::clone(&thread_id);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("dcc-cua-escape-hub".into())
            .spawn(move || {
                let result = run_escape_hub(
                    &thread_active,
                    &worker_stop_requested,
                    &worker_thread_id,
                    &ready_tx,
                );
                if let Err(error) = result {
                    let _ = ready_tx.try_send(Err(error));
                }
                thread_active.store(false, Ordering::Release);
            })
            .map_err(|error| format!("failed to start Escape hub: {error}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(Self {
                active,
                stop_requested,
                thread_id,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(error) => {
                stop_requested.store(true, Ordering::Release);
                request_escape_hub_stop(thread_id.load(Ordering::Acquire));
                let _ = thread.join();
                Err(format!("timed out while starting Escape hub: {error}"))
            }
        }
    }

    fn stop(&mut self) {
        self.stop_requested.store(true, Ordering::Release);
        let thread_id = self.thread_id.swap(0, Ordering::AcqRel);
        let thread_is_running = self
            .thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished());
        if thread_is_running {
            request_escape_hub_stop(thread_id);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.active.store(false, Ordering::Release);
    }
}

impl Drop for EscapeHub {
    fn drop(&mut self) {
        self.stop();
    }
}

impl EscapeHubLease {
    fn acquire() -> Result<Self, IndicatorError> {
        let state = ESCAPE_HUB_STATE.get_or_init(|| Mutex::new(EscapeHubState::default()));
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let active = state
            .hub
            .as_ref()
            .is_some_and(|hub| hub.active.load(Ordering::Acquire));
        match escape_hub_acquire_action(state.hub.is_some(), active) {
            EscapeHubAcquireAction::Reuse => {}
            EscapeHubAcquireAction::Start | EscapeHubAcquireAction::Restart => {
                drop(state.hub.take());
                state.hub = Some(EscapeHub::start().map_err(IndicatorError::Backend)?);
            }
        }
        let active = Arc::clone(
            &state
                .hub
                .as_ref()
                .expect("Escape hub is initialized before leasing")
                .active,
        );
        state.leases = state.leases.checked_add(1).ok_or_else(|| {
            IndicatorError::Backend("Escape hub banner lease limit was exhausted".into())
        })?;
        ACTIVE_BANNERS.fetch_add(1, Ordering::AcqRel);
        Ok(Self { active })
    }
}

impl Drop for EscapeHubLease {
    fn drop(&mut self) {
        let state = ESCAPE_HUB_STATE.get_or_init(|| Mutex::new(EscapeHubState::default()));
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        debug_assert!(state.leases > 0, "Escape hub lease underflow");
        if state.leases == 0 {
            return;
        }
        let release = escape_hub_release_action(state.leases);
        state.leases -= 1;
        if ACTIVE_BANNERS.fetch_sub(1, Ordering::AcqRel) == 1 {
            ESCAPE_DOWN.store(false, Ordering::Release);
        }
        if release == EscapeHubReleaseAction::Stop
            && let Some(hub) = state.hub.take()
        {
            drop(hub);
        }
    }
}

fn request_escape_hub_stop(thread_id: u32) {
    if thread_id != 0 {
        let _ = unsafe { PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
    }
}

fn run_escape_hub(
    active: &AtomicBool,
    stop_requested: &AtomicBool,
    thread_id: &AtomicU32,
    ready: &std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    let mut message = MSG::default();
    let _ = unsafe { PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE) };
    thread_id.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    if stop_requested.load(Ordering::Acquire) {
        return Ok(());
    }
    let mut hook = RegisteredKeyboardHook::install()?;
    let watchdog = RegisteredThreadTimer::install()?;
    if stop_requested.load(Ordering::Acquire) {
        return Ok(());
    }
    active.store(true, Ordering::Release);
    ready
        .try_send(Ok(()))
        .map_err(|error| format!("signal Escape hub readiness: {error}"))?;
    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 == -1 {
            return Err(format!(
                "read Escape hub message: {}",
                windows::core::Error::from_win32()
            ));
        }
        if result.0 == 0 {
            break;
        }
        if stop_requested.load(Ordering::Acquire) {
            break;
        }
        if message.message == WM_TIMER && message.wParam.0 == watchdog.0 {
            hook = RegisteredKeyboardHook::install()?;
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    drop(hook);
    Ok(())
}

unsafe extern "system" fn low_level_keyboard_hook(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code == HC_ACTION as i32 {
        let event = wparam.0 as u32;
        let keyboard = unsafe { (lparam.0 as *const KBDLLHOOKSTRUCT).as_ref() };
        if let Some(is_down) = keyboard.and_then(|keyboard| {
            escape_key_transition_for_active_banners(
                ACTIVE_BANNERS.load(Ordering::Acquire),
                code,
                event,
                keyboard.vkCode,
                keyboard.flags.0,
            )
        }) {
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

pub(super) fn escape_key_transition_for_active_banners(
    active_banners: usize,
    code: i32,
    message: u32,
    virtual_key: u32,
    flags: u32,
) -> Option<bool> {
    if active_banners == 0 {
        return None;
    }
    escape_key_transition(code, message, virtual_key, flags)
}

pub(super) fn escape_key_transition(
    code: i32,
    message: u32,
    virtual_key: u32,
    flags: u32,
) -> Option<bool> {
    if code != HC_ACTION as i32
        || virtual_key != VK_ESCAPE.0 as u32
        || flags & LLKHF_INJECTED.0 != 0
    {
        return None;
    }
    match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(true),
        WM_KEYUP | WM_SYSKEYUP => Some(false),
        _ => None,
    }
}

pub(super) struct ThreadDpiAwareness {
    previous: DPI_AWARENESS_CONTEXT,
}

impl ThreadDpiAwareness {
    pub(super) fn enter() -> Result<Self, IndicatorError> {
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

fn system_animation_preference() -> Option<bool> {
    let mut enabled = BOOL(1);
    unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut enabled).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .ok()
    .map(|()| enabled.as_bool())
}

#[allow(clippy::too_many_arguments)]
fn run_banner(
    target: BannerTarget,
    stop: &AtomicBool,
    active: &AtomicBool,
    interrupted: &AtomicBool,
    visible: &AtomicBool,
    frame_visible: &AtomicBool,
    inside_target: &AtomicBool,
    runtime: &BannerRuntime,
    motion: IndicatorMotionStatus,
    ready: &std::sync::mpsc::SyncSender<Result<(), IndicatorError>>,
) -> Result<(), IndicatorError> {
    let dpi_awareness = ThreadDpiAwareness::enter()?;
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
    let dpi_probe = DpiProbeWindow::create(&dpi_awareness).ok();
    let target_geometry = read_target_geometry(target_window, dpi_probe.as_ref())?;
    let mut compact_hidden = compact_target(target_geometry);
    let mut geometry = banner_geometry(target_geometry, read_monitor_geometry(target_window)?);
    let mut frame_geometry = target_frame_geometry(target_geometry);
    position_banner(overlay.0, target_window, geometry, true)?;
    for (band, frame) in frames.iter().enumerate() {
        set_overlay_alpha(
            frame.0,
            target_frame_band_alpha(TARGET_FRAME_ALPHA_MAX, band),
        )?;
        position_target_frame(frame.0, target_window, frame_geometry, band, true)?;
    }
    inside_target.store(geometry.inside_target, Ordering::Release);
    let initial_presentation = current_target_presentation(target_window);
    let mut overlay_sync = TargetOverlaySyncState::new(initial_presentation);
    let initial_visible = initial_presentation.is_visible() && !compact_hidden;
    if !initial_visible {
        let _ = unsafe { ShowWindow(overlay.0, SW_HIDE) };
        for frame in &frames {
            let _ = unsafe { ShowWindow(frame.0, SW_HIDE) };
        }
    }
    let mut displayed_activity = BannerActivity::Connecting;
    let mut displayed_recording = false;
    let mut displayed_live_observation = false;
    let pulse_started = Instant::now();
    let mut frame_alpha = TARGET_FRAME_ALPHA_MAX;
    active.store(true, Ordering::Release);
    visible.store(initial_visible, Ordering::Release);
    frame_visible.store(
        initial_visible && target_frame_has_visible_band(frame_geometry.thickness),
        Ordering::Release,
    );
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
        let indicators = BannerIndicators {
            recording: runtime.recording.load(Ordering::Acquire),
            live_observation: runtime.live_observation.load(Ordering::Acquire),
        };
        let next_activity = runtime.activity.load().presented_with(indicators);
        if next_activity != displayed_activity
            || indicators.recording != displayed_recording
            || indicators.live_observation != displayed_live_observation
        {
            set_activity_property(overlay.0, next_activity)?;
            set_recording_property(overlay.0, indicators.recording)?;
            set_live_observation_property(overlay.0, indicators.live_observation)?;
            let _ = unsafe { InvalidateRect(Some(overlay.0), None, false) };
            displayed_activity = next_activity;
            displayed_recording = indicators.recording;
            displayed_live_observation = indicators.live_observation;
        }
        let next_frame_alpha = indicator_frame_alpha(motion, pulse_started.elapsed());
        if next_frame_alpha != frame_alpha {
            for (band, frame) in frames.iter().enumerate() {
                set_overlay_alpha(frame.0, target_frame_band_alpha(next_frame_alpha, band))?;
            }
            frame_alpha = next_frame_alpha;
        }
        validate_target(target_window, target.process_id)?;
        let presentation = current_target_presentation(target_window);
        let reassert_z_order = overlay_sync.observe(presentation);
        if presentation.is_visible() {
            let next_target = read_target_geometry(target_window, dpi_probe.as_ref())?;
            let next_compact_hidden = compact_target(next_target);
            if next_compact_hidden != compact_hidden {
                compact_hidden = next_compact_hidden;
                let command = if compact_hidden {
                    SW_HIDE
                } else {
                    SW_SHOWNOACTIVATE
                };
                let _ = unsafe { ShowWindow(overlay.0, command) };
                for frame in &frames {
                    let _ = unsafe { ShowWindow(frame.0, command) };
                }
                visible.store(!compact_hidden, Ordering::Release);
                frame_visible.store(
                    !compact_hidden && target_frame_has_visible_band(frame_geometry.thickness),
                    Ordering::Release,
                );
            }
            let next_geometry = banner_geometry(next_target, read_monitor_geometry(target_window)?);
            if next_geometry != geometry {
                position_banner(
                    overlay.0,
                    target_window,
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
                        target_window,
                        next_frame_geometry,
                        band,
                        next_frame_geometry.width != frame_geometry.width
                            || next_frame_geometry.height != frame_geometry.height
                            || next_frame_geometry.thickness != frame_geometry.thickness,
                    )?;
                }
                frame_geometry = next_frame_geometry;
            }
            if reassert_z_order {
                position_banner(overlay.0, target_window, geometry, false)?;
                for (band, frame) in frames.iter().enumerate() {
                    position_target_frame(frame.0, target_window, frame_geometry, band, false)?;
                }
            } else if !compact_hidden && !visible.load(Ordering::Acquire) {
                let _ = unsafe { ShowWindow(overlay.0, SW_SHOWNOACTIVATE) };
                for (band, frame) in frames.iter().enumerate() {
                    let command =
                        if visible_target_frame_band(frame_geometry.thickness, band).is_some() {
                            SW_SHOWNOACTIVATE
                        } else {
                            SW_HIDE
                        };
                    let _ = unsafe { ShowWindow(frame.0, command) };
                }
            }
            visible.store(!compact_hidden, Ordering::Release);
            frame_visible.store(
                !compact_hidden && target_frame_has_visible_band(frame_geometry.thickness),
                Ordering::Release,
            );
        } else if visible.swap(false, Ordering::AcqRel) {
            let _ = unsafe { ShowWindow(overlay.0, SW_HIDE) };
            for frame in &frames {
                let _ = unsafe { ShowWindow(frame.0, SW_HIDE) };
            }
            frame_visible.store(false, Ordering::Release);
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
    let theme = theme_for_identity(identity);
    if let Err(error) = unsafe {
        SetPropW(
            window,
            OVERLAY_THEME_PROP,
            Some(HANDLE((theme + 1) as *mut core::ffi::c_void)),
        )
    } {
        let _ = unsafe { DestroyWindow(window) };
        return Err(IndicatorError::Backend(format!(
            "set banner theme: {error}"
        )));
    }
    if let Err(error) = set_activity_property(window, activity) {
        let _ = unsafe { DestroyWindow(window) };
        return Err(error);
    }
    if let Err(error) = set_recording_property(window, false) {
        let _ = unsafe { DestroyWindow(window) };
        return Err(error);
    }
    if let Err(error) = set_live_observation_property(window, false) {
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

pub(super) fn create_frame_overlay() -> Result<HWND, IndicatorError> {
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
                WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_TRANSPARENT.0 | WS_EX_LAYERED.0,
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

fn set_recording_property(window: HWND, recording: bool) -> Result<(), IndicatorError> {
    if recording {
        unsafe {
            SetPropW(
                window,
                OVERLAY_RECORDING_PROP,
                Some(HANDLE(
                    core::ptr::NonNull::<core::ffi::c_void>::dangling().as_ptr(),
                )),
            )
        }
        .map_err(|error| IndicatorError::Backend(format!("set banner recording state: {error}")))
    } else {
        let _ = unsafe { RemovePropW(window, OVERLAY_RECORDING_PROP) };
        Ok(())
    }
}

fn set_live_observation_property(
    window: HWND,
    live_observation: bool,
) -> Result<(), IndicatorError> {
    if live_observation {
        unsafe {
            SetPropW(
                window,
                OVERLAY_LIVE_PROP,
                Some(HANDLE(
                    core::ptr::NonNull::<core::ffi::c_void>::dangling().as_ptr(),
                )),
            )
        }
        .map_err(|error| {
            IndicatorError::Backend(format!("set banner live-observation state: {error}"))
        })
    } else {
        let _ = unsafe { RemovePropW(window, OVERLAY_LIVE_PROP) };
        Ok(())
    }
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

fn read_target_geometry(
    target: HWND,
    dpi_probe: Option<&DpiProbeWindow>,
) -> Result<TargetGeometry, IndicatorError> {
    let mut window_rect = RECT::default();
    unsafe { GetWindowRect(target, &mut window_rect) }
        .map_err(|error| IndicatorError::Backend(format!("read target bounds: {error}")))?;
    let mut extended_frame = RECT::default();
    let extended_frame = unsafe {
        DwmGetWindowAttribute(
            target,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut extended_frame).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .ok()
    .map(|()| extended_frame);
    let target_rect = visible_target_rect(window_rect, extended_frame);
    let probe_dpi = dpi_probe.and_then(|probe| probe.dpi_for_target(target, target_rect));
    let dpi = resolve_indicator_dpi(probe_dpi, unsafe { GetDpiForWindow(target) });
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

pub(super) fn resolve_indicator_dpi(
    controlled_probe_dpi: Option<u32>,
    target_window_dpi: u32,
) -> u32 {
    controlled_probe_dpi
        .filter(|dpi| *dpi > 0)
        .unwrap_or(target_window_dpi)
        .max(96)
}

pub(super) fn dpi_probe_point(target_rect: RECT, monitor_rect: RECT) -> (i32, i32) {
    let intersection_left = target_rect.left.max(monitor_rect.left);
    let intersection_top = target_rect.top.max(monitor_rect.top);
    let intersection_right = target_rect.right.min(monitor_rect.right);
    let intersection_bottom = target_rect.bottom.min(monitor_rect.bottom);
    let (left, top, right, bottom) =
        if intersection_right > intersection_left && intersection_bottom > intersection_top {
            (
                intersection_left,
                intersection_top,
                intersection_right,
                intersection_bottom,
            )
        } else {
            (
                monitor_rect.left,
                monitor_rect.top,
                monitor_rect.right,
                monitor_rect.bottom,
            )
        };
    (left + (right - left) / 2, top + (bottom - top) / 2)
}

pub(super) fn visible_target_rect(fallback: RECT, extended_frame: Option<RECT>) -> RECT {
    extended_frame
        .filter(|bounds| bounds.right > bounds.left && bounds.bottom > bounds.top)
        .unwrap_or(fallback)
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

pub(super) fn compact_target(target: TargetGeometry) -> bool {
    target.width < scale(800, target.dpi) || target.height < scale(500, target.dpi)
}

pub(super) fn theme_for_identity(identity: &str) -> usize {
    let hash = identity.bytes().fold(0_u32, |value, byte| {
        value.wrapping_mul(33).wrapping_add(u32::from(byte))
    });
    (hash as usize) % SURFACE_VARIANTS.len()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct TargetFrameGeometry {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) thickness: i32,
    pub(super) corner_radius: i32,
}

pub(super) fn target_frame_geometry(target: TargetGeometry) -> TargetFrameGeometry {
    let safe_depth = target.width.min(target.height).saturating_sub(1).max(0) / 2;
    TargetFrameGeometry {
        x: target.x,
        y: target.y,
        width: target.width,
        height: target.height,
        thickness: scale(TARGET_FRAME_THICKNESS_DIP, target.dpi).min(safe_depth),
        corner_radius: scale(10, target.dpi),
    }
}

/// Keep the overlay immediately above the exact target without joining its
/// accessibility tree or entering the global topmost band.
pub(super) fn position_target_scoped_overlay(
    window: HWND,
    target: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    show_window: bool,
) -> windows::core::Result<()> {
    position_target_scoped_overlay_with_z_order(
        TargetScopedOverlayPosition {
            window,
            target,
            x,
            y,
            width,
            height,
            show_window,
        },
        false,
    )
}

fn position_banner_overlay(
    window: HWND,
    target: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    show_window: bool,
) -> windows::core::Result<()> {
    position_target_scoped_overlay_with_z_order(
        TargetScopedOverlayPosition {
            window,
            target,
            x,
            y,
            width,
            height,
            show_window,
        },
        true,
    )
}

struct TargetScopedOverlayPosition {
    window: HWND,
    target: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    show_window: bool,
}

fn position_target_scoped_overlay_with_z_order(
    position: TargetScopedOverlayPosition,
    topmost: bool,
) -> windows::core::Result<()> {
    let TargetScopedOverlayPosition {
        window,
        target,
        x,
        y,
        width,
        height,
        show_window,
    } = position;
    unsafe {
        SetWindowPos(
            window,
            Some(HWND_NOTOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )?;
        let insert_after = if topmost {
            HWND_TOPMOST
        } else {
            let previous = GetWindow(target, GW_HWNDPREV).unwrap_or(HWND(core::ptr::null_mut()));
            if previous.0.is_null() {
                HWND_TOP
            } else {
                previous
            }
        };
        let visibility = if show_window {
            SWP_SHOWWINDOW
        } else {
            SET_WINDOW_POS_FLAGS(0)
        };
        SetWindowPos(
            window,
            Some(insert_after),
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | visibility | SWP_NOOWNERZORDER,
        )
    }
}

fn position_banner(
    window: HWND,
    target: HWND,
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
    position_banner_overlay(
        window,
        target,
        geometry.x,
        geometry.y,
        geometry.width,
        geometry.height,
        true,
    )
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
    target: HWND,
    geometry: TargetFrameGeometry,
    band: usize,
    update_shape: bool,
) -> Result<(), IndicatorError> {
    let Some((outer_inset, inner_inset)) = visible_target_frame_band(geometry.thickness, band)
    else {
        let _ = unsafe { ShowWindow(window, SW_HIDE) };
        return Ok(());
    };
    if update_shape {
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
    position_target_scoped_overlay(
        window,
        target,
        geometry.x,
        geometry.y,
        geometry.width,
        geometry.height,
        true,
    )
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
        let theme_value = unsafe { GetPropW(window, OVERLAY_THEME_PROP) };
        let theme = theme_value.0 as usize;
        paint_surface(device, bounds, dpi, theme);
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
        let recording = !unsafe { GetPropW(window, OVERLAY_RECORDING_PROP) }
            .0
            .is_null();
        let live_observation = !unsafe { GetPropW(window, OVERLAY_LIVE_PROP) }.0.is_null();
        paint_activity(device, activity, dpi);
        let identity = window_text(window);
        paint_copy(
            device,
            bounds,
            dpi,
            &identity,
            activity,
            recording,
            live_observation,
        );
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

fn paint_surface(device: windows::Win32::Graphics::Gdi::HDC, bounds: RECT, dpi: u32, theme: usize) {
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
        let border = unsafe { CreateSolidBrush(LINE_VARIANTS[theme.min(LINE_VARIANTS.len() - 1)]) };
        let surface =
            unsafe { CreateSolidBrush(SURFACE_VARIANTS[theme.min(SURFACE_VARIANTS.len() - 1)]) };
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
    recording: bool,
    live_observation: bool,
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
    let recording_width = if recording { scale(48, dpi) } else { 0 };
    let live_width = if live_observation { scale(52, dpi) } else { 0 };
    let badges_left = divider_x - recording_width - live_width;
    let text_right = badges_left - scale(7, dpi);
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
    let mut badge_left = badges_left;
    if recording {
        paint_persistent_badge(device, "REC", badge_left, recording_width, RECORDING, dpi);
        badge_left += recording_width;
    }
    if live_observation {
        let live_color = BannerActivity::Observing.color();
        paint_persistent_badge(
            device,
            "LIVE",
            badge_left,
            live_width,
            rgb(live_color.red, live_color.green, live_color.blue),
            dpi,
        );
    }
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

fn paint_persistent_badge(
    device: windows::Win32::Graphics::Gdi::HDC,
    label: &str,
    left: i32,
    width: i32,
    color: COLORREF,
    dpi: u32,
) {
    let dot_size = scale(7, dpi);
    let dot_left = left + scale(3, dpi);
    let dot_top = scale(18, dpi);
    let dot =
        unsafe { CreateEllipticRgn(dot_left, dot_top, dot_left + dot_size, dot_top + dot_size) };
    if !dot.0.is_null() {
        let brush = unsafe { CreateSolidBrush(color) };
        let _ = unsafe { FillRgn(device, dot, brush) };
        let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
        let _ = unsafe { DeleteObject(HGDIOBJ(dot.0)) };
    }
    draw_text(
        device,
        label,
        RECT {
            left: dot_left + scale(10, dpi),
            top: scale(11, dpi),
            right: left + width - scale(3, dpi),
            bottom: scale(33, dpi),
        },
        scale(10, dpi),
        FW_SEMIBOLD.0 as i32,
        color,
        false,
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
