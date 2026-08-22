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
        WindowsDesktopProbe::read(true).diagnostic()
    }
    #[cfg(not(windows))]
    platform_managed_diagnostic()
}

#[cfg(not(windows))]
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
    #[cfg(windows)]
    {
        let probe = WindowsDesktopProbe::read(false);
        if probe.desktop_ready() {
            Ok(())
        } else {
            require_desktop_observation_from(&probe.diagnostic())
        }
    }
    #[cfg(not(windows))]
    {
        let report = diagnostic();
        require_desktop_observation_from(&report)
    }
}

pub(crate) fn require_exact_window_observation_available() -> ComputerUseResult<()> {
    #[cfg(windows)]
    {
        let probe = WindowsDesktopProbe::read(false);
        if probe.observation_ready() {
            Ok(())
        } else {
            require_exact_window_observation_from(&probe.diagnostic())
        }
    }
    #[cfg(not(windows))]
    {
        let report = diagnostic();
        require_exact_window_observation_from(&report)
    }
}

pub(crate) fn require_window_activation_available() -> ComputerUseResult<()> {
    #[cfg(windows)]
    {
        let probe = WindowsDesktopProbe::read(false);
        if probe.observation_ready() {
            Ok(())
        } else {
            require_window_activation_from(&probe.diagnostic())
        }
    }
    #[cfg(not(windows))]
    {
        let report = diagnostic();
        require_window_activation_from(&report)
    }
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

pub(crate) fn require_window_activation_from(report: &Value) -> ComputerUseResult<()> {
    require_exact_window_observation_from(report)
}

pub(crate) fn require_input_available() -> ComputerUseResult<()> {
    #[cfg(windows)]
    {
        let probe = WindowsDesktopProbe::read(true);
        if probe.input_ready() {
            Ok(())
        } else {
            require_input_available_from(&probe.diagnostic())
        }
    }
    #[cfg(not(windows))]
    {
        let report = diagnostic();
        require_input_available_from(&report)
    }
}

#[cfg(windows)]
struct WindowsDesktopProbe {
    state: Result<i32, String>,
    input_desktop: Result<Option<String>, String>,
    thread_desktop: Result<Option<String>, String>,
    input_surface: Result<(), String>,
    foreground: bool,
}

#[cfg(windows)]
impl WindowsDesktopProbe {
    fn read(probe_input_surface: bool) -> Self {
        let desktop = dcc_cua_platform_windows::desktop_state();
        Self {
            state: windows_session_state(),
            input_desktop: desktop
                .input_desktop_error
                .map_or_else(|| Ok(desktop.input_desktop_name), Err),
            thread_desktop: thread_desktop_name(),
            input_surface: if probe_input_surface {
                windows_input_surface()
            } else {
                Ok(())
            },
            foreground: desktop.has_foreground_window,
        }
    }

    fn session_active(&self) -> bool {
        matches!(self.state, Ok(WINDOWS_SESSION_ACTIVE))
    }

    fn default_desktop_or_verified_fallback(&self) -> bool {
        match &self.input_desktop {
            Ok(Some(name)) => name.eq_ignore_ascii_case("Default"),
            Err(_) => {
                self.foreground
                    && matches!(
                        &self.thread_desktop,
                        Ok(Some(name)) if name.eq_ignore_ascii_case("Default")
                    )
            }
            Ok(None) => false,
        }
    }

    fn desktop_ready(&self) -> bool {
        self.session_active() && self.default_desktop_or_verified_fallback()
    }

    fn observation_ready(&self) -> bool {
        self.session_active()
            && match &self.input_desktop {
                Ok(Some(name)) => name.eq_ignore_ascii_case("Default"),
                Err(_) => true,
                Ok(None) => false,
            }
    }

    fn input_ready(&self) -> bool {
        self.desktop_ready() && self.input_surface.is_ok()
    }

    fn diagnostic(&self) -> Value {
        windows_diagnostic_with_thread_fallback(
            self.state.clone(),
            self.input_desktop
                .as_ref()
                .map(|name| name.as_deref())
                .map_err(String::as_str),
            self.thread_desktop
                .as_ref()
                .map(|name| name.as_deref())
                .map_err(String::as_str),
            self.input_surface.as_ref().copied().map_err(String::as_str),
            self.foreground,
        )
    }
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
    use windows_sys::Win32::{
        Foundation::POINT,
        UI::WindowsAndMessaging::{CURSORINFO, GetCursorInfo, GetCursorPos},
    };

    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `point` is valid for one synchronous Win32 output write.
    if unsafe { GetCursorPos(&raw mut point) } == 0 {
        let cursor_pos_error = std::io::Error::last_os_error();
        let mut cursor_info = CURSORINFO::default();
        // SAFETY: `cursor_info` is initialized with the ABI-required cbSize and
        // is valid for one synchronous Win32 output write. GetCursorInfo is a
        // read-only input-surface probe; it does not relax the desktop checks.
        if unsafe { GetCursorInfo(&raw mut cursor_info) } != 0 {
            return Ok(());
        }
        return Err(format!(
            "GetCursorPos failed: {cursor_pos_error}; GetCursorInfo failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn thread_desktop_name() -> Result<Option<String>, String> {
    use std::{mem, ptr};
    use windows_sys::Win32::System::{
        StationsAndDesktops::{GetThreadDesktop, GetUserObjectInformationW, UOI_NAME},
        Threading::GetCurrentThreadId,
    };

    // SAFETY: the current thread ID is valid for the lifetime of this call;
    // GetThreadDesktop returns a borrowed desktop handle that must not be closed.
    let desktop = unsafe { GetThreadDesktop(GetCurrentThreadId()) };
    if desktop.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }

    let mut required_bytes = 0_u32;
    // SAFETY: the null buffer is an explicit size query for this desktop handle.
    let queried = unsafe {
        GetUserObjectInformationW(desktop, UOI_NAME, ptr::null_mut(), 0, &mut required_bytes)
    };
    if queried != 0 || required_bytes < mem::size_of::<u16>() as u32 {
        return Ok(None);
    }

    let units = (required_bytes as usize).div_ceil(mem::size_of::<u16>());
    let mut buffer = vec![0_u16; units];
    // SAFETY: the buffer is writable for the reported size and the output length
    // pointer is valid for the duration of the call.
    let succeeded = unsafe {
        GetUserObjectInformationW(
            desktop,
            UOI_NAME,
            buffer.as_mut_ptr().cast(),
            required_bytes,
            &mut required_bytes,
        )
    } != 0;
    if !succeeded {
        return Err(std::io::Error::last_os_error().to_string());
    }

    let length = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    String::from_utf16(&buffer[..length])
        .map(Some)
        .map_err(|error| format!("thread desktop name is not valid UTF-16: {error}"))
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
pub(crate) fn windows_diagnostic_with_thread_fallback(
    state: Result<i32, String>,
    input_desktop: Result<Option<&str>, &str>,
    thread_desktop: Result<Option<&str>, &str>,
    input_surface: Result<(), &str>,
    foreground: bool,
) -> Value {
    let fallback_name = match (&input_desktop, &thread_desktop) {
        (Err(_), Ok(Some(name))) if name.eq_ignore_ascii_case("Default") && foreground => {
            Some(*name)
        }
        _ => None,
    };
    if let Some(name) = fallback_name {
        let probe_error = input_desktop.expect_err("fallback requires an input desktop error");
        let mut report = windows_diagnostic_base(state, Ok(Some(name)), input_surface, foreground);
        report["code"] = Value::from("interactive_desktop_ready_thread_fallback");
        report["input_desktop_source"] = Value::from("current_thread_fallback");
        report["input_desktop_probe_error"] = Value::from(probe_error);
        return report;
    }
    windows_diagnostic_base(state, input_desktop, input_surface, foreground)
}

#[cfg(any(windows, test))]
pub(crate) fn windows_diagnostic_base(
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
