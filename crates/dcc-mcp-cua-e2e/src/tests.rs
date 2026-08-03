#[cfg(feature = "gui-e2e")]
use std::net::TcpListener;
#[cfg(feature = "gui-e2e")]
use std::path::PathBuf;
#[cfg(feature = "gui-e2e")]
use std::process::{Command, Stdio};
#[cfg(feature = "gui-e2e")]
use std::time::{Duration, Instant};

#[cfg(feature = "gui-e2e")]
use cua_driver_testkit::{
    BrowserFixtureServer, ChildReaper, FixtureJournal, harness_app, spawn_in_job,
};
#[cfg(feature = "gui-e2e")]
use dcc_mcp_cua_client::{
    HostClient, HostClientError, HostProcess, HostResponse, SnapshotTransport,
};
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
const BROWSER_COMPLETENESS_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>CUA browser completeness</title></head>
<body>
  <p data-cua-id="page-marker">BROWSER_COMPLETENESS_MARKER_v1</p>
  <input data-cua-id="upload" id="upload" aria-label="standalone-upload" type="file">
  <span data-cua-id="upload-state" id="upload-state">upload=0</span>
  <a data-cua-id="download" aria-label="standalone-download" href="/download" download>Download fixture</a>
  <script>
    const publish = () => {
      const state = {};
      document.querySelectorAll('[data-cua-id]').forEach(element => {
        const entry = { text: (element.textContent || '').trim() };
        if ('value' in element) entry.value = element.value;
        state[element.dataset.cuaId] = entry;
      });
      fetch(window.__CUA_E2E_FIXTURE_JOURNAL_URL, {
        method: 'POST', headers: {'Content-Type': 'text/plain'}, body: JSON.stringify(state)
      }).catch(() => {});
    };
    document.getElementById('upload').addEventListener('change', event => {
      const names = Array.from(event.target.files).map(file => file.name).join(',');
      document.getElementById('upload-state').textContent = `upload=${event.target.files.length}:${names}`;
      publish();
    });
    new MutationObserver(publish).observe(document.body, {subtree:true, childList:true, characterData:true});
    window.addEventListener('load', publish, {once:true});
  </script>
</body></html>"#;

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
fn native_menu_fixture() -> (PathBuf, &'static str) {
    #[cfg(target_os = "windows")]
    {
        (wpf_fixture(), "wpf")
    }
    #[cfg(target_os = "macos")]
    {
        (
            harness_app(
                "harness-appkit",
                "CuaTestHarness.AppKit.app/Contents/MacOS/CuaTestHarness.AppKit",
            ),
            "appkit",
        )
    }
    #[cfg(target_os = "linux")]
    {
        (harness_app("harness-gtk3", "CuaTestHarness.Gtk3"), "gtk3")
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
fn screenshot_point(snapshot: &Value, element: &Value) -> (f64, f64) {
    let (frame, width_key, height_key) = if element["frame"].is_object() {
        (&element["frame"], "w", "h")
    } else {
        (&element["bounds"], "width", "height")
    };
    let center_x = frame["x"].as_f64().expect("element frame x")
        + frame[width_key].as_f64().expect("element frame width") / 2.0;
    let center_y = frame["y"].as_f64().expect("element frame y")
        + frame[height_key].as_f64().expect("element frame height") / 2.0;
    let source = snapshot["observation"]["source_rect"]
        .as_array()
        .expect("snapshot source rect");
    let image_width = snapshot["observation"]["width"]
        .as_f64()
        .expect("snapshot width");
    let image_height = snapshot["observation"]["height"]
        .as_f64()
        .expect("snapshot height");
    let x = (center_x - source[0].as_f64().expect("source x")) * image_width
        / source[2].as_f64().expect("source width");
    let y = (center_y - source[1].as_f64().expect("source y")) * image_height
        / source[3].as_f64().expect("source height");
    eprintln!(
        "raw-input target frame={frame} source={} image={image_width}x{image_height} point=({x}, {y})",
        snapshot["observation"]["source_rect"]
    );
    assert!(
        x >= 0.0 && x < image_width && y >= 0.0 && y < image_height,
        "element center ({x}, {y}) is outside the exact-window snapshot ({image_width}x{image_height})"
    );
    (x, y)
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
    let deadline = Instant::now() + Duration::from_secs(15);
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
fn wait_for_browser_fixture(server: &BrowserFixtureServer, id: &str, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while server.text(id).as_deref() != Some(expected) {
        assert!(
            Instant::now() < deadline,
            "browser fixture state {id:?} did not reach {expected:?}: {}",
            server.snapshot()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(all(feature = "gui-e2e", windows))]
fn wait_for_fixture_file(path: &std::path::Path, id: &str, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let state = std::fs::read(path)
            .ok()
            .and_then(|body| serde_json::from_slice::<Value>(&body).ok());
        if state.as_ref().and_then(|state| state[id]["text"].as_str()) == Some(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "fixture file state {id:?} did not reach {expected:?}: {}",
            state.unwrap_or(Value::Null)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(all(feature = "gui-e2e", windows))]
fn physically_focus_window(window_handle: u64) {
    // Match CUA's sentinel setup: a real click completes the Windows foreground-lock handshake.
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
        SendInput,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetForegroundWindow, GetWindowRect, SetCursorPos,
    };

    let window = window_handle as *mut core::ffi::c_void;
    let mut rect = RECT::default();
    let mut cursor = POINT::default();
    unsafe {
        assert_ne!(GetWindowRect(window, &mut rect), 0, "read fixture bounds");
        assert_ne!(GetCursorPos(&mut cursor), 0, "read cursor position");
        assert_ne!(
            SetCursorPos((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2),
            0,
            "move cursor onto fixture"
        );
    }
    let inputs = [
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dwFlags: MOUSEEVENTF_LEFTDOWN,
                    ..MOUSEINPUT::default()
                },
            },
        },
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dwFlags: MOUSEEVENTF_LEFTUP,
                    ..MOUSEINPUT::default()
                },
            },
        },
    ];
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    unsafe {
        assert_ne!(
            SetCursorPos(cursor.x, cursor.y),
            0,
            "restore cursor position"
        );
    }
    assert_eq!(sent, inputs.len() as u32, "focus fixture with one click");
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { GetForegroundWindow() } != window {
        assert!(
            Instant::now() < deadline,
            "fixture did not become foreground"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(feature = "gui-e2e")]
async fn host_request(host: &mut HostProcess, method: &str, params: Value) -> HostResponse {
    client_request(host.client_mut(), method, params).await
}

#[cfg(feature = "gui-e2e")]
async fn client_request(client: &mut HostClient, method: &str, params: Value) -> HostResponse {
    let started = Instant::now();
    let response = tokio::time::timeout(Duration::from_secs(90), client.request(method, params))
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
async fn expect_user_interrupted(client: &mut HostClient, params: Value) {
    let error = tokio::time::timeout(
        Duration::from_secs(90),
        client.request("get_session_state", params),
    )
    .await
    .expect("interrupted Host request exceeded 90 seconds")
    .expect_err("the second endpoint session must observe the shared stop");
    assert!(
        matches!(error, HostClientError::Remote { ref code, .. } if code == "user_interrupted"),
        "unexpected shared-stop response: {error:?}"
    );
}

#[cfg(feature = "gui-e2e")]
async fn start_endpoint_clients(
    binary: &std::path::Path,
    reaper: &mut ChildReaper,
    client_name: &str,
) -> (Vec<HostClient>, Option<tempfile::TempDir>) {
    #[cfg(windows)]
    let (endpoint, endpoint_directory) = (
        format!(
            r"\\.\pipe\dcc-mcp-cua-gui-e2e-{}-{client_name}",
            std::process::id()
        ),
        None,
    );
    #[cfg(unix)]
    let (endpoint, endpoint_directory) = {
        let directory = tempfile::tempdir().expect("create Host endpoint directory");
        (
            directory
                .path()
                .join("host.sock")
                .to_string_lossy()
                .into_owned(),
            Some(directory),
        )
    };
    let mut host_command = Command::new(binary);
    host_command
        .args(["host", "--endpoint"])
        .arg(&endpoint)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    reaper
        .spawn(&mut host_command)
        .expect("launch endpoint DCC-MCP CUA Host");

    let mut clients = Vec::new();
    for index in 0..2 {
        let deadline = Instant::now() + Duration::from_secs(15);
        let client = loop {
            match HostClient::connect_with_transport(
                endpoint.clone(),
                format!("{client_name}-{index}"),
                SnapshotTransport::SharedMemory,
            )
            .await
            {
                Ok(client) => break client,
                Err(error) => {
                    assert!(
                        Instant::now() < deadline,
                        "endpoint Host did not accept client {index}: {error:?}"
                    );
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        };
        clients.push(client);
    }
    (clients, endpoint_directory)
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
                "allow_browser_download": true,
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
        } else {
            "cua-driver-sdk"
        }
    );

    if opened.value["cursor"]["render_backend"] != "unavailable" {
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
    }

    let bound = |name: &str| {
        window["bounds"][name]
            .as_f64()
            .unwrap_or_else(|| panic!("fixture window omitted numeric {name} bound: {window}"))
    };
    let initial_x = bound("x");
    let initial_y = bound("y");
    let requested = [
        if initial_x > 32.0 {
            initial_x - 16.0
        } else {
            initial_x + 16.0
        },
        if initial_y > 32.0 {
            initial_y - 16.0
        } else {
            initial_y + 16.0
        },
        bound("width"),
        bound("height"),
    ];
    let frame = host_request(
        &mut host,
        "set_window_frame",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "frame": {
                "x": requested[0],
                "y": requested[1],
                "width": requested[2],
                "height": requested[3]
            }
        }),
    )
    .await;
    assert_eq!(frame.value["result"]["success"], true, "{}", frame.value);
    assert_eq!(frame.value["result"]["effect"], "confirmed");
    let actual = frame.value["result"]["target"]["bounds"]
        .as_array()
        .expect("revalidated window bounds")
        .iter()
        .map(|value| value.as_f64().expect("numeric revalidated bound"))
        .collect::<Vec<_>>();
    assert_eq!(actual.len(), requested.len());
    for (actual, requested) in actual.iter().zip(requested) {
        assert!(
            (actual - requested).abs() <= 2.0,
            "CUA confirmed a frame without matching independent readback: {}",
            frame.value
        );
    }

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

    let raw_snapshot = host_request(
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
    let raw_target = host_request(
        &mut host,
        "find",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "query": {"text": "Increment", "max_results": 1}
        }),
    )
    .await;
    let raw_target = raw_target.value["matches"]
        .as_array()
        .and_then(|matches| matches.first())
        .expect("raw-input click target");
    let (x, y) = screenshot_point(&raw_snapshot.value, raw_target);
    let raw_clicked = host_request(
        &mut host,
        "execute_action",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "observation_id": raw_snapshot.value["observation_id"],
            "accessibility_state_id": raw_snapshot.value["accessibility_state_id"],
            "action": {
                "action": "click",
                "input_kind": "raw_input",
                "intent": "navigate",
                "delivery_mode": "foreground",
                "x": x,
                "y": y
            },
            "capture_after": true,
            "post_snapshot_max_nodes": 1_000,
            "post_snapshot_max_depth": 20
        }),
    )
    .await;
    assert_eq!(
        raw_clicked.value["success"], true,
        "raw-input click failed: {}",
        raw_clicked.value
    );
    wait_for_journal(&journal, "lbl-counter", "counter=1");

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
    wait_for_journal(&journal, "lbl-counter", "counter=2");

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
    let click_target_ref = browser_ref_by_text(&browser_snapshot.value, "Click target");
    host_request(
        &mut host,
        "browser_pointer",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "request": {
                "target_id": target_id,
                "tab_id": tab_id,
                "snapshot_id": browser_snapshot_id(&browser_snapshot.value),
                "ref": click_target_ref,
                "action": "double_click",
                "input_route": "dom_event"
            }
        }),
    )
    .await;
    wait_for_journal(&journal, "lbl-last-action", "last_action=double_click");

    let browser_fixture = BrowserFixtureServer::start(BROWSER_COMPLETENESS_HTML);
    host_request(
        &mut host,
        "browser_navigate",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "request": {
                "target_id": target_id,
                "tab_id": tab_id,
                "url": browser_fixture.page_url()
            }
        }),
    )
    .await;
    wait_for_browser_fixture(
        &browser_fixture,
        "page-marker",
        "BROWSER_COMPLETENESS_MARKER_v1",
    );

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
    let upload_ref = browser_ref_by_text(&browser_snapshot.value, "standalone-upload");
    let upload_directory = tempfile::tempdir().expect("create browser upload directory");
    let upload_path = upload_directory.path().join("fixture-upload.txt");
    std::fs::write(&upload_path, b"fixture upload payload").expect("write browser upload file");
    let upload_path = std::fs::canonicalize(upload_path).expect("canonical browser upload file");
    let uploaded = host_request(
        &mut host,
        "browser_set_input_files",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "request": {
                "target_id": target_id,
                "tab_id": tab_id,
                "snapshot_id": browser_snapshot_id(&browser_snapshot.value),
                "ref": upload_ref,
                "files": [upload_path]
            }
        }),
    )
    .await;
    assert!(
        !uploaded.value.to_string().contains("fixture-upload.txt"),
        "browser upload response leaked a local path: {}",
        uploaded.value
    );
    wait_for_browser_fixture(
        &browser_fixture,
        "upload-state",
        "upload=1:fixture-upload.txt",
    );

    let primed_dialog = host_request(
        &mut host,
        "browser_dialog",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "request": {
                "target_id": target_id,
                "tab_id": tab_id,
                "action": "inspect"
            }
        }),
    )
    .await;
    assert_eq!(
        primed_dialog.value["result"]["structuredContent"]["present"],
        false
    );

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
    let download_ref = browser_ref_by_text(&browser_snapshot.value, "standalone-download");
    let download_directory = tempfile::tempdir().expect("create browser download directory");
    host_request(
        &mut host,
        "browser_download",
        json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": capability,
            "request": {
                "target_id": target_id,
                "tab_id": tab_id,
                "snapshot_id": browser_snapshot_id(&browser_snapshot.value),
                "ref": download_ref,
                "destination_root": download_directory.path()
            }
        }),
    )
    .await;
    let downloads = std::fs::read_dir(download_directory.path())
        .expect("read browser download directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("enumerate browser downloads");
    assert_eq!(
        downloads.len(),
        1,
        "expected one completed browser download"
    );
    assert_eq!(
        std::fs::read(downloads[0].path()).expect("read browser download"),
        b"CUA_DRIVER_BROWSER_DOWNLOAD_FIXTURE_V1\n"
    );

    host_request(&mut host, "stop_session", json!({"session_id": SESSION_ID})).await;
    let status = host.shutdown().await.expect("stop Host process");
    assert!(status.success(), "Host exited unsuccessfully: {status}");
    drop(fixture_reaper);
}

#[cfg(all(feature = "gui-e2e", not(windows)))]
#[rstest]
#[tokio::test]
async fn independent_endpoint_clients_serialize_scoped_raw_input() {
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
    let mut fixture_profiles = Vec::new();
    for _ in 0..2 {
        let journal = FixtureJournal::start();
        let cdp_port = allocate_loopback_port();
        let profile = tempfile::tempdir().expect("create isolated Electron profile");
        let mut command = Command::new(&fixture_path);
        command
            .args(fixture_args)
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .env("CUA_E2E_FIXTURE_JOURNAL_URL", journal.url())
            .env("CUA_ELECTRON_CDP_PORT", cdp_port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        let child = spawn_in_job(&mut command).expect("launch official CUA fixture");
        fixture_pids.push(child.id());
        fixture_reaper.push(child);
        journals.push(journal);
        fixture_profiles.push(profile);
    }

    let (mut clients, endpoint_directory) = start_endpoint_clients(
        &binary,
        &mut fixture_reaper,
        "concurrent-controlled-gui-e2e",
    )
    .await;
    for journal in &journals {
        wait_for_journal(journal, "page-marker", FIXTURE_MARKER);
    }

    let mut sessions = Vec::new();
    for (index, pid) in fixture_pids.iter().copied().enumerate() {
        let client = &mut clients[index];
        let ready = client_request(
            client,
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
        let session_id = "shared-public-session".to_owned();
        let grant_id = format!("concurrent-gui-grant-{index}");
        let opened = client_request(
            client,
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
        client_request(
            client,
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

    assert_eq!(sessions[0].0, sessions[1].0);
    assert_ne!(sessions[0].2, sessions[1].2);
    for (index, (session_id, grant_id, capability)) in sessions.iter().enumerate() {
        let state = client_request(
            &mut clients[index],
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
    let mut raw_input_requests = Vec::new();
    for (index, (session_id, grant_id, capability)) in sessions.iter().enumerate() {
        let client = &mut clients[index];
        let snapshot = client_request(
            client,
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
        let found = client_request(
            client,
            "find",
            json!({
                "session_id": session_id,
                "task_grant_id": grant_id,
                "window_capability": capability,
                "query": {"text": "Increment", "max_results": 1}
            }),
        )
        .await;
        let target = found.value["matches"]
            .as_array()
            .and_then(|matches| matches.first())
            .expect("parallel raw-input click target");
        let (x, y) = screenshot_point(&snapshot.value, target);
        raw_input_requests.push(json!({
            "session_id": session_id,
            "task_grant_id": grant_id,
            "window_capability": capability,
            "observation_id": observation_id,
            "accessibility_state_id": accessibility_state_id,
            "action": {
                "action": "click",
                "input_kind": "raw_input",
                "intent": "navigate",
                "delivery_mode": "foreground",
                "x": x,
                "y": y
            },
            "capture_after": true,
            "post_snapshot_max_nodes": 1_000,
            "post_snapshot_max_depth": 20
        }));
    }
    let (first_clients, second_clients) = clients.split_at_mut(1);
    let first = client_request(
        &mut first_clients[0],
        "execute_action",
        raw_input_requests.remove(0),
    );
    let second = client_request(
        &mut second_clients[0],
        "execute_action",
        raw_input_requests.remove(0),
    );
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.value["success"], true, "{}", first.value);
    assert_eq!(second.value["success"], true, "{}", second.value);
    for journal in &journals {
        wait_for_journal(journal, "lbl-counter", "counter=1");
    }
    let stopped = client_request(
        &mut clients[0],
        "stop_session",
        json!({"session_id": sessions[0].0}),
    )
    .await;
    assert_eq!(stopped.value["type"], "session_stopped");
    let still_active = client_request(
        &mut clients[1],
        "snapshot",
        json!({
            "session_id": sessions[1].0,
            "task_grant_id": sessions[1].1,
            "window_capability": sessions[1].2,
            "max_nodes": 1_000,
            "max_depth": 20
        }),
    )
    .await;
    assert!(
        still_active.value["root"]
            .to_string()
            .contains(FIXTURE_MARKER),
        "stopping the same public id on another connection must not end this runtime session: {}",
        still_active.value
    );
    let interrupted = client_request(&mut clients[0], "interrupt_all", json!({})).await;
    assert_eq!(interrupted.value["scope"], "host_process");
    assert_eq!(interrupted.value["stopped_window_sessions"], 0);
    expect_user_interrupted(
        &mut clients[1],
        json!({
            "session_id": sessions[1].0,
            "task_grant_id": sessions[1].1,
            "window_capability": sessions[1].2,
        }),
    )
    .await;
    drop(clients);
    drop(fixture_reaper);
    drop(endpoint_directory);
}

#[cfg(feature = "gui-e2e")]
#[rstest]
#[tokio::test]
async fn controlled_native_menu_round_trip() {
    let binary = std::env::var_os("DCC_MCP_CUA_E2E_BINARY")
        .map(PathBuf::from)
        .expect("DCC_MCP_CUA_E2E_BINARY must point to dcc-mcp-cua");
    let (fixture_path, dcc_type) = native_menu_fixture();
    assert!(
        fixture_path.is_file(),
        "official CUA native fixture is missing: {}",
        fixture_path.display()
    );

    let mut command = Command::new(&fixture_path);
    #[cfg(windows)]
    let fixture_state_dir = tempfile::tempdir().expect("create native fixture state directory");
    #[cfg(windows)]
    let fixture_state_path = fixture_state_dir.path().join("state.json");
    #[cfg(windows)]
    command.env("CUA_E2E_FIXTURE_STATE_PATH", &fixture_state_path);
    command.stdout(Stdio::null()).stderr(Stdio::inherit());
    let fixture_child = spawn_in_job(&mut command).expect("launch official CUA native fixture");
    let fixture_pid = fixture_child.id();
    let mut fixture_reaper = ChildReaper::new();
    fixture_reaper.push(fixture_child);
    #[cfg(windows)]
    wait_for_fixture_file(&fixture_state_path, "page-marker", "WPF_HARNESS_MARKER_v1");

    let mut host = HostProcess::spawn(
        &binary,
        "controlled-native-menu-e2e",
        SnapshotTransport::BinaryFrame,
    )
    .await
    .expect("launch DCC-MCP CUA Host");
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
    let opened = host_request(
        &mut host,
        "open_session",
        json!({
            "session_id": "controlled-native-menu-e2e",
            "grant": {
                "task_grant_id": "controlled-native-menu-e2e-grant",
                "dcc_type": dcc_type,
                "process_id": fixture_pid,
                "window_handle": window_id,
                "window_title": window_title,
                "allow_menu_invoke": true
            }
        }),
    )
    .await;
    let capability = opened.value["window_capability"]
        .as_str()
        .expect("window capability")
        .to_owned();
    let initial = host_request(
        &mut host,
        "accessibility_snapshot",
        json!({
            "session_id": "controlled-native-menu-e2e",
            "task_grant_id": "controlled-native-menu-e2e-grant",
            "window_capability": capability,
            "max_nodes": 1_000,
            "max_depth": 20
        }),
    )
    .await;
    assert!(
        initial.value["root"]
            .to_string()
            .contains("menu_action=none"),
        "native fixture initial menu state is missing: {}",
        initial.value
    );

    let invoked = host_request(
        &mut host,
        "invoke_menu",
        json!({
            "session_id": "controlled-native-menu-e2e",
            "task_grant_id": "controlled-native-menu-e2e-grant",
            "window_capability": capability,
            "request": {"path": ["Window", "Arrange", "Left"]}
        }),
    )
    .await;
    assert_eq!(invoked.value["result"]["success"], true);
    assert_eq!(invoked.value["result"]["effect"], "unverifiable");
    assert_eq!(invoked.value["result"]["verification_required"], true);

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let post = host_request(
            &mut host,
            "accessibility_snapshot",
            json!({
                "session_id": "controlled-native-menu-e2e",
                "task_grant_id": "controlled-native-menu-e2e-grant",
                "window_capability": capability,
                "max_nodes": 1_000,
                "max_depth": 20
            }),
        )
        .await;
        if post.value["root"]
            .to_string()
            .contains("menu_action=window_arrange_left")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "native menu post-state did not update: {}",
            post.value
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    host_request(
        &mut host,
        "stop_session",
        json!({"session_id": "controlled-native-menu-e2e"}),
    )
    .await;
    let status = host.shutdown().await.expect("stop Host process");
    assert!(status.success(), "Host exited unsuccessfully: {status}");
    drop(fixture_reaper);
}

#[cfg(all(feature = "gui-e2e", windows))]
#[rstest]
#[tokio::test]
async fn windows_endpoint_sessions_keep_background_uia_and_share_escape() {
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
    let mut fixture_state_dirs = Vec::new();
    for index in 0..2 {
        let state_dir = tempfile::tempdir().expect("create WPF fixture state directory");
        let state_path = state_dir.path().join(format!("state-{index}.json"));
        let mut command = Command::new(&fixture_path);
        command
            .env("CUA_E2E_FIXTURE_STATE_PATH", &state_path)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        let child = spawn_in_job(&mut command).expect("launch official CUA WPF fixture");
        fixture_pids.push(child.id());
        fixture_reaper.push(child);
        wait_for_fixture_file(&state_path, "page-marker", "WPF_HARNESS_MARKER_v1");
        fixture_state_dirs.push(state_dir);
    }

    let (mut clients, endpoint_directory) =
        start_endpoint_clients(&binary, &mut fixture_reaper, "windows-background-uia-e2e").await;
    let mut window_targets = Vec::new();
    for (index, pid) in fixture_pids.into_iter().enumerate() {
        let ready = client_request(
            &mut clients[index],
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
        let session_id = "shared-windows-public-session".to_owned();
        let grant_id = format!("windows-background-uia-grant-{index}");
        let opened = client_request(
            &mut clients[index],
            "open_session",
            json!({
                "session_id": session_id,
                "grant": {
                    "task_grant_id": grant_id,
                    "dcc_type": "wpf",
                    "process_id": pid,
                    "window_handle": window_handle,
                    "window_title": window_title,
                    "allow_raw_input": true,
                    "allow_session_escalation": true
                }
            }),
        )
        .await;
        sessions.push((
            index,
            session_id,
            grant_id,
            opened.value["window_capability"]
                .as_str()
                .expect("window capability")
                .to_owned(),
            *window_handle,
        ));
    }
    assert_eq!(sessions[0].1, sessions[1].1);
    assert_ne!(sessions[0].3, sessions[1].3);
    let foreground = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }
        as usize as u64;
    sessions.sort_by_key(|(_, _, _, _, window_handle)| *window_handle == foreground);
    assert_ne!(sessions[0].4, foreground);

    let mut background_action_observed = false;
    for (index, (client_index, session_id, grant_id, capability, window_handle)) in
        sessions.iter().enumerate()
    {
        let client = &mut clients[*client_index];
        let snapshot = client_request(
            client,
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
        let found = client_request(
            client,
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
        let completed = client_request(
            client,
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
        let updated = client_request(
            client,
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
    let (active_client, first_session_id, first_grant_id, first_capability, window_handle) =
        sessions
            .iter()
            .find(|(client_index, ..)| *client_index == 0)
            .expect("first endpoint client session");
    let active_client = *active_client;
    let activation = clients[active_client]
        .request(
            "change_window_state",
            json!({
                "session_id": first_session_id,
                "task_grant_id": first_grant_id,
                "window_capability": first_capability,
                "operation": "activate"
            }),
        )
        .await;
    assert!(
        activation.is_ok()
            || matches!(&activation, Err(HostClientError::Remote { code, .. }) if code == "input_failed"),
        "unexpected activation result before physical focus: {activation:?}"
    );
    physically_focus_window(*window_handle);
    client_request(
        &mut clients[active_client],
        "escalate_session",
        json!({
            "session_id": first_session_id,
            "task_grant_id": first_grant_id,
            "window_capability": first_capability,
            "reason": "ax_tree_pixel_mismatch",
            "detail": "controlled Escape E2E permits an exact-window pixel observation"
        }),
    )
    .await;
    let escape_snapshot = client_request(
        &mut clients[active_client],
        "snapshot",
        json!({
            "session_id": first_session_id,
            "task_grant_id": first_grant_id,
            "window_capability": first_capability,
            "max_nodes": 1_000,
            "max_depth": 20,
        }),
    )
    .await;
    let pressed = client_request(
        &mut clients[active_client],
        "execute_action",
        json!({
            "session_id": first_session_id,
            "task_grant_id": first_grant_id,
            "window_capability": first_capability,
            "observation_id": escape_snapshot.value["observation_id"],
            "accessibility_state_id": escape_snapshot.value["accessibility_state_id"],
            "action": {
                "action": "keypress",
                "input_kind": "raw_input",
                "intent": "navigate",
                "delivery_mode": "foreground",
                "keys": ["ESC"]
            },
            "capture_after": false
        }),
    )
    .await;
    assert_eq!(pressed.value["success"], true, "{}", pressed.value);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let interrupted_client = 1 - active_client;
    let (_, session_id, grant_id, capability, _) = sessions
        .iter()
        .find(|(client_index, ..)| *client_index == interrupted_client)
        .expect("other endpoint client session");
    expect_user_interrupted(
        &mut clients[interrupted_client],
        json!({
            "session_id": session_id,
            "task_grant_id": grant_id,
            "window_capability": capability,
        }),
    )
    .await;

    drop(clients);
    drop(fixture_reaper);
    drop(endpoint_directory);
}
