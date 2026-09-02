use std::cmp::min;
use std::thread::sleep;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_MOVE_NOCOALESCE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, MapVirtualKeyW, SendInput,
    VK_LBUTTON,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId, IsWindow,
    PostMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SetCursorPos, WM_MOUSEHWHEEL, WM_MOUSEWHEEL,
};

use crate::{
    UiaError, UiaTarget, WindowsForegroundRelation, WindowsPointerButton, WindowsRawInputSnapshot,
    WindowsWindowIdentity, snapshot_raw_pointer_input_after_down,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WindowsInputCount {
    requested: u32,
    inserted: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl WindowsInputCount {
    #[must_use]
    pub const fn accepted() -> Self {
        Self::accepted_count(1)
    }

    const fn accepted_count(requested: u32) -> Self {
        Self {
            requested,
            inserted: requested,
            error: None,
        }
    }

    #[must_use]
    pub fn incomplete(inserted: u32, error: impl Into<String>) -> Self {
        Self::incomplete_count(1, inserted, error)
    }

    fn incomplete_count(requested: u32, inserted: u32, error: impl Into<String>) -> Self {
        Self {
            requested,
            inserted,
            error: Some(error.into()),
        }
    }

    #[must_use]
    pub const fn requested(&self) -> u32 {
        self.requested
    }

    #[must_use]
    pub const fn inserted(&self) -> u32 {
        self.inserted
    }

    #[must_use]
    pub const fn was_accepted(&self) -> bool {
        self.inserted == self.requested && self.error.is_none()
    }

    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

pub type RelativeMoveInjection = WindowsInputCount;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WindowsPostButtonUpSnapshot {
    pub async_button_down: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_fence: Option<WindowsRawInputSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_fence_error: Option<String>,
}

impl WindowsPostButtonUpSnapshot {
    #[must_use]
    pub fn new(
        async_button_down: bool,
        target_fence: Option<WindowsRawInputSnapshot>,
        target_fence_error: Option<String>,
    ) -> Self {
        Self {
            async_button_down,
            target_fence,
            target_fence_error,
        }
    }
}

fn virtual_desktop() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
        )
    }
}

pub(crate) fn combined_source_move_and_left_down_inputs(
    source: (i32, i32),
    desktop: (i32, i32, i32, i32),
) -> [INPUT; 2] {
    let (normalized_x, normalized_y) = platform_windows::virtualdesk::to_virtualdesk_absolute(
        source.0, source.1, desktop.0, desktop.1, desktop.2, desktop.3,
    );
    [
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: normalized_x,
                    dy: normalized_y,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE
                        | MOUSEEVENTF_MOVE_NOCOALESCE
                        | MOUSEEVENTF_ABSOLUTE
                        | MOUSEEVENTF_VIRTUALDESK,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_LEFTDOWN,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
    ]
}

fn submit(inputs: &[INPUT]) -> WindowsInputCount {
    let requested = u32::try_from(inputs.len()).unwrap_or(u32::MAX);
    let inserted = unsafe {
        SendInput(
            requested,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if inserted == requested {
        WindowsInputCount::accepted_count(requested)
    } else {
        WindowsInputCount::incomplete_count(
            requested,
            inserted,
            std::io::Error::last_os_error().to_string(),
        )
    }
}

#[must_use]
pub fn inject_combined_source_move_and_left_down(source: (i32, i32)) -> WindowsInputCount {
    submit(&combined_source_move_and_left_down_inputs(
        source,
        virtual_desktop(),
    ))
}

#[must_use]
pub fn inject_absolute_mouse_move(x: i32, y: i32) -> WindowsInputCount {
    submit(&combined_source_move_and_left_down_inputs((x, y), virtual_desktop())[..1])
}

#[must_use]
pub fn inject_consumable_mouse_move(x: i32, y: i32) -> WindowsInputCount {
    if unsafe { SetCursorPos(x, y) } == 0 {
        return WindowsInputCount::incomplete(
            0,
            format!(
                "SetCursorPos({x}, {y}) failed: {}",
                std::io::Error::last_os_error()
            ),
        );
    }
    inject_absolute_mouse_move(x, y)
}

#[must_use]
pub fn inject_relative_mouse_move(dx: i32, dy: i32) -> RelativeMoveInjection {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_MOVE_NOCOALESCE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    submit(std::slice::from_ref(&input))
}

#[must_use]
pub fn inject_mouse_button(button: WindowsPointerButton, pressed: bool) -> WindowsInputCount {
    let flags = match (button, pressed) {
        (WindowsPointerButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
        (WindowsPointerButton::Left, false) => MOUSEEVENTF_LEFTUP,
        (WindowsPointerButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
        (WindowsPointerButton::Right, false) => MOUSEEVENTF_RIGHTUP,
        (WindowsPointerButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
        (WindowsPointerButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    submit(std::slice::from_ref(&input))
}

pub fn cursor_position() -> Result<(i32, i32), String> {
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&raw mut point) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok((point.x, point.y))
}

#[must_use]
pub fn snapshot_left_button_after_up(target: UiaTarget) -> WindowsPostButtonUpSnapshot {
    let async_button_down = unsafe { GetAsyncKeyState(i32::from(VK_LBUTTON)) } as u16 & 0x8000 != 0;
    match snapshot_raw_pointer_input_after_down(target, WindowsPointerButton::Left) {
        Ok(target_fence) => {
            WindowsPostButtonUpSnapshot::new(async_button_down, Some(target_fence), None)
        }
        Err(error) => WindowsPostButtonUpSnapshot::new(
            async_button_down,
            None,
            Some(format!("sample exact target fence after LEFTUP: {error}")),
        ),
    }
}

pub fn send_key_synthesized(window_id: u64, key: &str, modifiers: &[&str]) -> Result<(), String> {
    platform_windows::input::keyboard::send_key_synthesized(window_id, key, modifiers)
        .map_err(|error| error.to_string())
}

pub fn send_text_synthesized(window_id: u64, text: &str) -> Result<(), String> {
    platform_windows::input::keyboard::send_text_synthesized(window_id, text)
        .map_err(|error| error.to_string())
}

pub fn post_click_screen(
    window_id: u64,
    x: i32,
    y: i32,
    count: usize,
    button: &str,
) -> Result<(), String> {
    platform_windows::input::mouse::post_click_screen(window_id, x, y, count, button)
        .map_err(|error| error.to_string())
}

/// Post Unicode text through the target window's message queue. This is an
/// explicit provider-free route for custom-rendered windows whose Unreal/CEF
/// bridge consumes WM_CHAR but ignores SendInput Unicode packets.
pub fn post_text(window_id: u64, text: &str) -> Result<(), String> {
    platform_windows::input::keyboard::post_type_text(window_id, text)
        .map_err(|error| error.to_string())
}

/// Post bounded mouse-wheel messages at a screen point inside an exact window.
/// This is an explicit provider-free route for custom-rendered Unreal/CEF
/// surfaces that consume wheel messages but expose no UIA scroll pattern.
pub fn post_scroll_screen(
    window_id: u64,
    x: i32,
    y: i32,
    horizontal: i32,
    vertical: i32,
) -> Result<(), String> {
    if horizontal != 0 && vertical != 0 {
        return Err("Windows PostMessage scroll supports one axis per action".into());
    }
    if horizontal.unsigned_abs() > 50 || vertical.unsigned_abs() > 50 {
        return Err("Windows PostMessage scroll amount must be at most 50".into());
    }
    if horizontal == 0 && vertical == 0 {
        return Err("Windows PostMessage scroll requires a non-zero axis".into());
    }
    if let Some(msg) = post_message_blocked_by_uipi(window_id) {
        return Err(msg);
    }
    let hwnd = window_id as isize as windows_sys::Win32::Foundation::HWND;
    if hwnd.is_null() || unsafe { IsWindow(hwnd) == 0 } {
        return Err("invalid target hwnd".into());
    }
    let mut point = POINT { x, y };
    if unsafe { ScreenToClient(hwnd, &mut point) == 0 } {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let lparam = (((point.y as u32 as usize) << 16) | (point.x as u32 as usize & 0xffff)) as isize;
    let (message, delta) = if horizontal != 0 {
        (WM_MOUSEHWHEEL, horizontal.signum() * 120)
    } else {
        (WM_MOUSEWHEEL, vertical.signum() * 120)
    };
    let count = horizontal.unsigned_abs().max(vertical.unsigned_abs());
    for _ in 0..count {
        let wparam = ((delta as i16 as u16 as usize) << 16) as usize;
        if unsafe { PostMessageW(hwnd, message, wparam, lparam) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        sleep(Duration::from_millis(4));
    }
    Ok(())
}

pub fn inject_drag_screen(
    window_id: u64,
    from: (i32, i32),
    to: (i32, i32),
    steps: usize,
    button: &str,
) -> Result<(), String> {
    platform_windows::input::inject::inject_drag_screen(
        window_id, from.0, from.1, to.0, to.1, steps, button,
    )
    .map_err(|error| error.to_string())
}

#[must_use]
pub fn post_message_blocked_by_uipi(window_id: u64) -> Option<String> {
    platform_windows::input::post_message_blocked_by_uipi(window_id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForegroundDispatch<T, N, A> {
    Completed(T),
    NotAttempted(N),
    Attempted(A),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoundedForegroundDispatchError<E, N, A> {
    Activation { attempts: usize, error: E },
    NotAttempted { attempts: usize, error: N },
    Attempted { attempts: usize, error: A },
}

/// Run at most two activation/preflight attempts and dispatch input once.
///
/// A typed `NotAttempted` result may reacquire foreground because the physical
/// dispatcher proved that no input was submitted. `Attempted` is terminal even
/// when its inserted-event count is zero: the operating system call was entered
/// and repeating it could duplicate an action whose completion is not known.
pub(crate) fn run_bounded_foreground_dispatch<T, E, N, A>(
    mut activate: impl FnMut() -> Result<(), E>,
    mut dispatch: impl FnMut() -> ForegroundDispatch<T, N, A>,
    retry_activation: impl Fn(&E) -> bool,
    retry_not_attempted: impl Fn(&N) -> bool,
) -> Result<T, BoundedForegroundDispatchError<E, N, A>> {
    const MAX_ATTEMPTS: usize = 2;
    for attempts in 1..=MAX_ATTEMPTS {
        if let Err(error) = activate() {
            if attempts < MAX_ATTEMPTS && retry_activation(&error) {
                continue;
            }
            return Err(BoundedForegroundDispatchError::Activation { attempts, error });
        }
        match dispatch() {
            ForegroundDispatch::Completed(value) => return Ok(value),
            ForegroundDispatch::NotAttempted(error)
                if attempts < MAX_ATTEMPTS && retry_not_attempted(&error) => {}
            ForegroundDispatch::NotAttempted(error) => {
                return Err(BoundedForegroundDispatchError::NotAttempted { attempts, error });
            }
            ForegroundDispatch::Attempted(error) => {
                return Err(BoundedForegroundDispatchError::Attempted { attempts, error });
            }
        }
    }
    unreachable!("the bounded foreground loop always returns on its final attempt")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsForegroundClickPreflightReason {
    InvalidTarget,
    TargetNotForeground,
    UipiBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WindowsForegroundClickPreflightFailure {
    pub reason: WindowsForegroundClickPreflightReason,
    pub target: WindowsWindowIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreground: Option<WindowsWindowIdentity>,
    pub detail: String,
}

impl std::fmt::Display for WindowsForegroundClickPreflightFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.detail)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WindowsForegroundClickOutcome {
    pub activation_attempts: usize,
    pub target: WindowsWindowIdentity,
    pub foreground_before: WindowsWindowIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreground_after: Option<WindowsWindowIdentity>,
    pub foreground_after_relation: WindowsForegroundRelation,
    pub requested_clicks: usize,
    pub completed_clicks: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modifier_down: Option<WindowsInputCount>,
    pub click_batches: Vec<WindowsInputCount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modifier_up: Option<WindowsInputCount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modifier_up_retry: Option<WindowsInputCount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emergency_button_up: Option<WindowsInputCount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl WindowsForegroundClickOutcome {
    #[must_use]
    pub fn accepted(&self) -> bool {
        self.error.is_none()
            && self.completed_clicks == self.requested_clicks
            && self
                .modifier_down
                .as_ref()
                .is_none_or(WindowsInputCount::was_accepted)
            && self
                .modifier_up_retry
                .as_ref()
                .or(self.modifier_up.as_ref())
                .is_none_or(WindowsInputCount::was_accepted)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WindowsForegroundClickError {
    #[error(
        "foreground activation failed after {attempts} safe attempt(s); current foreground: {foreground:?}: {source}"
    )]
    Activation {
        attempts: usize,
        foreground: Option<WindowsWindowIdentity>,
        #[source]
        source: UiaError,
    },
    #[error("foreground click was not attempted after {attempts} safe attempt(s): {failure}")]
    NotAttempted {
        attempts: usize,
        failure: WindowsForegroundClickPreflightFailure,
    },
    #[error(
        "foreground click input was attempted and will not be retried after {attempts} activation attempt(s): {detail}"
    )]
    Attempted {
        attempts: usize,
        detail: String,
        outcome: Box<WindowsForegroundClickOutcome>,
    },
}

/// Deliver an exact-foreground click with a typed physical outcome.
///
/// The pinned upstream click helper currently collapses both pre-injection
/// foreground refusal and post-injection foreground change into one string
/// error. This host-owned boundary performs the same `SendInput` sequence while
/// retaining inserted-event counts. It may reacquire foreground once only when
/// no input was sent, and it never repeats a click after dispatch begins.
pub fn send_click_exact_foreground_mods(
    target: UiaTarget,
    point: (i32, i32),
    count: usize,
    button: &str,
    modifiers: &[&str],
    mut activate: impl FnMut() -> Result<(), UiaError>,
) -> Result<WindowsForegroundClickOutcome, WindowsForegroundClickError> {
    let mut activation_attempts = 0;
    let result = run_bounded_foreground_dispatch(
        || {
            activation_attempts += 1;
            activate()
        },
        || dispatch_click_exact_foreground(target, point, count, button, modifiers),
        |error| matches!(error, UiaError::ForegroundActivationRefused { .. }),
        |failure| failure.reason == WindowsForegroundClickPreflightReason::TargetNotForeground,
    );
    match result {
        Ok(mut outcome) => {
            outcome.activation_attempts = activation_attempts;
            Ok(outcome)
        }
        Err(BoundedForegroundDispatchError::Activation { attempts, error }) => {
            Err(WindowsForegroundClickError::Activation {
                attempts,
                foreground: foreground_identity(),
                source: error,
            })
        }
        Err(BoundedForegroundDispatchError::NotAttempted { attempts, error }) => {
            Err(WindowsForegroundClickError::NotAttempted {
                attempts,
                failure: error,
            })
        }
        Err(BoundedForegroundDispatchError::Attempted {
            attempts,
            error: mut outcome,
        }) => {
            outcome.activation_attempts = attempts;
            let detail = outcome
                .error
                .clone()
                .unwrap_or_else(|| "Windows returned an incomplete click outcome".into());
            Err(WindowsForegroundClickError::Attempted {
                attempts,
                detail,
                outcome: Box::new(outcome),
            })
        }
    }
}

fn dispatch_click_exact_foreground(
    target: UiaTarget,
    point: (i32, i32),
    count: usize,
    button: &str,
    modifiers: &[&str],
) -> ForegroundDispatch<
    WindowsForegroundClickOutcome,
    WindowsForegroundClickPreflightFailure,
    WindowsForegroundClickOutcome,
> {
    let target_identity = WindowsWindowIdentity {
        window_handle: target.window_handle,
        process_id: target.process_id,
    };
    let foreground = foreground_identity();
    if !exact_target_identity_is_current(target) {
        return ForegroundDispatch::NotAttempted(WindowsForegroundClickPreflightFailure {
            reason: WindowsForegroundClickPreflightReason::InvalidTarget,
            target: target_identity,
            foreground,
            detail: format!(
                "exact target HWND 0x{:x} no longer exists or belongs to PID {} (current foreground: {})",
                target.window_handle,
                target.process_id,
                foreground_diagnostic(foreground),
            ),
        });
    }
    let Some(foreground_before) = foreground else {
        return ForegroundDispatch::NotAttempted(WindowsForegroundClickPreflightFailure {
            reason: WindowsForegroundClickPreflightReason::TargetNotForeground,
            target: target_identity,
            foreground: None,
            detail: format!(
                "exact target HWND 0x{:x} PID {} was not foreground; current foreground: none; no input was submitted",
                target.window_handle, target.process_id,
            ),
        });
    };
    if foreground_before.window_handle != target.window_handle {
        return ForegroundDispatch::NotAttempted(WindowsForegroundClickPreflightFailure {
            reason: WindowsForegroundClickPreflightReason::TargetNotForeground,
            target: target_identity,
            foreground: Some(foreground_before),
            detail: format!(
                "exact target HWND 0x{:x} PID {} was not foreground; current foreground: {}; no input was submitted",
                target.window_handle,
                target.process_id,
                foreground_diagnostic(Some(foreground_before)),
            ),
        });
    }
    if let Some(detail) = post_message_blocked_by_uipi(target.window_handle) {
        return ForegroundDispatch::NotAttempted(WindowsForegroundClickPreflightFailure {
            reason: WindowsForegroundClickPreflightReason::UipiBlocked,
            target: target_identity,
            foreground: Some(foreground_before),
            detail: format!("{detail}; no input was submitted"),
        });
    }

    let requested_clicks = count.max(1);
    let modifier_keys = modifiers
        .iter()
        .filter_map(|modifier| modifier_virtual_key(modifier))
        .collect::<Vec<_>>();
    let modifier_down_inputs = modifier_keys
        .iter()
        .map(|key| keyboard_input(*key, false))
        .collect::<Vec<_>>();
    let modifier_up_inputs = modifier_keys
        .iter()
        .rev()
        .map(|key| keyboard_input(*key, true))
        .collect::<Vec<_>>();
    let mut outcome = WindowsForegroundClickOutcome {
        activation_attempts: 0,
        target: target_identity,
        foreground_before,
        foreground_after: None,
        foreground_after_relation: WindowsForegroundRelation::NoForeground,
        requested_clicks,
        completed_clicks: 0,
        modifier_down: None,
        click_batches: Vec::with_capacity(requested_clicks),
        modifier_up: None,
        modifier_up_retry: None,
        emergency_button_up: None,
        error: None,
    };

    let _ = unsafe { SetCursorPos(point.0, point.1) };
    if !modifier_down_inputs.is_empty() {
        let down = submit(&modifier_down_inputs);
        let accepted_modifier_count = down.inserted() as usize;
        let down_accepted = down.was_accepted();
        outcome.modifier_down = Some(down);
        if !down_accepted {
            let cleanup = modifier_keys
                .iter()
                .take(accepted_modifier_count)
                .rev()
                .map(|key| keyboard_input(*key, true))
                .collect::<Vec<_>>();
            if !cleanup.is_empty() {
                outcome.modifier_up = Some(submit(&cleanup));
            }
            outcome.error = Some("SendInput did not accept every modifier-down event".into());
            finish_click_outcome_foreground(&mut outcome);
            return ForegroundDispatch::Attempted(outcome);
        }
        sleep(Duration::from_millis(5));
    }

    let click_inputs = click_input_batch(point, button);
    for click_index in 0..requested_clicks {
        let batch = submit(&click_inputs);
        let inserted = batch.inserted();
        let accepted = batch.was_accepted();
        outcome.click_batches.push(batch);
        if !accepted {
            if inserted == 2 {
                outcome.emergency_button_up =
                    Some(inject_mouse_button(pointer_button(button), false));
            }
            outcome.error = Some(format!(
                "SendInput did not accept the complete mouse batch for click {} of {}",
                click_index + 1,
                requested_clicks,
            ));
            break;
        }
        outcome.completed_clicks += 1;
        if click_index + 1 < requested_clicks {
            sleep(Duration::from_millis(80));
        }
    }

    if !modifier_up_inputs.is_empty() {
        let up = submit(&modifier_up_inputs);
        let up_accepted = up.was_accepted();
        outcome.modifier_up = Some(up);
        if !up_accepted {
            let retry = submit(&modifier_up_inputs);
            let retry_accepted = retry.was_accepted();
            outcome.modifier_up_retry = Some(retry);
            if !retry_accepted {
                outcome.error.get_or_insert_with(|| {
                    "SendInput did not accept every modifier-up cleanup event".into()
                });
            }
        }
    }
    sleep(Duration::from_millis(120));
    finish_click_outcome_foreground(&mut outcome);
    if outcome.accepted() {
        ForegroundDispatch::Completed(outcome)
    } else {
        ForegroundDispatch::Attempted(outcome)
    }
}

fn click_input_batch(point: (i32, i32), button: &str) -> [INPUT; 3] {
    let desktop = virtual_desktop();
    let (normalized_x, normalized_y) = platform_windows::virtualdesk::to_virtualdesk_absolute(
        point.0, point.1, desktop.0, desktop.1, desktop.2, desktop.3,
    );
    let (down_flag, up_flag) = match pointer_button(button) {
        WindowsPointerButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        WindowsPointerButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        WindowsPointerButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    };
    [
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: normalized_x,
                    dy: normalized_y,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: down_flag,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: up_flag,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
    ]
}

fn pointer_button(button: &str) -> WindowsPointerButton {
    match button {
        "right" => WindowsPointerButton::Right,
        "middle" => WindowsPointerButton::Middle,
        _ => WindowsPointerButton::Left,
    }
}

fn modifier_virtual_key(modifier: &str) -> Option<u16> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_LWIN, VK_MENU, VK_SHIFT};
    match modifier.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(VK_CONTROL),
        "shift" => Some(VK_SHIFT),
        "alt" | "menu" | "option" => Some(VK_MENU),
        "win" | "meta" | "windows" | "cmd" | "command" => Some(VK_LWIN),
        _ => None,
    }
}

fn exact_target_identity_is_current(target: UiaTarget) -> bool {
    let hwnd = target.window_handle as usize as *mut core::ffi::c_void;
    if hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
        return false;
    }
    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) };
    process_id == target.process_id
}

fn foreground_identity() -> Option<WindowsWindowIdentity> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) };
    (process_id != 0).then_some(WindowsWindowIdentity {
        window_handle: hwnd as usize as u64,
        process_id,
    })
}

fn foreground_relation(
    target: WindowsWindowIdentity,
    foreground: Option<WindowsWindowIdentity>,
) -> WindowsForegroundRelation {
    match foreground {
        None => WindowsForegroundRelation::NoForeground,
        Some(actual) if actual.window_handle == target.window_handle => {
            WindowsForegroundRelation::ExactTarget
        }
        Some(actual) if actual.process_id == target.process_id => {
            WindowsForegroundRelation::SameProcess
        }
        Some(_) => WindowsForegroundRelation::ForeignProcess,
    }
}

fn finish_click_outcome_foreground(outcome: &mut WindowsForegroundClickOutcome) {
    outcome.foreground_after = foreground_identity();
    outcome.foreground_after_relation =
        foreground_relation(outcome.target, outcome.foreground_after);
}

fn foreground_diagnostic(foreground: Option<WindowsWindowIdentity>) -> String {
    foreground.map_or_else(
        || "none".into(),
        |identity| {
            format!(
                "HWND 0x{:x} PID {}",
                identity.window_handle, identity.process_id
            )
        },
    )
}

pub fn move_cursor_desktop(x: i32, y: i32) -> Result<(), String> {
    platform_windows::input::mouse::move_cursor_desktop(x, y).map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowsOverlayCommand {
    PinAbove(u64),
    ClickPulse { x: f64, y: f64 },
    MoveTo { x: f64, y: f64, heading: f64 },
}

pub fn send_overlay_command(session_id: String, command: WindowsOverlayCommand) {
    let command = match command {
        WindowsOverlayCommand::PinAbove(window_id) => {
            cursor_overlay::OverlayCommand::PinAbove(window_id)
        }
        WindowsOverlayCommand::ClickPulse { x, y } => {
            cursor_overlay::OverlayCommand::ClickPulse { x, y }
        }
        WindowsOverlayCommand::MoveTo { x, y, heading } => cursor_overlay::OverlayCommand::MoveTo {
            x,
            y,
            end_heading_radians: heading,
        },
    };
    platform_windows::overlay::send_command(session_id, command);
}

pub async fn animate_cursor_to(session_id: String, x: f64, y: f64) {
    platform_windows::overlay::animate_cursor_to(session_id, x, y).await;
}

#[derive(Debug, thiserror::Error)]
pub enum WindowsHeldKeyError {
    #[error("exact target HWND 0x{window_id:x} is not foreground")]
    TargetNotForeground { window_id: u64 },
    #[error("unsupported held key: {0}")]
    InvalidKey(String),
    #[error("held key input failed: {0}")]
    Injection(String),
    #[error("held keypress interrupted after all keys were released")]
    Interrupted,
}

pub fn send_held_keys_exact_foreground(
    window_id: u64,
    keys: &[String],
    duration_ms: u64,
    mut interrupted: impl FnMut() -> bool,
) -> Result<(), WindowsHeldKeyError> {
    let target = window_id as usize as *mut core::ffi::c_void;
    if unsafe { GetForegroundWindow() } != target {
        return Err(WindowsHeldKeyError::TargetNotForeground { window_id });
    }
    let virtual_keys = keys
        .iter()
        .map(|key| virtual_key(key))
        .collect::<Result<Vec<_>, _>>()?;
    let key_downs = virtual_keys
        .iter()
        .map(|key| keyboard_input(*key, false))
        .collect::<Vec<_>>();
    let down = submit(&key_downs);
    if !down.was_accepted() {
        for key in virtual_keys.iter().take(down.inserted() as usize).rev() {
            let _ = submit(std::slice::from_ref(&keyboard_input(*key, true)));
        }
        return Err(WindowsHeldKeyError::Injection(format!(
            "key-down inserted {}/{} events: {}",
            down.inserted(),
            down.requested(),
            down.error().unwrap_or("unknown Windows error")
        )));
    }

    let was_interrupted = wait_until_interrupted(duration_ms, &mut interrupted);

    let release_failures = release_all_keys(&virtual_keys, |key| {
        submit(std::slice::from_ref(&keyboard_input(key, true)))
    });
    if !release_failures.is_empty() {
        return Err(WindowsHeldKeyError::Injection(format!(
            "key-up cleanup failed for {} key(s): {}",
            release_failures.len(),
            release_failures.join("; ")
        )));
    }
    if was_interrupted {
        return Err(WindowsHeldKeyError::Interrupted);
    }
    Ok(())
}

pub(crate) fn wait_until_interrupted(
    duration_ms: u64,
    mut interrupted: impl FnMut() -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_millis(duration_ms);
    loop {
        if interrupted() {
            return true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        sleep(min(remaining, Duration::from_millis(20)));
    }
}

pub(crate) fn release_all_keys(
    virtual_keys: &[u16],
    mut release: impl FnMut(u16) -> WindowsInputCount,
) -> Vec<String> {
    let mut failures = Vec::new();
    for key in virtual_keys.iter().rev().copied() {
        let released = release(key);
        if !released.was_accepted() {
            failures.push(format!(
                "0x{key:02x}: {}",
                released.error().unwrap_or("unknown Windows error")
            ));
        }
    }
    failures
}

fn keyboard_input(virtual_key: u16, key_up: bool) -> INPUT {
    let scan_code = unsafe { MapVirtualKeyW(u32::from(virtual_key), MAPVK_VK_TO_VSC) } as u16;
    let mut flags = if scan_code == 0 {
        0
    } else {
        KEYEVENTF_SCANCODE
    };
    if key_is_extended(virtual_key) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: if scan_code == 0 { virtual_key } else { 0 },
                wScan: scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

pub(crate) fn virtual_key(key: &str) -> Result<u16, WindowsHeldKeyError> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
    let normalized = key.trim().to_ascii_lowercase();
    let value = match normalized.as_str() {
        "enter" | "return" => VK_RETURN,
        "tab" => VK_TAB,
        "escape" | "esc" => VK_ESCAPE,
        "space" | " " => VK_SPACE,
        "backspace" | "back" => VK_BACK,
        "delete" | "del" => VK_DELETE,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" | "pgup" => VK_PRIOR,
        "pagedown" | "pgdn" => VK_NEXT,
        "f1" => VK_F1,
        "f2" => VK_F2,
        "f3" => VK_F3,
        "f4" => VK_F4,
        "f5" => VK_F5,
        "f6" => VK_F6,
        "f7" => VK_F7,
        "f8" => VK_F8,
        "f9" => VK_F9,
        "f10" => VK_F10,
        "f11" => VK_F11,
        "f12" => VK_F12,
        _ if normalized.len() == 1 && normalized.as_bytes()[0].is_ascii_alphanumeric() => {
            normalized.as_bytes()[0].to_ascii_uppercase().into()
        }
        _ => return Err(WindowsHeldKeyError::InvalidKey(key.to_owned())),
    };
    Ok(value)
}

fn key_is_extended(key: u16) -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
    matches!(
        key,
        VK_DELETE
            | VK_INSERT
            | VK_HOME
            | VK_END
            | VK_PRIOR
            | VK_NEXT
            | VK_UP
            | VK_DOWN
            | VK_LEFT
            | VK_RIGHT
            | VK_RCONTROL
            | VK_RMENU
            | VK_RWIN
            | VK_NUMLOCK
            | VK_SNAPSHOT
    )
}
