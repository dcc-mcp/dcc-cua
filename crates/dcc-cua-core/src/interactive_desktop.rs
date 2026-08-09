use serde_json::{Value, json};

use crate::{ComputerUseError, ComputerUseErrorCode, ComputerUseResult};

#[cfg(windows)]
const WINDOWS_SESSION_ACTIVE: i32 = windows_sys::Win32::System::RemoteDesktop::WTSActive;
#[cfg(windows)]
const WINDOWS_SESSION_DISCONNECTED: i32 =
    windows_sys::Win32::System::RemoteDesktop::WTSDisconnected;
#[cfg(all(test, not(windows)))]
const WINDOWS_SESSION_ACTIVE: i32 = 0;
#[cfg(all(test, not(windows)))]
const WINDOWS_SESSION_DISCONNECTED: i32 = 4;

pub(crate) fn diagnostic() -> Value {
    #[cfg(windows)]
    {
        // SAFETY: GetForegroundWindow has no pointer or lifetime preconditions.
        windows_diagnostic(windows_session_state(), unsafe {
            !windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow().is_null()
        })
    }
    #[cfg(not(windows))]
    json!({
        "success": true,
        "code": "interactive_desktop_platform_managed",
        "message": "Interactive desktop readiness is reported by the platform CUA runtime"
    })
}

pub(crate) fn require_available() -> ComputerUseResult<()> {
    let report = diagnostic();
    if report["success"] == true {
        return Ok(());
    }
    Err(ComputerUseError::new(
        ComputerUseErrorCode::InteractiveDesktopUnavailable,
        report["message"]
            .as_str()
            .unwrap_or("interactive desktop is unavailable"),
    ))
}

#[cfg(windows)]
fn windows_session_state() -> Result<i32, String> {
    use std::{mem, ptr};
    use windows_sys::Win32::System::RemoteDesktop::{
        WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTSConnectState, WTSFreeMemory,
        WTSQuerySessionInformationW,
    };

    let mut buffer = ptr::null_mut();
    let mut bytes = 0;
    // SAFETY: both output pointers are valid for writes for the duration of the call.
    let succeeded = unsafe {
        WTSQuerySessionInformationW(
            WTS_CURRENT_SERVER_HANDLE,
            WTS_CURRENT_SESSION,
            WTSConnectState,
            &mut buffer,
            &mut bytes,
        )
    } != 0;
    if !succeeded {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let result = if buffer.is_null() || bytes < mem::size_of::<i32>() as u32 {
        Err("WTS returned an invalid connection-state buffer".into())
    } else {
        // SAFETY: the successful query returned at least one state-sized value.
        Ok(unsafe { ptr::read_unaligned(buffer.cast::<i32>()) })
    };
    if !buffer.is_null() {
        // SAFETY: WTS owns this successful query buffer and requires WTSFreeMemory.
        unsafe { WTSFreeMemory(buffer.cast()) };
    }
    result
}

#[cfg(any(windows, test))]
pub(crate) fn windows_diagnostic(state: Result<i32, String>, foreground: bool) -> Value {
    match state {
        Err(message) => json!({
            "success": false,
            "code": "interactive_session_unknown",
            "message": format!("Windows interactive session state could not be read: {message}"),
        }),
        Ok(state) if state != WINDOWS_SESSION_ACTIVE => json!({
            "success": false,
            "code": "interactive_session_not_active",
            "message": "Windows session is not actively connected; reconnect it before mouse or keyboard input",
            "session_state": if state == WINDOWS_SESSION_DISCONNECTED { "disconnected" } else { "not_active" },
            "session_state_code": state,
        }),
        Ok(_) if !foreground => json!({
            "success": false,
            "code": "interactive_desktop_unavailable",
            "message": "Windows interactive desktop is locked or has no foreground window",
            "session_state": "active",
            "session_state_code": WINDOWS_SESSION_ACTIVE,
        }),
        Ok(_) => json!({
            "success": true,
            "code": "interactive_desktop_ready",
            "message": "Windows session is actively connected and has a foreground window",
            "session_state": "active",
            "session_state_code": WINDOWS_SESSION_ACTIVE,
        }),
    }
}
