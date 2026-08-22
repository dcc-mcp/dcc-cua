use std::cmp::min;
use std::thread::sleep;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_MOVE_NOCOALESCE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, MapVirtualKeyW, SendInput,
    VK_LBUTTON,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SetCursorPos,
};

use crate::{
    UiaTarget, WindowsPointerButton, WindowsRawInputSnapshot, snapshot_raw_pointer_input_after_down,
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

pub fn send_click_synthesized_active_mods(
    window_id: u64,
    point: (i32, i32),
    count: usize,
    button: &str,
    modifiers: &[&str],
) -> Result<(), String> {
    platform_windows::input::mouse::send_click_synthesized_active_mods(
        window_id, point.0, point.1, count, button, modifiers,
    )
    .map_err(|error| error.to_string())
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
