use std::process::Command;

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
    }
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
fn mcp_server_rejects_an_untrusted_process_parent_before_reading_stdin() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .arg("mcp-server")
        .output()
        .expect("dcc-cua should start");

    assert!(!output.status.success());
    let envelope = parse_single_json_envelope(&output.stdout);
    assert_eq!(envelope["success"], false);
    assert_eq!(envelope["error"]["code"], "command_failed");
    assert_eq!(
        envelope["error"]["message"],
        "dcc-cua could not complete the command"
    );
    assert!(output.stderr.is_empty());
}
