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
        let desktop = platform_windows::diagnostics::desktop_state();
        let input_desktop = desktop
            .input_desktop_error
            .as_deref()
            .map_or_else(|| Ok(desktop.input_desktop_name.as_deref()), Err);
        let input_surface = windows_input_surface();
        windows_diagnostic(
            windows_session_state(),
            input_desktop,
            input_surface.as_ref().map(|_| ()).map_err(String::as_str),
            desktop.has_foreground_window(),
        )
    }
    #[cfg(not(windows))]
    platform_managed_diagnostic()
}

#[cfg(any(not(windows), test))]
pub(crate) fn platform_managed_diagnostic() -> Value {
    json!({
        "success": true,
        "code": "interactive_desktop_platform_managed",
        "message": "Interactive desktop readiness is reported by the platform CUA runtime",
        "observation_ready": true,
        "input_ready": true,
        "input_surface_ready": true,
    })
}

pub(crate) fn require_desktop_observation_available() -> ComputerUseResult<()> {
    let report = diagnostic();
    require_desktop_observation_from(&report)
}

pub(crate) fn require_exact_window_observation_available() -> ComputerUseResult<()> {
    let report = diagnostic();
    require_exact_window_observation_from(&report)
}

pub(crate) fn require_desktop_observation_from(report: &Value) -> ComputerUseResult<()> {
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

pub(crate) fn require_exact_window_observation_from(report: &Value) -> ComputerUseResult<()> {
    if report["observation_ready"] == true {
        return Ok(());
    }
    Err(ComputerUseError::new(
        ComputerUseErrorCode::InteractiveDesktopUnavailable,
        report["message"]
            .as_str()
            .unwrap_or("exact-window observation is unavailable"),
    ))
}

pub(crate) fn require_input_available() -> ComputerUseResult<()> {
    let report = diagnostic();
    require_input_available_from(&report)
}

pub(crate) fn require_input_available_from(report: &Value) -> ComputerUseResult<()> {
    if report["success"] == true && report["input_ready"] == true {
        return Ok(());
    }
    Err(ComputerUseError::new(
        ComputerUseErrorCode::InteractiveDesktopUnavailable,
        report["input_message"]
            .as_str()
            .or_else(|| report["message"].as_str())
            .unwrap_or("Windows input surface is unavailable"),
    ))
}

#[cfg(windows)]
fn windows_input_surface() -> Result<(), String> {
    use windows_sys::Win32::{Foundation::POINT, UI::WindowsAndMessaging::GetCursorPos};

    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `point` is valid for one synchronous Win32 output write.
    if unsafe { GetCursorPos(&raw mut point) } == 0 {
        return Err(format!(
            "GetCursorPos failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
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
pub(crate) fn windows_diagnostic(
    state: Result<i32, String>,
    input_desktop: Result<Option<&str>, &str>,
    input_surface: Result<(), &str>,
    foreground: bool,
) -> Value {
    let observation_ready = matches!(&state, Ok(state) if *state == WINDOWS_SESSION_ACTIVE)
        && match &input_desktop {
            Ok(Some(name)) => name.eq_ignore_ascii_case("Default"),
            Err(_) => true,
            Ok(None) => false,
        };
    let mut report = match (state, input_desktop) {
        (Err(message), input_desktop) => json!({
            "success": false,
            "code": "interactive_session_unknown",
            "message": format!("Windows interactive session state could not be read: {message}"),
            "input_desktop": input_desktop.ok().flatten(),
            "foreground_present": foreground,
        }),
        (Ok(state), input_desktop) if state != WINDOWS_SESSION_ACTIVE => json!({
            "success": false,
            "code": "interactive_session_not_active",
            "message": "Windows session is not actively connected; reconnect it before mouse or keyboard input",
            "session_state": if state == WINDOWS_SESSION_DISCONNECTED { "disconnected" } else { "not_active" },
            "session_state_code": state,
            "input_desktop": input_desktop.ok().flatten(),
            "foreground_present": foreground,
        }),
        (Ok(_), Err(message)) => json!({
            "success": false,
            "code": "interactive_desktop_unknown",
            "message": format!("Windows input desktop could not be read: {message}"),
            "session_state": "active",
            "session_state_code": WINDOWS_SESSION_ACTIVE,
            "input_desktop": Value::Null,
            "input_desktop_error": message,
            "foreground_present": foreground,
        }),
        (Ok(_), Ok(None)) => json!({
            "success": false,
            "code": "interactive_desktop_unknown",
            "message": "Windows input desktop identity is unavailable",
            "session_state": "active",
            "session_state_code": WINDOWS_SESSION_ACTIVE,
            "input_desktop": Value::Null,
            "foreground_present": foreground,
        }),
        (Ok(_), Ok(Some(name))) if !name.eq_ignore_ascii_case("Default") => json!({
            "success": false,
            "code": "interactive_desktop_unavailable",
            "message": "Windows input desktop is not the interactive Default desktop; return to Default before mouse or keyboard input",
            "session_state": "active",
            "session_state_code": WINDOWS_SESSION_ACTIVE,
            "input_desktop": name,
            "foreground_present": foreground,
        }),
        (Ok(_), Ok(Some(name))) => json!({
            "success": true,
            "code": "interactive_desktop_ready",
            "message": if foreground {
                "Windows session is actively connected and has a foreground window"
            } else {
                "Windows session is actively connected on the interactive Default desktop but has no foreground window; exact activation is required before window input"
            },
            "session_state": "active",
            "session_state_code": WINDOWS_SESSION_ACTIVE,
            "input_desktop": name,
            "foreground_present": foreground,
        }),
    };

    let input_ready = report["success"] == true && input_surface.is_ok();
    report["observation_ready"] = Value::Bool(observation_ready);
    report["input_ready"] = Value::Bool(input_ready);
    report["input_surface_ready"] = Value::Bool(input_surface.is_ok());
    report["input_surface_error"] = input_surface.err().map_or(Value::Null, Value::from);
    if report["success"] == true && !input_ready {
        let message = format!(
            "Windows input surface is unavailable: {}",
            input_surface.expect_err("unready input has a probe error")
        );
        report["input_code"] = Value::from("interactive_input_surface_unavailable");
        report["input_message"] = Value::from(message);
        report["input_attempted"] = Value::Bool(false);
        report["retry_safe"] = Value::Bool(true);
    }
    report
}
