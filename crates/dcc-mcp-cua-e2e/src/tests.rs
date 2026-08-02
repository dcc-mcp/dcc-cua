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
    let mut fixture_command = Command::new(&fixture_path);
    fixture_command
        .args(fixture_args)
        .env("CUA_E2E_FIXTURE_JOURNAL_URL", journal.url())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let fixture_child = spawn_in_job(&mut fixture_command).expect("launch official CUA fixture");
    let fixture_pid = fixture_child.id();
    let mut fixture_reaper = ChildReaper::new();
    fixture_reaper.push(fixture_child);

    let mut host = HostProcess::spawn(
        &binary,
        "controlled-gui-e2e",
        SnapshotTransport::BinaryFrame,
    )
    .await
    .expect("launch DCC-MCP CUA Host");
    let doctor = host_request(&mut host, "doctor", json!({})).await;
    assert_eq!(
        doctor.value["ready"], true,
        "Host is not GUI-ready: {}",
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
                "window_title": window_title
            }
        }),
    )
    .await;
    let capability = opened.value["window_capability"]
        .as_str()
        .expect("window capability")
        .to_owned();
    assert_eq!(opened.value["marker"]["visible"], true);

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
        "action": "set_text",
        "input_kind": "semantic",
        "intent": "ordinary_edit",
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

    host_request(&mut host, "stop_session", json!({"session_id": SESSION_ID})).await;
    let status = host.shutdown().await.expect("stop Host process");
    assert!(status.success(), "Host exited unsuccessfully: {status}");
    drop(fixture_reaper);
}
