use std::path::Path;
use std::process::Output;

use super::*;
use rstest::rstest;

#[derive(Clone, Debug)]
struct CliWindowTarget {
    app: String,
    pid: u32,
    window_id: u64,
    title: String,
}

impl CliWindowTarget {
    fn selector_arguments(&self) -> Vec<String> {
        vec![
            "--app".into(),
            self.app.to_uppercase(),
            "--pid".into(),
            self.pid.to_string(),
            "--window-id".into(),
            self.window_id.to_string(),
            "--title".into(),
            self.title.clone(),
        ]
    }
}

fn run_cli(binary: &Path, command: &str, arguments: &[String]) -> Output {
    Command::new(binary)
        .arg(command)
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("run external dcc-cua {command}: {error}"))
}

fn successful_json(binary: &Path, command: &str, arguments: &[String]) -> Value {
    let output = run_cli(binary, command, arguments);
    assert!(
        output.status.success(),
        "external dcc-cua {command} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("parse external dcc-cua {command} JSON: {error}"))
}

fn assert_selector_failure(binary: &Path, command: &str, arguments: &[String]) {
    let output = run_cli(binary, command, arguments);
    assert!(
        !output.status.success(),
        "{command} accepted a wrong identity"
    );
    let stdout = std::str::from_utf8(&output.stdout)
        .unwrap_or_else(|error| panic!("{command} failure was not UTF-8: {error}"));
    assert_eq!(
        stdout.lines().count(),
        1,
        "{command} failure was not exactly one JSON envelope: {stdout:?}"
    );
    let receipt: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|error| panic!("{command} failure was not one JSON receipt: {error}"));
    assert_eq!(receipt["success"], false, "{receipt}");
    assert_eq!(receipt["error"]["code"], "command_failed", "{receipt}");
    assert_eq!(
        receipt["error"]["message"], "dcc-cua could not complete the command",
        "{receipt}"
    );
    assert!(
        output.stderr.is_empty(),
        "{command} leaked diagnostics while rejecting a wrong identity: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture_text(path: &Path, id: &str) -> Option<String> {
    std::fs::read(path)
        .ok()
        .and_then(|body| serde_json::from_slice::<Value>(&body).ok())
        .and_then(|state| state[id]["text"].as_str().map(str::to_owned))
}

fn wait_for_fixture_text(path: &Path, id: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(text) = fixture_text(path, id) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "fixture state {id:?} did not become available"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn find_element_index(value: &Value, marker: &str) -> Option<u64> {
    match value {
        Value::Object(object) => {
            let contains_marker = object
                .values()
                .any(|value| value.as_str().is_some_and(|text| text.contains(marker)));
            if contains_marker
                && let Some(index) = object["element_index"]
                    .as_u64()
                    .or_else(|| object["index"].as_u64())
            {
                return Some(index);
            }
            object
                .values()
                .find_map(|value| find_element_index(value, marker))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|value| find_element_index(value, marker)),
        _ => None,
    }
}

fn value_contains_exact_string(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(value) => value == expected,
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_exact_string(value, expected)),
        Value::Object(object) => object
            .values()
            .any(|value| value_contains_exact_string(value, expected)),
        _ => false,
    }
}

unsafe extern "system" fn find_process_window(
    window: windows_sys::Win32::Foundation::HWND,
    state: windows_sys::Win32::Foundation::LPARAM,
) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindowVisible};

    let state = unsafe { &mut *(state as *mut (u32, usize)) };
    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(window, &mut process_id) };
    if process_id == state.0 && unsafe { IsWindowVisible(window) } != 0 {
        state.1 = window as usize;
        return 0;
    }
    1
}

fn wait_for_native_window(process_id: u32) -> u64 {
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let mut state = (process_id, 0usize);
        unsafe { EnumWindows(Some(find_process_window), &raw mut state as isize) };
        if state.1 != 0 {
            return state.1 as u64;
        }
        assert!(
            Instant::now() < deadline,
            "fixture process {process_id} did not create a visible window"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn inventory_target(binary: &Path, pid: u32) -> CliWindowTarget {
    let inventory = successful_json(
        binary,
        "list",
        &["--pid".into(), pid.to_string(), "--on-screen".into()],
    );
    let row = inventory
        .as_array()
        .and_then(|rows| rows.first())
        .unwrap_or_else(|| {
            panic!("external CLI returned no window for fixture PID {pid}: {inventory}")
        });
    CliWindowTarget {
        app: row["app_name"].as_str().expect("fixture app name").into(),
        pid,
        window_id: row["window_id"].as_u64().expect("fixture window id"),
        title: row["title"].as_str().expect("fixture title").into(),
    }
}

#[rstest]
#[tokio::test]
async fn external_cli_keeps_exact_identity_across_act_verify_and_clipboard() {
    let binary = std::env::var_os("DCC_CUA_E2E_BINARY")
        .map(PathBuf::from)
        .expect("DCC_CUA_E2E_BINARY must point to dcc-cua");
    let fixture_path = wpf_fixture();
    let mut fixture_reaper = ChildReaper::new();
    let mut fixtures = Vec::new();
    for index in 0..2 {
        let state_dir = tempfile::tempdir().expect("create external CLI fixture state directory");
        let state_path = state_dir.path().join(format!("cli-state-{index}.json"));
        let mut command = Command::new(&fixture_path);
        command
            .env("CUA_E2E_FIXTURE_STATE_PATH", &state_path)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        let child = spawn_in_job(&mut command).expect("launch external CLI WPF fixture");
        let pid = child.id();
        fixture_reaper.push(child);
        wait_for_fixture_file(&state_path, "page-marker", "WPF_HARNESS_MARKER_v1");
        let window_id = wait_for_native_window(pid);
        fixtures.push((pid, window_id, state_path, state_dir));
    }

    eprintln!(
        "provider=dcc-cua runtime={} target_pid={} target_hwnd={} decoy_pid={} decoy_hwnd={}",
        env!("CARGO_PKG_VERSION"),
        fixtures[0].0,
        fixtures[0].1,
        fixtures[1].0,
        fixtures[1].1
    );
    if let Ok(delay_ms) = std::env::var("DCC_CUA_EXACT_SELECTOR_ATTEST_DELAY_MS")
        && let Ok(delay_ms) = delay_ms.parse::<u64>()
    {
        std::thread::sleep(Duration::from_millis(delay_ms.min(30_000)));
    }

    let target = inventory_target(&binary, fixtures[0].0);
    let decoy = inventory_target(&binary, fixtures[1].0);
    assert_eq!(target.window_id, fixtures[0].1);
    assert_eq!(decoy.window_id, fixtures[1].1);
    assert_eq!(target.app.to_lowercase(), decoy.app.to_lowercase());
    let decoy_before = wait_for_fixture_text(&fixtures[1].2, "lbl-click-count");

    let accessibility = successful_json(&binary, "accessibility", &target.selector_arguments());
    let element_index =
        find_element_index(&accessibility, "border-click-target").unwrap_or_else(|| {
            panic!("external accessibility output omitted border-click-target: {accessibility}")
        });
    let mut act_arguments = target.selector_arguments();
    act_arguments.extend([
        "--action-json".into(),
        json!({
            "action": "click",
            "input_kind": "semantic",
            "intent": "navigate",
            "delivery_mode": "background",
            "element_index": element_index
        })
        .to_string(),
    ]);
    let acted = successful_json(&binary, "act", &act_arguments);
    assert_eq!(acted["success"], true, "{acted}");
    assert_eq!(acted["post_snapshot"]["success"], true, "{acted}");
    wait_for_fixture_file(&fixtures[0].2, "lbl-click-count", "clicks=1");
    assert_eq!(
        fixture_text(&fixtures[1].2, "lbl-click-count").as_deref(),
        Some(decoy_before.as_str()),
        "the decoy fixture changed after an exact-bound action"
    );

    let mut verify_arguments = target.selector_arguments();
    verify_arguments.extend([
        "--expect-json".into(),
        r#"[{"window":{"exists":true}}]"#.into(),
    ]);
    let verified = successful_json(&binary, "verify", &verify_arguments);
    assert_eq!(
        verified["structuredContent"]["status"], "satisfied",
        "{verified}"
    );

    let wrong_selectors = [
        CliWindowTarget {
            pid: decoy.pid,
            ..target.clone()
        },
        CliWindowTarget {
            window_id: decoy.window_id,
            ..target.clone()
        },
        CliWindowTarget {
            title: format!("{}-drifted", target.title),
            ..target.clone()
        },
    ];
    for wrong in &wrong_selectors {
        let mut wrong_act = wrong.selector_arguments();
        wrong_act.extend([
            "--action-json".into(),
            json!({
                "action": "click",
                "input_kind": "semantic",
                "intent": "navigate",
                "delivery_mode": "background",
                "element_index": element_index
            })
            .to_string(),
        ]);
        assert_selector_failure(&binary, "act", &wrong_act);

        let mut wrong_read = wrong.selector_arguments();
        wrong_read.push("--include-text".into());
        assert_selector_failure(&binary, "clipboard-read", &wrong_read);
    }
    assert_eq!(
        fixture_text(&fixtures[0].2, "lbl-click-count").as_deref(),
        Some("clicks=1")
    );
    assert_eq!(
        fixture_text(&fixtures[1].2, "lbl-click-count").as_deref(),
        Some(decoy_before.as_str())
    );

    let clipboard_metadata =
        successful_json(&binary, "clipboard-read", &target.selector_arguments());
    assert!(clipboard_metadata.is_object(), "{clipboard_metadata}");
    if std::env::var_os("DCC_CUA_E2E_CLIPBOARD_WRITE").is_some() {
        let clipboard_text = format!("dcc-cua-exact-selector-{}", target.pid);
        let mut write_arguments = target.selector_arguments();
        write_arguments.extend(["--text".into(), clipboard_text.clone()]);
        let written = successful_json(&binary, "clipboard-write", &write_arguments);
        assert!(written.is_object(), "{written}");

        let mut read_arguments = target.selector_arguments();
        read_arguments.push("--include-text".into());
        let read_back = successful_json(&binary, "clipboard-read", &read_arguments);
        assert!(
            value_contains_exact_string(&read_back, &clipboard_text),
            "clipboard write had no value postcondition: {read_back}"
        );
    }

    drop(fixture_reaper);
}
