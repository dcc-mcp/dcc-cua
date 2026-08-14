use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread::{self, JoinHandle},
    time::Duration,
};

use serde_json::{Value, json};
use uuid::Uuid;
use windows_sys::Win32::{
    Foundation::RECT,
    System::Threading::{AttachThreadInput, GetCurrentThreadId},
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON},
        WindowsAndMessaging::{
            BringWindowToTop, GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetWindowRect,
            GetWindowThreadProcessId, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, IsIconic, IsWindow,
            IsWindowVisible, PostMessageW, SMTO_ABORTIFHUNG, SW_RESTORE, SWP_ASYNCWINDOWPOS,
            SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SendMessageTimeoutW,
            SetForegroundWindow, SetWindowPos, ShowWindowAsync, WM_CLOSE, WM_NULL,
        },
    },
};

use crate::{
    UiaAction, UiaError, UiaTarget, WindowsForegroundRelation, WindowsPointerButton,
    WindowsRawInputSnapshot, WindowsWindowIdentity,
    snapshot::{ElementFence, SnapshotState, normalize, resolve_index},
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const ACTIVATION_INPUT_SYNC_TIMEOUT_MS: u32 = 250;
const STDERR_LIMIT: u64 = 64 * 1024;
const BACKEND: &str = include_str!("../assets/windows_uia_backend.ps1");
const HELPERS: &str = include_str!("../assets/windows_uia_helpers.ps1");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ActivationZOrder {
    TopMost,
    NotTopMost,
}

pub(super) fn activation_topmost_bounce(mut apply: impl FnMut(ActivationZOrder)) {
    apply(ActivationZOrder::TopMost);
    apply(ActivationZOrder::NotTopMost);
}

pub(super) fn window_frame_matches(actual: [i32; 4], requested: [i32; 4]) -> bool {
    actual == requested
}

pub fn set_window_frame(
    target: UiaTarget,
    requested: [i32; 4],
    mutation_available: impl FnOnce() -> Result<(), UiaError>,
) -> Result<[i32; 4], UiaError> {
    mutation_available()?;
    let expected = require_exact_window_handle(target, "window frame mutation")?;
    let [x, y, width, height] = requested;
    if width <= 0 || height <= 0 {
        return Err(UiaError::InvalidAction(
            "window frame width and height must be positive".into(),
        ));
    }
    let applied = unsafe {
        SetWindowPos(
            expected,
            std::ptr::null_mut(),
            x,
            y,
            width,
            height,
            SWP_ASYNCWINDOWPOS | SWP_NOZORDER,
        )
    } != 0;
    if !applied {
        return Err(UiaError::BackendUnavailable(format!(
            "SetWindowPos failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    for _ in 0..40 {
        let expected = require_exact_window_handle(target, "window frame validation")?;
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(expected, &mut rect) } != 0 {
            let actual = [
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            ];
            if window_frame_matches(actual, requested) {
                return Ok(actual);
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    Err(UiaError::BackendUnavailable(
        "the exact window did not reach the requested frame".into(),
    ))
}

pub fn post_close_window(
    target: UiaTarget,
    mutation_available: impl FnOnce() -> Result<(), UiaError>,
) -> Result<(), UiaError> {
    mutation_available()?;
    let expected = require_exact_window_handle(target, "window close")?;
    if unsafe { PostMessageW(expected, WM_CLOSE, 0, 0) } == 0 {
        return Err(UiaError::BackendUnavailable(format!(
            "PostMessageW(WM_CLOSE) failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    for _ in 0..80 {
        if unsafe { IsWindow(expected) } == 0 {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(UiaError::BackendUnavailable(
        "the exact window did not close after WM_CLOSE".into(),
    ))
}

fn live_window_identity(
    window_handle: windows_sys::Win32::Foundation::HWND,
) -> Option<WindowsWindowIdentity> {
    if window_handle.is_null() || unsafe { IsWindow(window_handle) } == 0 {
        return None;
    }
    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(window_handle, &mut process_id) };
    (process_id != 0).then_some(WindowsWindowIdentity {
        window_handle: window_handle as usize as u64,
        process_id,
    })
}

/// Sample the system and target GUI-thread state immediately after a scoped
/// synthetic mouse-button DOWN. This reports evidence; it does not claim that
/// an arbitrary target framework consumed the event.
pub fn snapshot_raw_pointer_input_after_down(
    target: UiaTarget,
    button: WindowsPointerButton,
) -> Result<WindowsRawInputSnapshot, UiaError> {
    let expected = target.window_handle as windows_sys::Win32::Foundation::HWND;
    if expected.is_null() || unsafe { IsWindow(expected) } == 0 {
        return Err(UiaError::InvalidTarget(
            "the exact target window no longer exists after button-down".into(),
        ));
    }
    let mut actual_target_process_id = 0;
    let target_thread =
        unsafe { GetWindowThreadProcessId(expected, &mut actual_target_process_id) };
    if target_thread == 0 || actual_target_process_id != target.process_id {
        return Err(UiaError::InvalidTarget(
            "the exact target window changed ownership after button-down".into(),
        ));
    }

    let virtual_key = match button {
        WindowsPointerButton::Left => VK_LBUTTON,
        WindowsPointerButton::Right => VK_RBUTTON,
        WindowsPointerButton::Middle => VK_MBUTTON,
    };
    let async_button_down =
        unsafe { GetAsyncKeyState(i32::from(virtual_key)) } as u16 & 0x8000 != 0;

    let foreground = live_window_identity(unsafe { GetForegroundWindow() });
    let foreground_relation = match foreground {
        Some(identity) if identity.window_handle == target.window_handle => {
            WindowsForegroundRelation::ExactTarget
        }
        Some(identity) if identity.process_id == target.process_id => {
            WindowsForegroundRelation::SameProcess
        }
        Some(_) => WindowsForegroundRelation::ForeignProcess,
        None => WindowsForegroundRelation::NoForeground,
    };

    let mut thread_info: GUITHREADINFO = unsafe { std::mem::zeroed() };
    thread_info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
    let capture_query_succeeded = unsafe { GetGUIThreadInfo(target_thread, &mut thread_info) } != 0;
    let target_thread_capture = capture_query_succeeded
        .then(|| live_window_identity(thread_info.hwndCapture))
        .flatten();
    let capture_owned_by_target_process =
        target_thread_capture.is_some_and(|identity| identity.process_id == target.process_id);

    Ok(WindowsRawInputSnapshot {
        async_button_down,
        target: WindowsWindowIdentity {
            window_handle: target.window_handle,
            process_id: target.process_id,
        },
        foreground,
        foreground_relation,
        target_thread_capture,
        capture_query_succeeded,
        capture_owned_by_target_process,
    })
}

pub struct UiaSession {
    target: UiaTarget,
    worker: Option<UiaWorker>,
    snapshot: Option<SnapshotState>,
}

impl UiaSession {
    pub fn new(target: UiaTarget) -> Self {
        Self {
            target,
            worker: None,
            snapshot: None,
        }
    }

    pub fn snapshot(
        &mut self,
        max_nodes: u32,
        max_depth: u32,
        allow_owned_standard_menu_popup: bool,
    ) -> Result<Value, UiaError> {
        let payload = json!({
            "mode": "snapshot",
            "scope": self.scope(allow_owned_standard_menu_popup),
            "max_depth": max_depth.clamp(1, 64),
            "max_nodes": max_nodes.clamp(1, 5_000),
        });
        let first = self.request(&payload);
        let raw = retry_read_only_after_backend_failure(first, || {
            self.worker = None;
            self.request(&payload)
        })?;
        ensure_ok(&raw)?;
        let (value, state) = normalize(&raw)?;
        self.snapshot = Some(state);
        Ok(value)
    }

    pub fn perform(&mut self, action: &UiaAction) -> Result<Value, UiaError> {
        let state = self.snapshot.as_ref().ok_or_else(|| {
            UiaError::StaleSnapshot("take a fresh Windows UIA snapshot before acting".into())
        })?;
        let index = resolve_index(state, action.element_index, action.element_token.as_deref())?;
        let fence = state.fences[index].clone();
        let action_name = normalized_action(&action.action)?;
        let foreground = (action.delivery_mode.as_deref() == Some("background")).then(|| {
            (
                unsafe { GetForegroundWindow() } as usize,
                self.target.process_id,
            )
        });
        let payload = json!({
            "mode": "act",
            "scope": self.scope(true),
            "max_depth": 64,
            "max_nodes": 5_000,
            "expected_fence": fence_value(&fence),
            "action": {
                "control_id": fence.control_id,
                "action": action_name,
                "text": action.text.as_deref().unwrap_or(""),
                "checked": action.checked.unwrap_or(false),
            },
        });
        let raw = self.request(&payload);
        self.snapshot = None;
        let foreground =
            foreground.map(|(window, process_id)| restore_foreground(window, process_id));
        let raw = raw?;
        completed_action_result(&raw, foreground)
    }

    fn scope(&self, allow_owned_standard_menu_popup: bool) -> Value {
        json!({
            "window_titles": [],
            "process_ids": [self.target.process_id],
            "process_names": [],
            "window_handles": [self.target.window_handle],
            "native_scope_trusted": true,
            "allow_owned_standard_menu_popup": allow_owned_standard_menu_popup,
        })
    }

    fn request(&mut self, payload: &Value) -> Result<Value, UiaError> {
        if self.worker.is_none() {
            self.worker = Some(UiaWorker::start()?);
        }
        self.worker
            .as_mut()
            .expect("worker was initialized")
            .request(payload)
    }
}

pub(crate) fn retry_read_only_after_backend_failure<T>(
    first: Result<T, UiaError>,
    retry: impl FnOnce() -> Result<T, UiaError>,
) -> Result<T, UiaError> {
    match first {
        Err(UiaError::BackendUnavailable(_)) => retry(),
        result => result,
    }
}

pub(crate) fn completed_action_result(
    raw: &Value,
    foreground_restore: Option<Result<(), UiaError>>,
) -> Result<Value, UiaError> {
    ensure_ok(raw)?;
    let foreground_restore = match foreground_restore {
        None => Value::Null,
        Some(Ok(())) => json!({
            "requested": true,
            "success": true,
        }),
        Some(Err(error)) => json!({
            "requested": true,
            "success": false,
            "message": error_detail(&error),
        }),
    };
    Ok(json!({
        "backend": "windows_uia",
        "success": true,
        "action_executed": true,
        "message": raw.get("message").cloned().unwrap_or(Value::Null),
        "control": raw.get("control").cloned().unwrap_or(Value::Null),
        "foreground_restore": foreground_restore,
    }))
}

fn error_detail(error: &UiaError) -> &str {
    match error {
        UiaError::InvalidTarget(message)
        | UiaError::StaleSnapshot(message)
        | UiaError::PermissionDenied(message)
        | UiaError::InvalidAction(message)
        | UiaError::BackendUnavailable(message) => message,
        UiaError::Unsupported => "Windows UI Automation fallback is unavailable on this platform",
    }
}

fn restore_foreground(expected: usize, controlled_process_id: u32) -> Result<(), UiaError> {
    if !foreground_restore_still_required(expected, controlled_process_id) {
        return Ok(());
    }
    let expected = expected as windows_sys::Win32::Foundation::HWND;
    if expected.is_null() || unsafe { IsWindow(expected) } == 0 {
        return Err(background_delivery_error());
    }
    unsafe { SetForegroundWindow(expected) };
    if !foreground_restore_still_required(expected as usize, controlled_process_id) {
        return Ok(());
    }

    let current = unsafe { GetForegroundWindow() };
    let current_thread = unsafe { GetCurrentThreadId() };
    let foreground_thread = unsafe { GetWindowThreadProcessId(current, std::ptr::null_mut()) };
    let expected_thread = unsafe { GetWindowThreadProcessId(expected, std::ptr::null_mut()) };
    let attached_foreground = foreground_thread != 0
        && foreground_thread != current_thread
        && unsafe { AttachThreadInput(current_thread, foreground_thread, 1) } != 0;
    let attached_expected = expected_thread != 0
        && expected_thread != current_thread
        && expected_thread != foreground_thread
        && unsafe { AttachThreadInput(current_thread, expected_thread, 1) } != 0;
    unsafe {
        BringWindowToTop(expected);
        SetForegroundWindow(expected);
    }
    let mut preserved = false;
    for _ in 0..20 {
        if !foreground_restore_still_required(expected as usize, controlled_process_id) {
            preserved = true;
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    unsafe {
        if attached_expected {
            AttachThreadInput(current_thread, expected_thread, 0);
        }
        if attached_foreground {
            AttachThreadInput(current_thread, foreground_thread, 0);
        }
    }
    if preserved {
        Ok(())
    } else {
        Err(background_delivery_error())
    }
}

pub fn activate_window(
    target: UiaTarget,
    activation_available: impl FnOnce() -> Result<(), UiaError>,
) -> Result<(), UiaError> {
    let expected = require_available_window_handle(target, "activation frame capture")?;
    let frame_before_activation = capture_visible_window_frame(expected);
    input_gated_window_mutation(activation_available, || activate_window_after_gate(target))?;
    let expected = require_available_window_handle(target, "activation final validation")?;
    if let Some(frame) = frame_before_activation {
        restore_window_frame_after_activation(expected, frame)?;
    }
    synchronize_activated_input_queue(expected)?;
    if unsafe { GetForegroundWindow() } != expected {
        return Err(UiaError::BackendUnavailable(
            "the exact target was no longer foreground at activation final validation".into(),
        ));
    }
    Ok(())
}

fn capture_visible_window_frame(
    expected: windows_sys::Win32::Foundation::HWND,
) -> Option<[i32; 4]> {
    if unsafe { IsWindowVisible(expected) } == 0 || unsafe { IsIconic(expected) } != 0 {
        return None;
    }
    let mut rect = RECT::default();
    (unsafe { GetWindowRect(expected, &mut rect) } != 0).then_some([
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
    ])
}

fn restore_window_frame_after_activation(
    expected: windows_sys::Win32::Foundation::HWND,
    requested: [i32; 4],
) -> Result<(), UiaError> {
    let mut current = RECT::default();
    if unsafe { GetWindowRect(expected, &mut current) } == 0 {
        return Err(UiaError::BackendUnavailable(
            "Windows could not read the exact target frame after activation".into(),
        ));
    }
    let actual = [
        current.left,
        current.top,
        current.right - current.left,
        current.bottom - current.top,
    ];
    if window_frame_matches(actual, requested) {
        return Ok(());
    }
    let [x, y, width, height] = requested;
    if unsafe {
        SetWindowPos(
            expected,
            std::ptr::null_mut(),
            x,
            y,
            width,
            height,
            SWP_ASYNCWINDOWPOS | SWP_NOZORDER | SWP_SHOWWINDOW,
        )
    } == 0
    {
        return Err(UiaError::BackendUnavailable(format!(
            "SetWindowPos failed while preserving the activation frame: {}",
            std::io::Error::last_os_error()
        )));
    }
    for _ in 0..40 {
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(expected, &mut rect) } != 0 {
            let actual = [
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            ];
            if window_frame_matches(actual, requested) {
                return Ok(());
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    Err(UiaError::BackendUnavailable(
        "the exact target frame changed during activation and could not be restored".into(),
    ))
}

fn synchronize_activated_input_queue(
    expected: windows_sys::Win32::Foundation::HWND,
) -> Result<(), UiaError> {
    let mut message_result = 0;
    let synchronized = unsafe {
        SendMessageTimeoutW(
            expected,
            WM_NULL,
            0,
            0,
            SMTO_ABORTIFHUNG,
            ACTIVATION_INPUT_SYNC_TIMEOUT_MS,
            &mut message_result,
        )
    } != 0;
    if !synchronized {
        return Err(UiaError::BackendUnavailable(
            "the exact target did not process its foreground activation before input".into(),
        ));
    }
    if unsafe { GetForegroundWindow() } != expected {
        return Err(UiaError::BackendUnavailable(
            "the exact target lost foreground while synchronizing activation input".into(),
        ));
    }
    Ok(())
}

fn activate_window_after_gate(target: UiaTarget) -> Result<(), UiaError> {
    let expected = require_available_window_handle(target, "activation")?;
    if unsafe { GetForegroundWindow() } == expected {
        return Ok(());
    }

    unsafe { SetForegroundWindow(expected) };
    if unsafe { GetForegroundWindow() } == expected {
        return Ok(());
    }

    let expected = require_available_window_handle(target, "fallback activation")?;
    unsafe {
        SetWindowPos(
            expected,
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_ASYNCWINDOWPOS | SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        SetForegroundWindow(expected);
    }
    if unsafe { GetForegroundWindow() } != expected {
        let mut topmost_applied = false;
        let mut topmost_released = false;
        activation_topmost_bounce(|step| {
            let insert_after = match step {
                ActivationZOrder::TopMost => HWND_TOPMOST,
                ActivationZOrder::NotTopMost => HWND_NOTOPMOST,
            };
            let succeeded = unsafe {
                SetWindowPos(
                    expected,
                    insert_after,
                    0,
                    0,
                    0,
                    0,
                    SWP_ASYNCWINDOWPOS | SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                )
            } != 0;
            match step {
                ActivationZOrder::TopMost => topmost_applied = succeeded,
                ActivationZOrder::NotTopMost => topmost_released = succeeded,
            }
        });
        if topmost_applied && !topmost_released {
            return Err(UiaError::BackendUnavailable(
                "Windows raised the exact target but could not release temporary topmost state"
                    .into(),
            ));
        }
        unsafe { SetForegroundWindow(expected) };
    }
    if unsafe { GetForegroundWindow() } != expected {
        activate_with_attached_input(expected);
    }
    let activated = (0..20).any(|_| {
        if unsafe { GetForegroundWindow() } == expected {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
        false
    });
    if activated {
        Ok(())
    } else {
        Err(UiaError::BackendUnavailable(
            "Windows could not make the exact target window foreground".into(),
        ))
    }
}

fn activate_with_attached_input(expected: windows_sys::Win32::Foundation::HWND) {
    let current_thread = unsafe { GetCurrentThreadId() };
    let foreground = unsafe { GetForegroundWindow() };
    let foreground_thread = unsafe { GetWindowThreadProcessId(foreground, std::ptr::null_mut()) };
    let expected_thread = unsafe { GetWindowThreadProcessId(expected, std::ptr::null_mut()) };
    let attached_foreground = foreground_thread != 0
        && foreground_thread != current_thread
        && unsafe { AttachThreadInput(current_thread, foreground_thread, 1) } != 0;
    let attached_expected = expected_thread != 0
        && expected_thread != current_thread
        && expected_thread != foreground_thread
        && unsafe { AttachThreadInput(current_thread, expected_thread, 1) } != 0;
    unsafe {
        BringWindowToTop(expected);
        SetForegroundWindow(expected);
        if attached_expected {
            AttachThreadInput(current_thread, expected_thread, 0);
        }
        if attached_foreground {
            AttachThreadInput(current_thread, foreground_thread, 0);
        }
    }
}

fn require_exact_window_handle(
    target: UiaTarget,
    operation: &str,
) -> Result<windows_sys::Win32::Foundation::HWND, UiaError> {
    let expected = target.window_handle as windows_sys::Win32::Foundation::HWND;
    let exists = !expected.is_null() && unsafe { IsWindow(expected) } != 0;
    let mut actual_process_id = 0;
    if exists {
        unsafe { GetWindowThreadProcessId(expected, &mut actual_process_id) };
    }
    if !exact_window_ownership_matches(exists, target.process_id, actual_process_id) {
        return Err(UiaError::InvalidTarget(format!(
            "the exact {operation} target no longer exists or belongs to the granted process"
        )));
    }
    Ok(expected)
}

fn require_available_window_handle(
    target: UiaTarget,
    operation: &str,
) -> Result<windows_sys::Win32::Foundation::HWND, UiaError> {
    let expected = require_exact_window_handle(target, operation)?;
    let exists = unsafe { IsWindow(expected) } != 0;
    let visible = exists && unsafe { IsWindowVisible(expected) } != 0;
    let minimized = exists && unsafe { IsIconic(expected) } != 0;
    let mut actual_process_id = 0;
    if exists {
        unsafe { GetWindowThreadProcessId(expected, &mut actual_process_id) };
    }
    if !exact_window_available_for_activation(
        exists,
        visible,
        minimized,
        target.process_id,
        actual_process_id,
    ) {
        let state = if minimized {
            "target_minimized"
        } else {
            "target_unavailable"
        };
        return Err(UiaError::InvalidTarget(format!(
            "{state}: the exact {operation} target must be visible and not minimized"
        )));
    }
    Ok(expected)
}

pub(crate) const fn exact_window_ownership_matches(
    window_exists: bool,
    expected_process_id: u32,
    actual_process_id: u32,
) -> bool {
    window_exists && actual_process_id != 0 && actual_process_id == expected_process_id
}

pub(crate) const fn exact_window_available_for_activation(
    window_exists: bool,
    visible: bool,
    minimized: bool,
    expected_process_id: u32,
    actual_process_id: u32,
) -> bool {
    exact_window_ownership_matches(window_exists, expected_process_id, actual_process_id)
        && visible
        && !minimized
}

pub(crate) fn input_gated_window_mutation<T, E>(
    input_available: impl FnOnce() -> Result<(), E>,
    mutation: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    input_available()?;
    mutation()
}

pub(crate) fn run_restore_activate_mutation_sequence<E>(
    restore_input_available: impl FnOnce() -> Result<(), E>,
    restore: impl FnOnce() -> Result<(), E>,
    activate_input_available: impl FnOnce() -> Result<(), E>,
    activate: impl FnOnce() -> Result<(), E>,
) -> Result<(), E> {
    input_gated_window_mutation(restore_input_available, restore)?;
    input_gated_window_mutation(activate_input_available, activate)
}

/// Restore and foreground one exact PID/HWND target after an explicit Host
/// request. No input is injected and no other process window is eligible.
pub fn restore_and_activate_window(
    target: UiaTarget,
    restore_input_available: impl FnOnce() -> Result<(), UiaError>,
    activate_input_available: impl FnOnce() -> Result<(), UiaError>,
) -> Result<(), UiaError> {
    let expected = require_exact_window_handle(target, "restore")?;
    run_restore_activate_mutation_sequence(
        restore_input_available,
        || {
            require_exact_window_handle(target, "restore")?;
            if unsafe { IsIconic(expected) } != 0 {
                if unsafe { ShowWindowAsync(expected, SW_RESTORE) } == 0 {
                    return Err(UiaError::BackendUnavailable(
                        "Windows refused to request restoration of the exact target".into(),
                    ));
                }
                let restored = (0..40).any(|_| {
                    if unsafe { IsIconic(expected) } == 0 {
                        return true;
                    }
                    thread::sleep(Duration::from_millis(10));
                    false
                });
                if !restored {
                    return Err(UiaError::BackendUnavailable(
                        "the exact target remained minimized after the explicit restore request"
                            .into(),
                    ));
                }
            }
            Ok(())
        },
        activate_input_available,
        || activate_window_after_gate(target),
    )?;
    let mut final_process_id = 0;
    let final_exists = unsafe { IsWindow(expected) } != 0;
    if final_exists {
        unsafe { GetWindowThreadProcessId(expected, &mut final_process_id) };
    }
    if !exact_window_ownership_matches(final_exists, target.process_id, final_process_id)
        || unsafe { IsIconic(expected) } != 0
        || unsafe { IsWindowVisible(expected) } == 0
        || unsafe { GetForegroundWindow() } != expected
    {
        return Err(UiaError::BackendUnavailable(
            "the exact target did not finish restored, visible, and foreground".into(),
        ));
    }
    Ok(())
}

fn foreground_restore_still_required(expected: usize, controlled_process_id: u32) -> bool {
    let current = unsafe { GetForegroundWindow() };
    let mut current_process_id = 0;
    if !current.is_null() {
        unsafe { GetWindowThreadProcessId(current, &mut current_process_id) };
    }
    foreground_restore_required(
        expected,
        current as usize,
        current_process_id,
        controlled_process_id,
    )
}

pub(crate) fn foreground_restore_required(
    expected: usize,
    current: usize,
    current_process_id: u32,
    controlled_process_id: u32,
) -> bool {
    current != expected && (current == 0 || current_process_id == controlled_process_id)
}

fn background_delivery_error() -> UiaError {
    UiaError::BackendUnavailable(
        "Windows UIA action completed but could not preserve the foreground window".into(),
    )
}

fn normalized_action(action: &str) -> Result<&str, UiaError> {
    match action {
        "set_value" => Ok("set_text"),
        "click" | "toggle" | "set_text" | "focus" | "select_option" => Ok(action),
        _ => Err(UiaError::InvalidAction(format!(
            "semantic action {action:?} is not supported by Windows UIA"
        ))),
    }
}

fn fence_value(fence: &ElementFence) -> Value {
    json!({
        "identity": fence.identity,
        "is_password": fence.is_password,
        "name": fence.name,
        "automation_id": fence.automation_id,
        "class_name": fence.class_name,
        "policy_tier": fence.policy_tier,
    })
}

fn ensure_ok(value: &Value) -> Result<(), UiaError> {
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("request failed")
        .to_owned();
    match value
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "stale_observation" | "not_found" => Err(UiaError::StaleSnapshot(message)),
        "permission_denied" => Err(UiaError::PermissionDenied(message)),
        "invalid_target" | "missing_window" => Err(UiaError::InvalidTarget(message)),
        "unsupported_action" => Err(UiaError::InvalidAction(message)),
        _ => Err(UiaError::BackendUnavailable(message)),
    }
}

struct UiaWorker {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    responses: Receiver<Vec<u8>>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<Vec<u8>>>,
    stderr: Vec<u8>,
    script_path: PathBuf,
}

impl UiaWorker {
    fn start() -> Result<Self, UiaError> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let script_path =
            std::env::temp_dir().join(format!("dcc-cua-uia-{}.ps1", Uuid::new_v4().simple()));
        let script = BACKEND.replace("# DCC_CUA_UIA_HELPERS", HELPERS);
        std::fs::write(&script_path, script).map_err(|error| {
            UiaError::BackendUnavailable(format!("materialize UIA worker: {error}"))
        })?;
        let child = Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_file(&script_path);
                return Err(UiaError::BackendUnavailable(format!(
                    "start UIA worker: {error}"
                )));
            }
        };
        let stdin = child.stdin.take().expect("piped UIA stdin");
        let stdout = child.stdout.take().expect("piped UIA stdout");
        let stderr = child.stderr.take().expect("piped UIA stderr");
        let (sender, responses) = mpsc::channel();
        let stdout_reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut response = Vec::new();
                match reader.read_until(b'\n', &mut response) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        while response
                            .last()
                            .is_some_and(|byte| matches!(byte, b'\r' | b'\n'))
                        {
                            response.pop();
                        }
                        if sender.send(response).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let stderr_reader = thread::spawn(move || read_bounded(stderr, STDERR_LIMIT));
        let mut worker = Self {
            child: Some(child),
            stdin: Some(stdin),
            responses,
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
            stderr: Vec::new(),
            script_path,
        };
        worker.wait_until_ready()?;
        Ok(worker)
    }

    fn wait_until_ready(&mut self) -> Result<(), UiaError> {
        let response = match self.responses.recv_timeout(STARTUP_TIMEOUT) {
            Ok(response) => response,
            Err(RecvTimeoutError::Timeout) => {
                return Err(self.fail("UIA worker startup timed out after 15 seconds"));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(self.fail("UIA worker closed during startup"));
            }
        };
        let response: Value = serde_json::from_slice(&response)
            .map_err(|error| self.fail(format!("decode UIA worker readiness: {error}")))?;
        if response["type"] != "ready" {
            return Err(self.fail("UIA worker returned an invalid readiness message"));
        }
        Ok(())
    }

    fn request(&mut self, payload: &Value) -> Result<Value, UiaError> {
        if let Some(status) = self
            .child
            .as_mut()
            .ok_or_else(|| UiaError::BackendUnavailable("UIA worker is unavailable".into()))?
            .try_wait()
            .map_err(|error| UiaError::BackendUnavailable(format!("poll UIA worker: {error}")))?
        {
            return Err(self.fail(format!("UIA worker exited with status {status}")));
        }
        let stdin = self.stdin.as_mut().expect("active UIA stdin");
        if let Err(error) = writeln!(stdin, "{payload}").and_then(|()| stdin.flush()) {
            return Err(self.fail(format!("send UIA request: {error}")));
        }
        let response = match self.responses.recv_timeout(REQUEST_TIMEOUT) {
            Ok(response) => response,
            Err(RecvTimeoutError::Timeout) => {
                return Err(self.fail("UIA request timed out after 15 seconds"));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(self.fail("UIA worker closed without a response"));
            }
        };
        serde_json::from_slice(&response)
            .map_err(|error| self.fail(format!("decode UIA response: {error}")))
    }

    fn fail(&mut self, message: impl Into<String>) -> UiaError {
        self.shutdown();
        let mut message = message.into();
        let stderr = String::from_utf8_lossy(&self.stderr);
        if !stderr.trim().is_empty() {
            message.push_str(": ");
            message.push_str(stderr.trim());
        }
        UiaError::BackendUnavailable(message)
    }

    fn shutdown(&mut self) {
        self.stdin = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader) = self.stdout_reader.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stderr_reader.take() {
            self.stderr = reader.join().unwrap_or_default();
        }
        let _ = std::fs::remove_file(&self.script_path);
    }
}

impl Drop for UiaWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn read_bounded(reader: impl Read, limit: u64) -> Vec<u8> {
    let mut reader = reader.take(limit);
    let mut output = Vec::new();
    let _ = reader.read_to_end(&mut output);
    output
}
