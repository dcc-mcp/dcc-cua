#[cfg(feature = "gui-e2e")]
use std::net::TcpListener;
#[cfg(feature = "gui-e2e")]
use std::path::PathBuf;
#[cfg(feature = "gui-e2e")]
use std::process::{Command, Stdio};
#[cfg(feature = "gui-e2e")]
use std::time::{Duration, Instant};

#[cfg(feature = "gui-e2e")]
use cua_driver_testkit::{ChildReaper, FixtureJournal, harness_app, spawn_in_job};
#[cfg(feature = "gui-e2e")]
use dcc_mcp_cua_client::{HostProcess, HostResponse, SnapshotTransport};
#[cfg(feature = "gui-e2e")]
use rstest::rstest;
#[cfg(feature = "gui-e2e")]
use serde_json::{Value, json};

#[cfg(feature = "gui-e2e")]
const FIXTURE_TITLE: &str = "CuaTestHarness Electron";
#[cfg(feature = "gui-e2e")]
const FIXTURE_MARKER: &str = "WEB_HARNESS_MARKER_v1";
#[cfg(feature = "gui-e2e")]
const SESSION_ID: &str = "controlled-gui-e2e";
#[cfg(feature = "gui-e2e")]
const GRANT_ID: &str = "controlled-gui-e2e-grant";

#[cfg(feature = "gui-e2e")]
fn fixture() -> (PathBuf, &'static [&'static str]) {
    #[cfg(target_os = "windows")]
    {
        (
            harness_app("harness-electron", "CuaTestHarness.Electron.exe"),
            &[
                "--no-sandbox",
                "--disable-gpu",
                "--force-renderer-accessibility",
            ],
        )
    }
    #[cfg(target_os = "macos")]
    {
        (
            harness_app(
                "harness-electron",
                "CuaTestHarness.Electron.app/Contents/MacOS/Electron",
            ),
            &["--force-renderer-accessibility"],
        )
    }
    #[cfg(target_os = "linux")]
    {
        (
            harness_app("harness-electron", "CuaTestHarness.Electron"),
            &[
                "--no-sandbox",
                "--disable-gpu",
                "--force-renderer-accessibility",
            ],
        )
    }
}

#[cfg(all(feature = "gui-e2e", windows))]
fn wpf_fixture() -> PathBuf {
    harness_app("harness-wpf", "CuaTestHarness.Wpf.exe")
}

#[cfg(feature = "gui-e2e")]
fn first_window(response: &Value) -> &Value {
    response["result"]["windows"]
        .as_array()
        .and_then(|windows| windows.first())
        .expect("the fixture must expose one scoped window")
}

#[cfg(feature = "gui-e2e")]
fn semantic_locator(element: &Value) -> Value {
    if let Some(token) = element["element_token"].as_str() {
        return json!({"element_token": token});
    }
    let index = element["element_index"]
        .as_u64()
        .or_else(|| element["index"].as_u64())
        .expect("the semantic match must expose an element locator");
    json!({"element_index": index})
}

#[cfg(feature = "gui-e2e")]
fn assert_png(image: &[u8]) {
    assert!(
        image.len() > 8 && image.starts_with(b"\x89PNG\r\n\x1a\n"),
        "the Host must return captured PNG pixels"
    );
}

#[cfg(feature = "gui-e2e")]
fn allocate_loopback_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("allocate Electron CDP port")
        .local_addr()
        .expect("read Electron CDP port")
        .port()
}

#[cfg(feature = "gui-e2e")]
fn browser_ref_by_text(snapshot: &Value, text: &str) -> String {
    snapshot["result"]["structuredContent"]["refs"]
        .as_array()
        .and_then(|refs| {
            refs.iter().find(|entry| {
                entry["label"]
                    .as_str()
                    .or_else(|| entry["name"].as_str())
                    .is_some_and(|label| label.contains(text))
            })
        })
        .and_then(|entry| entry["ref"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("browser snapshot is missing ref text {text:?}: {snapshot}"))
}

#[cfg(feature = "gui-e2e")]
fn browser_snapshot_id(snapshot: &Value) -> String {
    let structured = &snapshot["result"]["structuredContent"];
    structured["snapshot_id"]
        .as_str()
        .or_else(|| structured["snapshot"]["id"].as_str())
        .expect("browser snapshot id")
        .to_owned()
}

#[cfg(feature = "gui-e2e")]
fn wait_for_journal(journal: &FixtureJournal, id: &str, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while journal.text(id).as_deref() != Some(expected) {
        assert!(
            Instant::now() < deadline,
            "fixture state {id:?} did not reach {expected:?}: {}",
            journal.snapshot()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(feature = "gui-e2e")]
async fn host_request(host: &mut HostProcess, method: &str, params: Value) -> HostResponse {
    let started = Instant::now();
    let response = tokio::time::timeout(
        Duration::from_secs(90),
        host.client_mut().request(method, params),
    )
    .await
    .unwrap_or_else(|_| panic!("Host request {method:?} exceeded 90 seconds"))
    .unwrap_or_else(|error| panic!("Host request {method:?} failed: {error:?}"));
    eprintln!(
        "Host request {method:?} completed in {:?}",
        started.elapsed()
    );
    response
}

#[cfg(feature = "gui-e2e")]
#[rstest]
#[tokio::test]
async fn controlled_electron_round_trip() {
    let binary = std::env::var_os("DCC_MCP_CUA_E2E_BINARY")
        .map(PathBuf::from)
        .expect("DCC_MCP_CUA_E2E_BINARY must point to dcc-mcp-cua");
    assert!(
        binary.is_file(),
        "Host binary is missing: {}",
        binary.display()
    );

    let (fixture_path, fixture_args) = fixture();
    assert!(
        fixture_path.is_file(),
        "official CUA Electron fixture is missing: {}",
        fixture_path.display()
    );
    let journal = FixtureJournal::start();
    let cdp_port = allocate_loopback_port();
    let mut fixture_command = Command::new(&fixture_path);
    fixture_command
        .args(fixture_args)
        .env("CUA_E2E_FIXTURE_JOURNAL_URL", journal.url())
        .env("CUA_ELECTRON_CDP_PORT", cdp_port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let fixture_child = spawn_in_job(&mut fixture_command).expect("launch official CUA fixture");
    let fixture_pid = fixture_child.id();
    let mut fixture_reaper = ChildReaper::new();
    fixture_reaper.push(fixture_child);

    let mut host = HostProcess::spawn_with_host_args(
        &binary,
        "controlled-gui-e2e",
        SnapshotTransport::BinaryFrame,
        &["--grant", "existing-profile"],
    )
    .await
    .expect("launch DCC-MCP CUA Host");
    let doctor = host_request(&mut host, "doctor", json!({})).await;
    assert_eq!(
        doctor.value["checks"]["driver"]["success"], true,
        "Host driver is unavailable: {}",
        doctor.value
    );
    assert_eq!(
        doctor.value["checks"]["health"]["success"], true,
        "Host health check failed: {}",
        doctor.value
    );

    let ready = host_request(
        &mut host,
        "wait_for_window",
        json!({
            "query": {"process_id": fixture_pid, "on_screen_only": true},
            "timeout_ms": 30_000,
            "interval_ms": 100
        }),
    )
    .await;
    let window = first_window(&ready.value);
    let window_id = window["window_id"].as_u64().expect("window id");
    let window_title = window["title"].as_str().expect("window title");
    assert!(
        window_title.starts_with(FIXTURE_TITLE),
        "unexpected fixture title: {window_title}"
    );

    wait_for_journal(&journal, "page-marker", FIXTURE_MARKER);
    let opened = host_request(
        &mut host,
        "open_session",
        json!({
            "session_id": SESSION_ID,
            "grant": {
                "task_grant_id": GRANT_ID,
                "dcc_type": "electron",
                "process_id": fixture_pid,
                "window_handle": window_id,
                "window_title": window_title,
                "allow_raw_input": true,
                "allow_browser_input": true,
                "allow_browser_prepare": true,
                "allow_session_escalation": true
            }
        }),
    )
    .await;
    let capability = opened.value["window_capability"]
        .as_str()
        .expect("window capability")
        .to_owned();
    host_request(
        &mut host,
        "escalate_session",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "reason": "ax_tree_pixel_mismatch",
            "detail": "controlled GUI E2E permits exact-window pixel fallback"
        }),
    )
    .await;
    assert_eq!(opened.value["marker"]["visible"], true);
    assert_eq!(opened.value["banner"]["visible"], cfg!(windows));
    assert_eq!(
        opened.value["banner"]["target_frame_visible"],
        cfg!(windows)
    );
    assert_eq!(opened.value["banner"]["stop_key"], "Escape");
    assert_eq!(opened.value["cursor"]["visible"], true);
    assert_eq!(opened.value["cursor"]["shape"], "mouse_pointer");
    assert_eq!(opened.value["cursor"]["theme"], "cua.default");
    assert_eq!(
        opened.value["cursor"]["render_backend"],
        if cfg!(windows) {
            "host-native-overlay"
        } else if cfg!(target_os = "linux") {
            "cua-driver-sdk"
        } else {
            "unavailable"
        }
    );

    host_request(
        &mut host,
        "cursor_tool",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "tool": "move_cursor",
            "arguments": {"x": 64, "y": 64}
        }),
    )
    .await;
    let cursor_state = host_request(
        &mut host,
        "cursor_tool",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "tool": "get_agent_cursor_state",
            "arguments": {}
        }),
    )
    .await;
    let cursor_state = cursor_state.value["result"].to_string();
    assert!(cursor_state.contains("cua.default"), "{cursor_state}");
    assert!(cursor_state.contains("enabled"), "{cursor_state}");

    let snapshot = host_request(
        &mut host,
        "snapshot",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "max_nodes": 1_000,
            "max_depth": 20
        }),
    )
    .await;
    assert_png(
        snapshot
            .binary_attachment
            .as_deref()
            .expect("snapshot PNG attachment"),
    );
    assert!(snapshot.value["root"].to_string().contains(FIXTURE_MARKER));
    let observation_id = snapshot.value["observation_id"]
        .as_str()
        .expect("observation id")
        .to_owned();
    let accessibility_state_id = snapshot.value["accessibility_state_id"]
        .as_str()
        .expect("accessibility state id")
        .to_owned();

    let found = host_request(
        &mut host,
        "find",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "query": {"text": "txt-input", "max_results": 1}
        }),
    )
    .await;
    let input = found.value["matches"]
        .as_array()
        .and_then(|matches| matches.first())
        .expect("semantic text input match");
    let locator = semantic_locator(input);
    let expected = "host-ipc-e2e";
    let mut action = json!({
        "action": if cfg!(target_os = "linux") { "type" } else { "set_text" },
        "input_kind": "semantic",
        "intent": "ordinary_edit",
        "delivery_mode": if cfg!(target_os = "linux") { "foreground" } else { "background" },
        "text": expected
    });
    action
        .as_object_mut()
        .expect("action object")
        .extend(locator.as_object().expect("locator object").clone());

    let completed = host_request(
        &mut host,
        "execute_action",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "observation_id": observation_id,
            "accessibility_state_id": accessibility_state_id,
            "action": action,
            "capture_after": true,
            "post_snapshot_max_nodes": 1_000,
            "post_snapshot_max_depth": 20
        }),
    )
    .await;
    assert_eq!(
        completed.value["success"], true,
        "action failed: {}",
        completed.value
    );
    assert_png(
        completed
            .binary_attachment
            .as_deref()
            .expect("post-action PNG attachment"),
    );

    let verified = host_request(
        &mut host,
        "wait_for",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "condition": {
                "kind": "text_contains",
                "text": format!("mirror={expected}"),
                "timeout_ms": 5_000,
                "interval_ms": 100
            }
        }),
    )
    .await;
    assert_eq!(
        verified.value["success"], true,
        "semantic verification failed: {}",
        verified.value
    );
    wait_for_journal(&journal, "lbl-input-mirror", &format!("mirror={expected}"));

    let prepared = host_request(
        &mut host,
        "browser_prepare",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "request": {
                "allow_launch": false,
                "strategy": {"kind": "existing_profile"}
            }
        }),
    )
    .await;
    assert_eq!(
        prepared.value["result"]["structuredContent"]["prepared"], true,
        "browser_prepare failed: {}",
        prepared.value
    );

    let bound = host_request(
        &mut host,
        "browser_snapshot",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "request": {"snapshot_format": "semantic_v2"}
        }),
    )
    .await;
    let browser_state = &bound.value["result"]["structuredContent"];
    assert_eq!(browser_state["binding_quality"], "exact");
    let target_id = browser_state["target_id"]
        .as_str()
        .expect("browser target id")
        .to_owned();
    let tab_id = browser_state["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|tab| tab["active"] == true))
        .and_then(|tab| tab["tab_id"].as_str())
        .expect("active browser tab id")
        .to_owned();

    let browser_snapshot = host_request(
        &mut host,
        "browser_snapshot",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "request": {
                "target_id": target_id,
                "tab_id": tab_id,
                "snapshot_format": "semantic_v2"
            }
        }),
    )
    .await;
    let snapshot_id = browser_snapshot_id(&browser_snapshot.value);
    let increment_ref = browser_ref_by_text(&browser_snapshot.value, "Increment");
    host_request(
        &mut host,
        "browser_click",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "request": {
                "target_id": target_id,
                "tab_id": tab_id,
                "snapshot_id": snapshot_id,
                "ref": increment_ref
            }
        }),
    )
    .await;
    wait_for_journal(&journal, "lbl-counter", "counter=1");

    let browser_snapshot = host_request(
        &mut host,
        "browser_snapshot",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "request": {
                "target_id": target_id,
                "tab_id": tab_id,
                "snapshot_format": "semantic_v2"
            }
        }),
    )
    .await;
    let snapshot_id = browser_snapshot_id(&browser_snapshot.value);
    let input_ref = browser_ref_by_text(&browser_snapshot.value, "txt-input");
    let browser_expected = "browser-host-ipc-e2e";
    host_request(
        &mut host,
        "browser_type",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "request": {
                "target_id": target_id,
                "tab_id": tab_id,
                "snapshot_id": snapshot_id,
                "ref": input_ref,
                "text": browser_expected,
                "replace": true
            }
        }),
    )
    .await;
    wait_for_journal(
        &journal,
        "lbl-input-mirror",
        &format!("mirror={browser_expected}"),
    );

    host_request(&mut host, "stop_session", json!({"session_id": SESSION_ID})).await;
    let status = host.shutdown().await.expect("stop Host process");
    assert!(status.success(), "Host exited unsuccessfully: {status}");
    drop(fixture_reaper);
}

#[cfg(all(feature = "gui-e2e", not(windows)))]
#[rstest]
#[tokio::test]
async fn concurrent_electron_sessions_keep_distinct_capabilities() {
    let binary = std::env::var_os("DCC_MCP_CUA_E2E_BINARY")
        .map(PathBuf::from)
        .expect("DCC_MCP_CUA_E2E_BINARY must point to dcc-mcp-cua");
    assert!(
        binary.is_file(),
        "Host binary is missing: {}",
        binary.display()
    );

    let (fixture_path, fixture_args) = fixture();
    assert!(
        fixture_path.is_file(),
        "official CUA Electron fixture is missing: {}",
        fixture_path.display()
    );
    let mut journals = Vec::new();
    let mut fixture_reaper = ChildReaper::new();
    let mut fixture_pids = Vec::new();
    for _ in 0..2 {
        let journal = FixtureJournal::start();
        let cdp_port = allocate_loopback_port();
        let mut command = Command::new(&fixture_path);
        command
            .args(fixture_args)
            .env("CUA_E2E_FIXTURE_JOURNAL_URL", journal.url())
            .env("CUA_ELECTRON_CDP_PORT", cdp_port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        let child = spawn_in_job(&mut command).expect("launch official CUA fixture");
        fixture_pids.push(child.id());
        fixture_reaper.push(child);
        journals.push(journal);
    }

    let mut host = HostProcess::spawn(
        &binary,
        "concurrent-controlled-gui-e2e",
        SnapshotTransport::SharedMemory,
    )
    .await
    .expect("launch DCC-MCP CUA Host");
    for journal in &journals {
        wait_for_journal(journal, "page-marker", FIXTURE_MARKER);
    }

    let mut sessions = Vec::new();
    for (index, pid) in fixture_pids.iter().copied().enumerate() {
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
        let window = first_window(&ready.value);
        let window_id = window["window_id"].as_u64().expect("window id");
        let window_title = window["title"].as_str().expect("window title");
        assert!(window_title.starts_with(FIXTURE_TITLE));
        let session_id = format!("concurrent-gui-{index}");
        let grant_id = format!("concurrent-gui-grant-{index}");
        let opened = host_request(
            &mut host,
            "open_session",
            json!({
                "session_id": session_id,
                "grant": {
                    "task_grant_id": grant_id,
                    "dcc_type": "electron",
                    "process_id": pid,
                    "window_handle": window_id,
                    "window_title": window_title,
                    "allow_raw_input": true,
                    "allow_session_escalation": true
                }
            }),
        )
        .await;
        assert_eq!(opened.value["marker"]["visible"], true);
        assert_eq!(opened.value["cursor"]["visible"], true);
        let capability = opened.value["window_capability"]
            .as_str()
            .expect("window capability")
            .to_owned();
        host_request(
            &mut host,
            "escalate_session",
            json!({
                "session_id": session_id,
                "task_grant_id": grant_id,
                "window_capability": capability,
                "reason": "ax_tree_pixel_mismatch",
                "detail": "controlled concurrent E2E permits exact-window pixel fallback"
            }),
        )
        .await;
        sessions.push((session_id, grant_id, capability));
    }

    assert_ne!(sessions[0].0, sessions[1].0);
    assert_ne!(sessions[0].2, sessions[1].2);
    for (session_id, grant_id, capability) in &sessions {
        let state = host_request(
            &mut host,
            "get_session_state",
            json!({
                "session_id": session_id,
                "task_grant_id": grant_id,
                "window_capability": capability
            }),
        )
        .await;
        assert_eq!(
            state.value["state"]["structuredContent"]["session"],
            session_id.as_str(),
            "unexpected session state response: {}",
            state.value
        );
        assert_eq!(
            state.value["state"]["structuredContent"]["effective_scope"],
            "window"
        );
    }
    for (index, (session_id, grant_id, capability)) in sessions.iter().enumerate() {
        let snapshot = host_request(
            &mut host,
            "snapshot",
            json!({
                "session_id": session_id,
                "task_grant_id": grant_id,
                "window_capability": capability,
                "max_nodes": 1_000,
                "max_depth": 20
            }),
        )
        .await;
        assert_eq!(
            snapshot.value["image"]["encoding"], "shared_memory",
            "parallel snapshot must use the negotiated shared-memory transport: {}",
            snapshot.value
        );
        assert!(snapshot.value["root"].to_string().contains(FIXTURE_MARKER));
        let observation_id = snapshot.value["observation_id"]
            .as_str()
            .expect("parallel observation id")
            .to_owned();
        let accessibility_state_id = snapshot.value["accessibility_state_id"]
            .as_str()
            .expect("parallel accessibility state id")
            .to_owned();
        let found = host_request(
            &mut host,
            "find",
            json!({
                "session_id": session_id,
                "task_grant_id": grant_id,
                "window_capability": capability,
                "query": {"text": "txt-input", "max_results": 1}
            }),
        )
        .await;
        let input = found.value["matches"]
            .as_array()
            .and_then(|matches| matches.first())
            .expect("parallel semantic text input match");
        let mut action = json!({
            "action": if cfg!(target_os = "linux") { "type" } else { "set_text" },
            "input_kind": "semantic",
            "intent": "ordinary_edit",
            "delivery_mode": if cfg!(target_os = "linux") { "foreground" } else { "background" },
            "text": format!("parallel-host-ipc-e2e-{index}")
        });
        action
            .as_object_mut()
            .expect("parallel action object")
            .extend(
                semantic_locator(input)
                    .as_object()
                    .expect("parallel locator")
                    .clone(),
            );
        host_request(
            &mut host,
            "execute_action",
            json!({
                "session_id": session_id,
                "task_grant_id": grant_id,
                "window_capability": capability,
                "observation_id": observation_id,
                "accessibility_state_id": accessibility_state_id,
                "action": action,
                "capture_after": true,
                "post_snapshot_max_nodes": 1_000,
                "post_snapshot_max_depth": 20
            }),
        )
        .await;
        wait_for_journal(
            &journals[index],
            "lbl-input-mirror",
            &format!("mirror=parallel-host-ipc-e2e-{index}"),
        );
    }
    for (session_id, _, _) in &sessions {
        host_request(&mut host, "stop_session", json!({"session_id": session_id})).await;
    }
    let status = host.shutdown().await.expect("stop Host process");
    assert!(status.success(), "Host exited unsuccessfully: {status}");
    drop(fixture_reaper);
}

#[cfg(all(feature = "gui-e2e", windows))]
#[rstest]
#[tokio::test]
async fn windows_background_uia_keeps_concurrent_host_sessions_isolated() {
    let binary = std::env::var_os("DCC_MCP_CUA_E2E_BINARY")
        .map(PathBuf::from)
        .expect("DCC_MCP_CUA_E2E_BINARY must point to dcc-mcp-cua");
    let fixture_path = wpf_fixture();
    assert!(
        fixture_path.is_file(),
        "official CUA WPF fixture is missing: {}",
        fixture_path.display()
    );

    let mut fixture_reaper = ChildReaper::new();
    let mut fixture_pids = Vec::new();
    for _ in 0..2 {
        let child = spawn_in_job(
            Command::new(&fixture_path)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit()),
        )
        .expect("launch official CUA WPF fixture");
        fixture_pids.push(child.id());
        fixture_reaper.push(child);
    }

    let mut host = HostProcess::spawn(
        &binary,
        "windows-background-uia-e2e",
        SnapshotTransport::BinaryFrame,
    )
    .await
    .expect("launch DCC-MCP CUA Host");
    let mut window_targets = Vec::new();
    for pid in fixture_pids {
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
        let window = first_window(&ready.value);
        window_targets.push((
            pid,
            window["window_id"].as_u64().expect("window id"),
            window["title"].as_str().expect("window title").to_owned(),
        ));
    }

    let foreground = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }
        as usize as u64;
    window_targets.sort_by_key(|(_, window_handle, _)| *window_handle == foreground);
    assert_ne!(
        window_targets[0].1, foreground,
        "two WPF windows must include one non-foreground target"
    );

    let mut sessions = Vec::new();
    for (index, (pid, window_handle, window_title)) in window_targets.iter().enumerate() {
        let session_id = format!("windows-background-uia-{index}");
        let grant_id = format!("windows-background-uia-grant-{index}");
        let opened = host_request(
            &mut host,
            "open_session",
            json!({
                "session_id": session_id,
                "grant": {
                    "task_grant_id": grant_id,
                    "dcc_type": "wpf",
                    "process_id": pid,
                    "window_handle": window_handle,
                    "window_title": window_title
                }
            }),
        )
        .await;
        sessions.push((
            session_id,
            grant_id,
            opened.value["window_capability"]
                .as_str()
                .expect("window capability")
                .to_owned(),
            *window_handle,
        ));
    }
    assert_ne!(sessions[0].2, sessions[1].2);
    let foreground = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }
        as usize as u64;
    sessions.sort_by_key(|(_, _, _, window_handle)| *window_handle == foreground);
    assert_ne!(sessions[0].3, foreground);

    let mut background_action_observed = false;
    for (index, (session_id, grant_id, capability, window_handle)) in sessions.iter().enumerate() {
        let snapshot = host_request(
            &mut host,
            "accessibility_snapshot",
            json!({
                "session_id": session_id,
                "task_grant_id": grant_id,
                "window_capability": capability,
                "max_nodes": 1_000,
                "max_depth": 20
            }),
        )
        .await;
        assert_eq!(snapshot.value["root"]["backend"], "windows_uia");
        let found = host_request(
            &mut host,
            "find",
            json!({
                "session_id": session_id,
                "task_grant_id": grant_id,
                "window_capability": capability,
                "query": {"text": "txt-input", "max_results": 1}
            }),
        )
        .await;
        let input = found.value["matches"]
            .as_array()
            .and_then(|matches| matches.first())
            .expect("WPF UIA input match");
        let mut action = json!({
            "action": "set_text",
            "input_kind": "semantic",
            "intent": "ordinary_edit",
            "delivery_mode": "background",
            "text": format!("windows-background-uia-e2e-{index}")
        });
        action.as_object_mut().expect("action object").extend(
            semantic_locator(input)
                .as_object()
                .expect("locator object")
                .clone(),
        );
        let foreground_before =
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() } as usize
                as u64;
        let completed = host_request(
            &mut host,
            "execute_action",
            json!({
                "session_id": session_id,
                "task_grant_id": grant_id,
                "window_capability": capability,
                "observation_id": snapshot.value["observation_id"],
                "accessibility_state_id": snapshot.value["accessibility_state_id"],
                "action": action,
                "capture_after": false
            }),
        )
        .await;
        assert_eq!(completed.value["success"], true);
        if foreground_before != *window_handle {
            background_action_observed = true;
            assert_eq!(
                unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }
                    as usize as u64,
                foreground_before,
                "background UIA action must preserve the foreground window"
            );
        }
        let updated = host_request(
            &mut host,
            "accessibility_snapshot",
            json!({
                "session_id": session_id,
                "task_grant_id": grant_id,
                "window_capability": capability,
                "max_nodes": 1_000,
                "max_depth": 20
            }),
        )
        .await;
        assert!(
            updated.value["root"]
                .to_string()
                .contains(&format!("windows-background-uia-e2e-{index}")),
            "background UIA post-state is missing: {}",
            updated.value
        );
    }
    assert!(
        background_action_observed,
        "two WPF sessions must exercise at least one background UIA action"
    );
    for (session_id, _, _, _) in &sessions {
        host_request(&mut host, "stop_session", json!({"session_id": session_id})).await;
    }

    let status = host.shutdown().await.expect("stop Host process");
    assert!(status.success(), "Host exited unsuccessfully: {status}");
    drop(fixture_reaper);
}
