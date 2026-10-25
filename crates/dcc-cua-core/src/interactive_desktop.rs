#[cfg(windows)]
use crate::desktop_context::{
    process_window_station_name, thread_desktop_name, thread_desktop_receives_input,
};

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
    receives_input: Result<bool, String>,
    station: Result<Option<String>, String>,
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
            receives_input: thread_desktop_receives_input(),
            station: process_window_station_name(),
            input_surface: if probe_input_surface {
                windows_input_surface()
            } else {
                Ok(())
            },
            foreground: desktop.has_foreground_window,
        }
    }

    fn desktop_ready(&self) -> bool {
        self.diagnostic()["success"] == true
    }

    fn observation_ready(&self) -> bool {
        self.diagnostic()["observation_ready"] == true
    }

    fn input_ready(&self) -> bool {
        self.diagnostic()["input_ready"] == true
    }

    fn diagnostic(&self) -> Value {
        let mut report = windows_diagnostic_with_thread_fallback(
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
        );
        let mut context = windows_execution_context(
            self.station
                .as_ref()
                .map(|name| name.as_deref())
                .map_err(String::as_str),
            self.thread_desktop
                .as_ref()
                .map(|name| name.as_deref())
                .map_err(String::as_str),
            self.receives_input
                .as_ref()
                .copied()
                .map_err(String::as_str),
            self.input_desktop
                .as_ref()
                .map(|name| name.as_deref())
                .map_err(String::as_str),
        );
        context["process_id"] = Value::from(std::process::id());
        // SAFETY: querying our current thread ID does not open or change a desktop.
        context["thread_id"] =
            Value::from(unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() });
        apply_execution_context(&mut report, context);
        report
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
    use windows_sys::Win32::{Foundation::POINT, UI::WindowsAndMessaging::GetCursorPos};

    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `point` is valid for one synchronous Win32 output write.
    if unsafe { GetCursorPos(&raw mut point) } == 0 {
        let cursor_pos_error = std::io::Error::last_os_error();
        return windows_cursor_info_input_surface().map_err(|cursor_info_error| {
            format!(
                "GetCursorPos failed: {cursor_pos_error}; GetCursorInfo failed: {cursor_info_error}"
            )
        });
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn windows_cursor_info_input_surface() -> Result<(), std::io::Error> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CURSORINFO, GetCursorInfo};

    let mut cursor_info = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `cursor_info` has the ABI-required cbSize and is valid for one
    // synchronous Win32 output write. This read-only probe does not relax the
    // desktop checks or move the cursor.
    if unsafe { GetCursorInfo(&raw mut cursor_info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(any(windows, test))]
pub(crate) fn windows_execution_context(
    station: Result<Option<&str>, &str>,
    thread_desktop: Result<Option<&str>, &str>,
    receives_input: Result<bool, &str>,
    input_desktop: Result<Option<&str>, &str>,
) -> Value {
    fn name_probe(result: Result<Option<&str>, &str>) -> Value {
        match result {
            Ok(name) => json!({"name": name, "error": Value::Null}),
            Err(error) => json!({"name": Value::Null, "error": error}),
        }
    }

    let mut thread = name_probe(thread_desktop);
    thread["receives_input"] = receives_input.ok().map_or(Value::Null, Value::Bool);
    thread["receives_input_error"] = receives_input.err().map_or(Value::Null, Value::from);
    json!({
        "source": "current_process_and_thread",
        "window_station": name_probe(station),
        "thread_desktop": thread,
        "input_desktop": name_probe(input_desktop),
    })
}

#[cfg(any(windows, test))]
pub(crate) fn apply_execution_context(report: &mut Value, context: Value) {
    let thread = context["thread_desktop"]["name"].as_str();
    let input = context["input_desktop"]["name"].as_str();
    let mismatch = matches!((thread, input), (Some(thread), Some(input)) if !thread.eq_ignore_ascii_case(input));
    let owned = context["thread_desktop"]["receives_input"] == true;
    let station = context["window_station"]["name"].as_str();
    let known = owned
        && thread.is_some_and(|name| name.eq_ignore_ascii_case("Default"))
        && station.is_some_and(|name| name.eq_ignore_ascii_case("WinSta0"))
        && !mismatch;
    report["local_inventory_ready"] = Value::Bool(known && report["success"] == true);
    if !known {
        report["success"] = Value::Bool(false);
        report["code"] = Value::from(if mismatch {
            "interactive_desktop_context_mismatch"
        } else {
            "interactive_desktop_context_unverified"
        });
        report["message"] = Value::from(
            "Current executor desktop ownership is incompatible or unverified; window discovery is unknown, not application absence. Exact-window capture requires independent native proof; activation or relaunch cannot repair this execution boundary.",
        );
        report["observation_ready"] = Value::Bool(false);
        report["input_ready"] = Value::Bool(false);
        report["retry_safe"] = Value::Bool(false);
    }
    report["execution_context"] = context;
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
