use super::*;

pub(super) fn send_windows_key_holds(
    window_id: u64,
    keys: &[String],
    duration_ms: u64,
) -> ComputerUseResult<()> {
    use std::thread::sleep;
    use std::time::Duration;
    use windows_sys::Win32::UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
            KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput,
        },
        WindowsAndMessaging::GetForegroundWindow,
    };

    let target = window_id as usize as *mut core::ffi::c_void;
    if unsafe { GetForegroundWindow() } != target {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InputFailed,
            format!(
                "send Windows held key refused: exact target HWND 0x{window_id:x} is not foreground"
            ),
        ));
    }

    let virtual_keys = keys
        .iter()
        .map(|key| windows_key_virtual_code(key))
        .collect::<ComputerUseResult<Vec<_>>>()?;
    let make_input = |virtual_key: u16, key_up: bool| {
        let scan_code = unsafe { MapVirtualKeyW(virtual_key as u32, MAPVK_VK_TO_VSC) } as u16;
        let mut flags = if scan_code == 0 {
            0
        } else {
            KEYEVENTF_SCANCODE
        };
        if windows_key_is_extended(virtual_key) {
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
    };
    let key_downs: Vec<_> = virtual_keys
        .iter()
        .map(|key| make_input(*key, false))
        .collect();
    let input_size = std::mem::size_of::<INPUT>() as i32;
    let inserted = unsafe { SendInput(key_downs.len() as u32, key_downs.as_ptr(), input_size) };
    if inserted != key_downs.len() as u32 {
        for key in virtual_keys.iter().take(inserted as usize) {
            let key_up = make_input(*key, true);
            let _ = unsafe { SendInput(1, &key_up, input_size) };
        }
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InputFailed,
            format!(
                "send Windows held key-down inserted {inserted}/{} events: {}",
                key_downs.len(),
                std::io::Error::last_os_error()
            ),
        ));
    }

    sleep(Duration::from_millis(duration_ms));
    for key in virtual_keys.iter().rev() {
        let key_up = make_input(*key, true);
        let released = unsafe { SendInput(1, &key_up, input_size) };
        if released != 1 {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InputFailed,
                format!(
                    "send Windows held key-up inserted {released}/1 events: {}",
                    std::io::Error::last_os_error()
                ),
            ));
        }
    }
    Ok(())
}

fn windows_key_virtual_code(key: &str) -> ComputerUseResult<u16> {
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
        _ if normalized.len() == 1 => {
            let ch = normalized.as_bytes()[0];
            if ch.is_ascii_alphanumeric() {
                if ch.is_ascii_lowercase() {
                    u16::from(ch - b'a' + b'A')
                } else {
                    u16::from(ch)
                }
            } else {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::InvalidAction,
                    format!("unsupported held key: {key}"),
                ));
            }
        }
        _ => {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!("unsupported held key: {key}"),
            ));
        }
    };
    Ok(value)
}

fn windows_key_is_extended(key: u16) -> bool {
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
