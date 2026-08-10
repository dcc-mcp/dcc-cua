#[cfg(feature = "gui-e2e")]
use std::path::PathBuf;
#[cfg(feature = "gui-e2e")]
use std::process::{Command, Stdio};
#[cfg(feature = "gui-e2e")]
use std::time::{Duration, Instant};

#[cfg(feature = "gui-e2e")]
use cua_driver_testkit::harness_app;
#[cfg(feature = "gui-e2e")]
use dcc_cua_client::{HostProcess, HostResponse, SnapshotTransport};
#[cfg(feature = "gui-e2e")]
use rstest::rstest;
#[cfg(feature = "gui-e2e")]
use serde_json::{Value, json};

#[cfg(feature = "gui-e2e")]
const SESSION_ID: &str = "application-lifecycle-e2e";
#[cfg(feature = "gui-e2e")]
const GRANT_ID: &str = "application-lifecycle-e2e-grant";

#[cfg(feature = "gui-e2e")]
struct ExactProcessGuard {
    pid: u32,
    armed: bool,
}

#[cfg(feature = "gui-e2e")]
impl ExactProcessGuard {
    fn new(pid: u32) -> Self {
        Self { pid, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(feature = "gui-e2e")]
impl Drop for ExactProcessGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let pid = self.pid.to_string();
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("taskkill");
            command.args(["/PID", &pid, "/T", "/F"]);
            command
        };
        #[cfg(unix)]
        let mut command = {
            let mut command = Command::new("/bin/kill");
            command.args(["-KILL", &pid]);
            command
        };
        let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

#[cfg(feature = "gui-e2e")]
fn launch_request() -> (&'static str, Value) {
    #[cfg(windows)]
    {
        let path = harness_app("harness-wpf", "CuaTestHarness.Wpf.exe");
        assert!(
            path.is_file(),
            "WPF lifecycle fixture is missing: {}",
            path.display()
        );
        ("WPF Test Harness", json!({"path": path}))
    }
    #[cfg(target_os = "linux")]
    {
        let path = harness_app("harness-gtk3", "CuaTestHarness.Gtk3");
        assert!(
            path.is_file(),
            "GTK lifecycle fixture is missing: {}",
            path.display()
        );
        ("GTK3 Test Harness", json!({"launch_path": path}))
    }
    #[cfg(target_os = "macos")]
    {
        (
            "Calculator",
            json!({
                "bundle_id": "com.apple.calculator",
                "creates_new_application_instance": true
            }),
        )
    }
}

#[cfg(feature = "gui-e2e")]
async fn host_request(host: &mut HostProcess, method: &str, params: Value) -> HostResponse {
    host.client_mut()
        .request(method, params)
        .await
        .unwrap_or_else(|error| panic!("Host {method} failed: {error}"))
}

#[cfg(feature = "gui-e2e")]
async fn wait_for_file(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    loop {
        if path.is_file() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "recording artifact is missing: {}",
            path.display()
        );
        interval.tick().await;
    }
}

#[cfg(feature = "gui-e2e")]
async fn wait_for_window_cleanup(host: &mut HostProcess, pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    loop {
        let response = host_request(
            host,
            "list_windows",
            json!({"pid": pid, "on_screen_only": false}),
        )
        .await;
        let windows = response.value["windows"]
            .as_array()
            .expect("list_windows result");
        if windows.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "terminated pid {pid} still owns windows: {}",
            response.value
        );
        interval.tick().await;
    }
}

#[cfg(feature = "gui-e2e")]
#[rstest]
#[tokio::test]
async fn host_launches_records_and_terminates_one_exact_application() {
    let binary = std::env::var_os("DCC_CUA_E2E_BINARY")
        .map(PathBuf::from)
        .expect("DCC_CUA_E2E_BINARY must point to dcc-cua");
    assert!(
        binary.is_file(),
        "Host binary is missing: {}",
        binary.display()
    );

    let mut host = HostProcess::spawn(&binary, SESSION_ID, SnapshotTransport::BinaryFrame)
        .await
        .expect("launch dcc-cua Host");
    let (application_label, launch) = launch_request();
    let launched = host_request(
        &mut host,
        "launch_app",
        json!({
            "session_id": SESSION_ID,
            "grant": {
                "task_grant_id": GRANT_ID,
                "application_label": application_label,
                "allow_app_launch": true
            },
            "launch": launch
        }),
    )
    .await;
    assert_eq!(launched.value["type"], "app_launched");
    let pid = launched.value["result"]["structuredContent"]["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .unwrap_or_else(|| panic!("launch_app omitted a valid pid: {}", launched.value));
    let mut process_guard = ExactProcessGuard::new(pid);

    let ready = host_request(
        &mut host,
        "wait_for_window",
        json!({
            "query": {"process_id": pid, "on_screen_only": true},
            "timeout_ms": 30_000,
            "interval_ms": 100
        }),
    )
    .await;
    let window = ready.value["result"]["windows"]
        .as_array()
        .and_then(|windows| {
            windows.iter().find(|window| {
                window["title"]
                    .as_str()
                    .is_some_and(|title| !title.trim().is_empty())
            })
        })
        .unwrap_or_else(|| panic!("launched pid exposed no window: {}", ready.value));
    let window_id = window["window_id"].as_u64().expect("window id");
    let window_title = window["title"].as_str().expect("window title");

    let opened = host_request(
        &mut host,
        "open_session",
        json!({
            "session_id": SESSION_ID,
            "grant": {
                "task_grant_id": GRANT_ID,
                "application_label": application_label,
                "process_id": pid,
                "window_handle": window_id,
                "window_title": window_title,
                "allow_app_terminate": true,
                "allow_recording": true
            }
        }),
    )
    .await;
    let marker_label = opened.value["marker"]["label"]
        .as_str()
        .expect("control marker label");
    assert!(marker_label.contains(SESSION_ID));
    assert!(marker_label.contains(application_label));
    let capability = opened.value["window_capability"]
        .as_str()
        .expect("window capability")
        .to_owned();

    let recording_root = tempfile::tempdir().expect("create recording root");
    let trajectory = recording_root.path().join("trajectory");
    let started = host_request(
        &mut host,
        "recording_start",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "request": {"output_dir": trajectory, "record_video": false}
        }),
    )
    .await;
    assert_eq!(
        started.value["result"]["structuredContent"]["enabled"],
        true
    );

    host_request(
        &mut host,
        "cursor_tool",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "tool": "set_agent_cursor_enabled",
            "arguments": {"enabled": false}
        }),
    )
    .await;
    let state = host_request(
        &mut host,
        "recording_state",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability
        }),
    )
    .await;
    assert_eq!(state.value["result"]["status"], "active");
    assert_eq!(state.value["result"]["healthy"], true);
    assert!(
        state.value["result"]["trajectory"]["structuredContent"]["next_turn"]
            .as_u64()
            .is_some_and(|turn| turn >= 2),
        "recording did not observe the Host mutation: {}",
        state.value
    );
    let stopped = host_request(
        &mut host,
        "recording_stop",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability
        }),
    )
    .await;
    assert_eq!(
        stopped.value["result"]["structuredContent"]["enabled"],
        false
    );
    let action_path = trajectory.join("turn-00001").join("action.json");
    wait_for_file(&action_path).await;
    let action: Value =
        serde_json::from_slice(&std::fs::read(&action_path).expect("read recording action"))
            .expect("parse recording action");
    assert_eq!(action["tool"], "set_agent_cursor_enabled");
    assert_eq!(action["arguments"]["enabled"], false);

    host_request(
        &mut host,
        "cursor_tool",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "tool": "set_agent_cursor_enabled",
            "arguments": {"enabled": true}
        }),
    )
    .await;
    let terminated = host_request(
        &mut host,
        "terminate_app",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability
        }),
    )
    .await;
    assert_eq!(terminated.value["type"], "app_terminated");
    assert_eq!(terminated.value["result"]["success"], true);
    wait_for_window_cleanup(&mut host, pid).await;
    process_guard.disarm();

    let status = host.shutdown().await.expect("stop Host process");
    assert!(status.success(), "Host exited unsuccessfully: {status}");
}
