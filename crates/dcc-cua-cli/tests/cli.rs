use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rstest::rstest;

fn parse_single_json_envelope(bytes: &[u8]) -> serde_json::Value {
    let stdout = std::str::from_utf8(bytes).expect("stdout should be UTF-8");
    assert_eq!(
        stdout.lines().count(),
        1,
        "stdout should contain exactly one JSON envelope: {stdout:?}"
    );
    serde_json::from_str(stdout.trim()).expect("stdout should contain valid JSON")
}

fn compile_hostile_host(output_directory: &Path) -> PathBuf {
    let binary_name = if cfg!(windows) {
        "hostile-host.exe"
    } else {
        "hostile-host"
    };
    let binary_path = output_directory.join(binary_name);
    let source_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("scripts")
        .join("fixtures")
        .join("hostile_host.rs");
    let output = Command::new("rustc")
        .arg("--edition=2024")
        .arg("-o")
        .arg(&binary_path)
        .arg(source_path)
        .output()
        .expect("rustc should compile the hostile Host fixture");
    assert!(
        output.status.success(),
        "hostile Host fixture compilation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary_path
}

#[rstest]
fn version_flag_reports_cli_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .arg("--version")
        .output()
        .expect("dcc-cua should start");

    assert!(
        output.status.success(),
        "--version failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("version output should be UTF-8"),
        format!("dcc-cua {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[rstest]
fn version_aliases_match_the_long_flag() {
    for argument in ["-V", "version"] {
        let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
            .arg(argument)
            .output()
            .expect("dcc-cua should start");
        assert!(
            output.status.success(),
            "{argument} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).expect("version output should be UTF-8"),
            format!("dcc-cua {}\n", env!("CARGO_PKG_VERSION"))
        );
        assert!(output.stderr.is_empty());
    }
}

#[rstest]
fn help_routes_remain_successful_and_do_not_write_diagnostics() {
    for arguments in [
        &[][..],
        &["--help"][..],
        &["snapshot", "--help"][..],
        &["accessibility", "--help"][..],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
            .args(arguments)
            .output()
            .expect("dcc-cua should start");
        assert!(output.status.success());
        let stdout = std::str::from_utf8(&output.stdout).expect("help output should be UTF-8");
        assert!(stdout.contains("host-batch"));
        assert!(stdout.contains("no_accessibility_provider"));
        assert!(stdout.contains("snapshot --pixels-only"));
        assert!(stdout.contains("OCR or another perception layer"));
        assert!(stdout.contains("Coordinate actions require --observation-width"));
        assert!(stdout.contains("screen_x = bounds.x + x * bounds.width / observation_width"));
        assert!(stdout.contains("Do not apply screen-size scale_factor"));
        assert!(output.stderr.is_empty());
    }
}

#[rstest]
fn profiles_default_to_usable_entries_and_offer_an_explicit_invalid_diagnostic_view() {
    let store = tempfile::tempdir().expect("Profile store");
    std::fs::create_dir(store.path().join("artstation")).expect("invalid Profile directory");

    let run = |state: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dcc-cua"));
        command
            .arg("profiles")
            .arg("--profile-store")
            .arg(store.path());
        if let Some(state) = state {
            command.args(["--state", state]);
        }
        command.output().expect("dcc-cua profiles should start")
    };

    let default = run(None);
    assert!(default.status.success());
    let default_profiles: serde_json::Value =
        serde_json::from_slice(&default.stdout).expect("default Profile listing");
    assert!(
        default_profiles
            .as_array()
            .expect("Profile array")
            .iter()
            .all(|profile| profile["status"] == "ready")
    );
    assert!(
        !default_profiles
            .as_array()
            .expect("Profile array")
            .iter()
            .any(|profile| profile["id"] == "artstation")
    );

    let invalid = run(Some("invalid"));
    assert!(invalid.status.success());
    let invalid_profiles: serde_json::Value =
        serde_json::from_slice(&invalid.stdout).expect("invalid Profile listing");
    assert_eq!(invalid_profiles.as_array().expect("Profile array").len(), 1);
    assert_eq!(invalid_profiles[0]["id"], "artstation");
    assert_eq!(invalid_profiles[0]["status"], "invalid");
    assert!(
        invalid_profiles[0]["error"]
            .as_str()
            .expect("invalid reason")
            .contains("profile-package.json")
    );
    assert!(invalid.stderr.is_empty());

    let all = run(Some("all"));
    assert!(all.status.success());
    let all_profiles: serde_json::Value =
        serde_json::from_slice(&all.stdout).expect("complete Profile listing");
    assert!(
        all_profiles
            .as_array()
            .expect("Profile array")
            .iter()
            .any(|profile| profile["id"] == "artstation")
    );
}

#[rstest]
fn exact_window_commands_advertise_the_snapshot_selector_contract() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .arg("help")
        .output()
        .expect("dcc-cua should start");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help output should be UTF-8");
    for command in ["act", "verify", "clipboard-read", "clipboard-write"] {
        let synopsis = stdout
            .lines()
            .find(|line| line.trim_start().starts_with(command))
            .unwrap_or_else(|| panic!("help omitted {command}"));
        for selector in ["--app", "--pid", "--window-id", "--title"] {
            assert!(
                synopsis.contains(selector),
                "{command} does not advertise {selector}: {synopsis}"
            );
        }
    }
}

#[rstest]
#[case("act", &["--action-json", r#"{"action":"click","x":1,"y":1}"#])]
#[case("verify", &["--expect-json", "[]"])]
#[case("clipboard-read", &[])]
#[case("clipboard-write", &["--text", "bounded text"])]
fn exact_window_commands_share_fail_closed_selector_validation(
    #[case] command: &str,
    #[case] required_arguments: &[&str],
) {
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .arg(command)
        .args(["--pid", "0"])
        .args(required_arguments)
        .output()
        .expect("dcc-cua should start");

    assert!(!output.status.success());
    let receipt = parse_single_json_envelope(&output.stdout);
    assert_eq!(receipt["success"], false);
    assert_eq!(receipt["error"]["code"], "command_failed");
    assert_eq!(
        receipt["error"]["message"],
        "dcc-cua could not complete the command"
    );
    assert!(output.stderr.is_empty());
}

#[rstest]
#[case("list", &["--pid"])]
#[case("list", &["--pid", "42", "--pid"])]
#[case(
    "act",
    &["--pid", "--window-id", "77", "--action-json", r#"{"action":"click","x":1,"y":1}"#]
)]
#[case("verify", &["--pid", "--window-id", "77", "--expect-json", "[]"])]
#[case("clipboard-read", &["--pid", "--window-id", "77"])]
#[case(
    "clipboard-write",
    &["--pid", "--window-id", "77", "--text", "bounded text"]
)]
fn malformed_selectors_fail_before_inventory_or_scoped_access(
    #[case] command: &str,
    #[case] arguments: &[&str],
) {
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .arg(command)
        .args(arguments)
        .output()
        .expect("dcc-cua should start");

    assert!(!output.status.success());
    let receipt = parse_single_json_envelope(&output.stdout);
    assert_eq!(receipt["success"], false);
    assert_eq!(receipt["error"]["code"], "command_failed");
    assert_eq!(
        receipt["error"]["message"],
        "dcc-cua could not complete the command"
    );
    assert!(output.stderr.is_empty());
}

#[rstest]
fn ordinary_command_failures_emit_one_machine_envelope_on_stdout() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .arg("definitely-not-a-command")
        .output()
        .expect("dcc-cua should start");

    assert_eq!(output.status.code(), Some(1));
    let envelope = parse_single_json_envelope(&output.stdout);
    assert_eq!(envelope["success"], false);
    assert_eq!(envelope["error"]["code"], "command_failed");
    assert!(
        output.stderr.is_empty(),
        "structured command failure should not be duplicated on stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[rstest]
#[case("REVIEW_PRIVATE_ARGUMENT_8e1ab4", &[])]
#[case("snapshot", &["--REVIEW_PRIVATE_OPTION_351cc7"])]
fn rejected_cli_syntax_does_not_echo_untrusted_arguments(
    #[case] command: &str,
    #[case] arguments: &[&str],
) {
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .arg(command)
        .args(arguments)
        .output()
        .expect("dcc-cua should start");

    assert_eq!(output.status.code(), Some(1));
    let envelope = parse_single_json_envelope(&output.stdout);
    assert_eq!(envelope["error"]["code"], "command_failed");
    assert_eq!(
        envelope["error"]["message"],
        "dcc-cua could not complete the command"
    );
    let stdout = std::str::from_utf8(&output.stdout).expect("stdout should be UTF-8");
    assert!(
        !stdout.contains("REVIEW_PRIVATE_"),
        "untrusted CLI input leaked: {stdout:?}"
    );
    assert!(output.stderr.is_empty());
}

#[rstest]
#[case("Codex Desktop")]
#[case("Codex Cloud")]
#[case("Codex CLI")]
#[case("Cursor")]
#[case("WorkBuddy")]
#[case("CodeBuddy CLI")]
fn mcp_server_reports_missing_human_integration_without_creating_authority(#[case] client: &str) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .arg("mcp-server")
        .env("DCC_CUA_TRUSTED_EMBEDDING", client)
        .env("DCC_CUA_AUTHORIZATION", "AUTHORIZE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("dcc-cua should start");
    let mut input = child.stdin.take().unwrap();
    let requests = [
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2024-11-05","clientInfo":{"name":client,"version":"1"}}}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
            "name":"authorization_integration_status","arguments":{}}}),
        serde_json::json!({"jsonrpc":"2.0","id":4,"method":"resources/list"}),
    ];
    for request in requests {
        writeln!(input, "{request}").unwrap();
    }
    for (index, name) in [
        "prepare_task_authorization",
        "authorize_task",
        "start_authorized_task",
        "dcc_cua_task_call",
        "revoke_task_authorization",
    ]
    .into_iter()
    .enumerate()
    {
        let request = serde_json::json!({"jsonrpc":"2.0","id":index+5,"method":"tools/call","params":{
            "name":name,"arguments":{"acknowledgement":"AUTHORIZE","proposal_id":"forged",
                "signature":"forged","task_grant_id":"forged","trusted_embedding":client}}});
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let responses: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 9);
    assert_eq!(
        responses[0]["result"]["serverInfo"]["name"],
        "dcc-cua-task-authorization"
    );
    let tools = responses[1]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "authorization_integration_status");
    assert!(tools[0].get("_meta").is_none());
    let status = &responses[2]["result"]["structuredContent"];
    assert_eq!(status["status"], "integration_required");
    assert_eq!(status["authorization_available"], false);
    assert_eq!(status["user_confirmation_available"], false);
    assert_eq!(status["provider"], "dcc-cua");
    assert_eq!(
        status["next_owners"],
        serde_json::json!(["dcc_cua_core", "client_embedding_integration"])
    );
    assert_eq!(
        status["signed_receipt_protocol"]["runtime_accepts_receipts"],
        false
    );
    assert_eq!(responses[3]["result"]["resources"], serde_json::json!([]));
    for response in &responses[4..] {
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(
            response["result"]["structuredContent"]["code"],
            "integration_required"
        );
        assert!(response["result"].get("_meta").is_none());
        assert!(!response.to_string().contains("forged"));
    }
}

#[rstest]
fn private_worker_failure_stays_on_its_protocol_native_boundary() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .args(["__private-worker", "--generation", "dynamic_marker_8e1ab4"])
        .output()
        .expect("dcc-cua private-worker fixture should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "private-worker failure appended non-protocol stdout bytes: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(
        stderr.is_empty() || stderr == "dcc-cua: private worker failed\n",
        "private-worker failure exposed dynamic stderr: {stderr:?}"
    );
    assert!(
        !stderr.contains("dynamic_marker_8e1ab4"),
        "private-worker stderr exposed the fixture marker: {stderr:?}"
    );
}

#[rstest]
fn mcp_server_rejects_oversized_input_without_an_unframed_error() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .arg("mcp-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(&vec![b' '; dcc_cua_protocol::MAX_JSON_FRAME_BYTES + 1])
        .unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

fn compile_failing_jsonl_host(output_directory: &Path) -> PathBuf {
    let binary_name = if cfg!(windows) {
        "failing-jsonl-host.exe"
    } else {
        "failing-jsonl-host"
    };
    let binary_path = output_directory.join(binary_name);
    let source_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("scripts/fixtures/failing_jsonl_host.rs");
    let output = Command::new("rustc")
        .arg("--edition=2024")
        .arg("-o")
        .arg(&binary_path)
        .arg(source_path)
        .output()
        .expect("rustc should compile the failing Host fixture");
    assert!(
        output.status.success(),
        "failing Host fixture compilation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary_path
}

fn compile_doctor_shutdown_failure_host(output_directory: &Path) -> PathBuf {
    let binary_name = if cfg!(windows) {
        "doctor-shutdown-failure-host.exe"
    } else {
        "doctor-shutdown-failure-host"
    };
    let binary_path = output_directory.join(binary_name);
    let source_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("scripts/fixtures/doctor_shutdown_failure_host.rs");
    let output = Command::new("rustc")
        .arg("--edition=2024")
        .arg("-o")
        .arg(&binary_path)
        .arg(source_path)
        .output()
        .expect("rustc should compile the doctor shutdown failure Host fixture");
    assert!(
        output.status.success(),
        "doctor shutdown failure Host fixture compilation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary_path
}

#[rstest]
fn host_doctor_publishes_diagnostics_before_shutdown_failure() {
    let fixture_directory = tempfile::tempdir().expect("create Host fixture directory");
    let fixture = compile_doctor_shutdown_failure_host(fixture_directory.path());
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .env("DCC_CUA_NO_UPDATE_CHECK", "1")
        .args(["doctor", "--spawn"])
        .arg(fixture)
        .output()
        .expect("dcc-cua doctor should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stderr.is_empty(),
        "doctor shutdown failure escaped to stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let diagnostics: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("stdout should contain exactly one diagnostics document and no trailer");
    assert_eq!(diagnostics["type"], "diagnostics");
    assert_eq!(diagnostics["ready"], false);
    assert!(diagnostics.get("success").is_none());
}

#[rstest]
#[case(&["chrome-extension://abcdefghijklmnop/"])]
#[case(&["native-host.json", "chrome-extension://abcdefghijklmnop/"])]
fn truncated_native_messaging_frame_never_emits_unframed_json(#[case] arguments: &[&str]) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("dcc-cua native host should start");
    child
        .stdin
        .take()
        .expect("native host stdin")
        .write_all(&[1, 0])
        .expect("write truncated native message prefix");
    let output = child.wait_with_output().expect("native host should exit");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "native terminal failure appended non-framed bytes: {:02X?}",
        output.stdout
    );
    assert!(output.stderr.is_empty());
}

#[rstest]
fn host_jsonl_argument_failure_never_emits_a_one_shot_envelope() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .args(["host-jsonl", "--REVIEW_PROTOCOL_PRIVATE_OPTION"])
        .output()
        .expect("dcc-cua should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[rstest]
fn midstream_host_jsonl_failure_has_no_one_shot_trailer() {
    let fixture_directory = tempfile::tempdir().expect("create Host fixture directory");
    let fixture = compile_failing_jsonl_host(fixture_directory.path());
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .args(["host-jsonl", "--spawn"])
        .arg(fixture)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("dcc-cua host-jsonl should start");
    child
        .stdin
        .take()
        .expect("host-jsonl stdin")
        .write_all(
            b"{\"request_id\":\"first\",\"method\":\"ping\",\"params\":{}}\n{\"request_id\":\"second\",\"method\":\"ping\",\"params\":{}}\n",
        )
        .expect("write two host-jsonl requests");
    let output = child.wait_with_output().expect("host-jsonl should exit");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let lines = std::str::from_utf8(&output.stdout)
        .expect("host-jsonl stdout should be UTF-8")
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSONL response"))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "unexpected protocol trailer: {lines:?}");
    assert_eq!(lines[0]["type"], "pong");
    assert_eq!(lines[0]["request_id"], "first");
    assert_eq!(lines[1]["type"], "error");
    assert!(lines[1].get("success").is_none());
}

#[rstest]
fn spawned_host_diagnostics_do_not_escape_the_structured_output_boundary() {
    let fixture_directory = tempfile::tempdir().expect("create Host fixture directory");
    let hostile_host = compile_hostile_host(fixture_directory.path());
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .env("DCC_CUA_NO_UPDATE_CHECK", "1")
        .args(["host-call", "--spawn"])
        .arg(hostile_host)
        .args(["--method", "ping"])
        .output()
        .expect("dcc-cua should start");

    assert_eq!(output.status.code(), Some(1));
    let envelope = parse_single_json_envelope(&output.stdout);
    assert_eq!(envelope["success"], false);
    assert_eq!(envelope["error"]["code"], "host_protocol_failed");
    assert_eq!(
        envelope["error"]["message"],
        "dcc-cua could not complete the command"
    );
    assert!(
        output.stderr.is_empty(),
        "spawned Host diagnostics escaped to public stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
