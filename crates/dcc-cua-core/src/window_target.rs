use serde::Serialize;
use serde_json::Value;
#[cfg(windows)]
use serde_json::json;

use crate::contracts::{
    ComputerUseError, ComputerUseErrorCode, ComputerUseResult, ComputerUseTargetScope,
};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct WindowTarget {
    pub(crate) pid: u32,
    pub(crate) window_id: u64,
    pub(crate) title: String,
    pub(crate) app_name: String,
    pub(crate) bounds: [i32; 4],
    #[serde(default)]
    pub(crate) is_on_screen: bool,
    #[serde(default)]
    pub(crate) is_minimized: bool,
    #[serde(default)]
    pub(crate) z_index: Option<i32>,
    #[serde(default)]
    pub(crate) is_foreground: bool,
}

impl WindowTarget {
    pub(crate) fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            pid: value["pid"].as_u64()?.try_into().ok()?,
            window_id: value["window_id"].as_u64()?,
            title: value["title"].as_str().unwrap_or_default().to_owned(),
            app_name: value["app_name"].as_str().unwrap_or_default().to_owned(),
            bounds: bounds(value["bounds"].as_object()?)?,
            is_on_screen: value["is_on_screen"].as_bool().unwrap_or(false),
            is_minimized: value["minimized"].as_bool().unwrap_or(false),
            z_index: value["z_index"]
                .as_i64()
                .and_then(|value| value.try_into().ok()),
            is_foreground: value["is_foreground"].as_bool().unwrap_or(false),
        })
    }
}

impl ComputerUseTargetScope {
    pub(crate) fn matches(&self, target: &WindowTarget) -> bool {
        self.process_id.is_none_or(|value| value == target.pid)
            && self
                .window_handle
                .is_none_or(|value| value == target.window_id)
            && self
                .window_title
                .as_deref()
                .is_none_or(|value| value == target.title)
    }
}

pub(crate) fn validate_target_policy(target: &WindowTarget) -> ComputerUseResult<()> {
    let value = format!("{} {}", target.app_name, target.title).to_ascii_lowercase();
    const DENIED: [&str; 12] = [
        "password",
        "credential",
        "authentication",
        "sign in",
        "login",
        "terminal",
        "command prompt",
        "cmd.exe",
        "powershell",
        "pwsh",
        "security",
        "consent",
    ];
    if DENIED.iter().any(|marker| value.contains(marker)) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTarget,
            "system, terminal, authentication, and password targets are not allowed",
        ));
    }
    Ok(())
}

pub(crate) fn native_inventory(
    process_id: Option<u32>,
    on_screen_only: bool,
) -> Option<Vec<Value>> {
    #[cfg(windows)]
    {
        Some(windows_inventory(process_id, on_screen_only))
    }
    #[cfg(not(windows))]
    {
        let _ = (process_id, on_screen_only);
        None
    }
}

#[cfg(windows)]
struct WindowsInventoryContext {
    process_id: Option<u32>,
    on_screen_only: bool,
    foreground: usize,
    z_index: i32,
    process_names: std::collections::HashMap<u32, String>,
    rows: Vec<Value>,
}

#[cfg(windows)]
fn windows_inventory(process_id: Option<u32>, on_screen_only: bool) -> Vec<Value> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetForegroundWindow};

    let mut context = WindowsInventoryContext {
        process_id,
        on_screen_only,
        foreground: unsafe { GetForegroundWindow() } as usize,
        z_index: 0,
        process_names: std::collections::HashMap::new(),
        rows: Vec::new(),
    };
    unsafe {
        EnumWindows(
            Some(enum_window),
            (&raw mut context).cast::<core::ffi::c_void>() as isize,
        );
    }
    context.rows
}

#[cfg(windows)]
fn window_title(hwnd: windows_sys::Win32::Foundation::HWND) -> String {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};

    let length = unsafe { GetWindowTextLengthW(hwnd) }.max(0) as usize;
    let mut buffer = vec![0_u16; length.saturating_add(1)];
    let copied = unsafe {
        GetWindowTextW(
            hwnd,
            buffer.as_mut_ptr(),
            buffer.len().try_into().unwrap_or(i32::MAX),
        )
    }
    .max(0) as usize;
    buffer.truncate(copied.min(buffer.len()));
    String::from_utf16_lossy(&buffer)
}

#[cfg(windows)]
fn process_name(pid: u32) -> String {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return "native-window".into();
    }
    let mut buffer = vec![0_u16; 32_768];
    let mut length = buffer.len() as u32;
    let resolved =
        unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) != 0 };
    unsafe { CloseHandle(process) };
    if !resolved {
        return "native-window".into();
    }
    buffer.truncate(length as usize);
    let path = String::from_utf16_lossy(&buffer);
    std::path::Path::new(&path).file_name().map_or_else(
        || "native-window".into(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(windows)]
unsafe extern "system" fn enum_window(
    hwnd: windows_sys::Win32::Foundation::HWND,
    context: isize,
) -> i32 {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    };

    let context = unsafe { &mut *(context as *mut WindowsInventoryContext) };
    let z_index = context.z_index;
    context.z_index = context.z_index.saturating_add(1);
    let mut pid = 0_u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 || context.process_id.is_some_and(|expected| expected != pid) {
        return 1;
    }
    let minimized = unsafe { IsIconic(hwnd) } != 0;
    let visible = unsafe { IsWindowVisible(hwnd) } != 0;
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) };
    let width = rect.right.saturating_sub(rect.left);
    let height = rect.bottom.saturating_sub(rect.top);
    let on_screen = visible && !minimized && width > 0 && height > 0;
    if context.on_screen_only && !on_screen {
        return 1;
    }
    let title = window_title(hwnd);
    let app_name = context
        .process_names
        .entry(pid)
        .or_insert_with(|| process_name(pid))
        .clone();
    context.rows.push(json!({
        "pid": pid,
        "window_id": hwnd as usize as u64,
        "title": title,
        "app_name": app_name,
        "bounds": {"x":rect.left, "y":rect.top, "width":width, "height":height},
        "is_on_screen": on_screen,
        "minimized": minimized,
        "z_index": z_index,
        "is_foreground": hwnd as usize == context.foreground,
        "backend": "windows-native-window-inventory",
    }));
    1
}

fn bounds(value: &serde_json::Map<String, Value>) -> Option<[i32; 4]> {
    Some([
        bound(&value["x"])?,
        bound(&value["y"])?,
        bound(&value["width"])?,
        bound(&value["height"])?,
    ])
}

fn bound(value: &Value) -> Option<i32> {
    let value = value.as_f64()?;
    (value.is_finite() && value >= i32::MIN as f64 && value <= i32::MAX as f64)
        .then(|| value.round() as i32)
}

/// Resolve an explicitly supplied native window without asking CUA to enumerate
/// every desktop window. This keeps exact-target operations usable when a UIA
/// provider is slow or blocked by another application.
#[cfg(windows)]
pub(crate) fn resolve_exact(
    scope: &ComputerUseTargetScope,
) -> ComputerUseResult<Option<WindowTarget>> {
    let Some(window_id) = scope.window_handle else {
        return Ok(None);
    };
    resolve_windows_target(scope, window_id)
}

#[cfg(windows)]
fn resolve_windows_target(
    scope: &ComputerUseTargetScope,
    window_id: u64,
) -> ComputerUseResult<Option<WindowTarget>> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowRect, IsIconic, IsWindowVisible,
    };

    let hwnd = window_id as *mut std::ffi::c_void;
    let pid = windows_window_process_id(window_id)?;
    if scope.process_id.is_some_and(|expected| expected != pid) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetUnavailable,
            "native window process identity changed",
        ));
    }

    let title = window_title(hwnd);
    if scope
        .window_title
        .as_deref()
        .is_some_and(|expected| expected != title)
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetUnavailable,
            "native window title identity changed",
        ));
    }

    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) };
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::MissingWindow,
            "native window has empty bounds",
        ));
    }

    let minimized = unsafe { IsIconic(hwnd) } != 0;
    let visible = unsafe { IsWindowVisible(hwnd) } != 0;
    let foreground = unsafe { GetForegroundWindow() } == hwnd;
    Ok(Some(WindowTarget {
        pid,
        window_id,
        title,
        app_name: process_name(pid),
        bounds: [rect.left, rect.top, width, height],
        is_on_screen: visible && !minimized,
        is_minimized: minimized,
        z_index: None,
        is_foreground: foreground,
    }))
}

#[cfg(windows)]
pub(crate) fn windows_window_process_id(window_id: u64) -> ComputerUseResult<u32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow};

    let hwnd = window_id as *mut std::ffi::c_void;
    if hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::MissingWindow,
            format!("native window {window_id} is no longer valid"),
        ));
    }
    let mut process_id = 0_u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) };
    if process_id == 0 {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::MissingWindow,
            format!("native window {window_id} has no owning process"),
        ));
    }
    Ok(process_id)
}

/// Detect a visible same-process owned window that has taken foreground input
/// from the exact granted HWND (for example a native or Qt modal file dialog).
///
/// This is intentionally detection-only. Following the returned HWND without
/// a new exact-window grant would silently widen the action scope.
#[cfg(windows)]
pub(crate) fn windows_foreground_owned_takeover(
    target: &WindowTarget,
) -> ComputerUseResult<Option<WindowTarget>> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GW_OWNER, GetForegroundWindow, GetWindow, GetWindowThreadProcessId,
    };

    let foreground = unsafe { GetForegroundWindow() };
    let target_hwnd = target.window_id as *mut std::ffi::c_void;
    if foreground.is_null() || foreground == target_hwnd {
        return Ok(None);
    }

    let mut foreground_pid = 0_u32;
    unsafe { GetWindowThreadProcessId(foreground, &mut foreground_pid) };
    if foreground_pid != target.pid {
        return Ok(None);
    }

    let mut owner = foreground;
    let mut owned_by_target = false;
    for _ in 0..16 {
        owner = unsafe { GetWindow(owner, GW_OWNER) };
        if owner.is_null() {
            break;
        }
        if owner == target_hwnd {
            owned_by_target = true;
            break;
        }
    }
    if !owned_by_target {
        return Ok(None);
    }

    resolve_windows_target(
        &ComputerUseTargetScope {
            process_id: Some(target.pid),
            window_handle: Some(foreground as usize as u64),
            window_title: None,
        },
        foreground as usize as u64,
    )
}

#[cfg(test)]
pub(crate) fn owned_foreground_takeover_relation(
    target_window: u64,
    target_pid: u32,
    foreground_window: u64,
    foreground_pid: u32,
    owner_chain: &[u64],
) -> bool {
    foreground_window != 0
        && foreground_window != target_window
        && foreground_pid == target_pid
        && owner_chain.contains(&target_window)
}
