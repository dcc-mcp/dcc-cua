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
    System::Threading::{AttachThreadInput, GetCurrentThreadId},
    UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsWindow,
        SetForegroundWindow,
    },
};

use crate::{
    UiaAction, UiaError, UiaTarget,
    snapshot::{ElementFence, SnapshotState, normalize, resolve_index},
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const STDERR_LIMIT: u64 = 64 * 1024;
const BACKEND: &str = include_str!("../assets/windows_uia_backend.ps1");
const HELPERS: &str = include_str!("../assets/windows_uia_helpers.ps1");

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
        let raw = self.request(&payload)?;
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
        let foreground = foreground
            .map(|(window, process_id)| restore_foreground(window, process_id))
            .transpose();
        let raw = raw?;
        foreground?;
        ensure_ok(&raw)?;
        Ok(json!({
            "backend": "windows_uia",
            "success": true,
            "message": raw.get("message").cloned().unwrap_or(Value::Null),
            "control": raw.get("control").cloned().unwrap_or(Value::Null),
        }))
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
            std::env::temp_dir().join(format!("dcc-mcp-cua-uia-{}.ps1", Uuid::new_v4().simple()));
        let script = BACKEND.replace("# DCC_MCP_UIA_HELPERS", HELPERS);
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
