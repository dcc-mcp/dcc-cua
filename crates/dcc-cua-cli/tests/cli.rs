use std::process::Command;

use rstest::rstest;

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
    assert!(output.stdout.is_empty());
    let receipt: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("failure should be one JSON receipt");
    assert_eq!(receipt["success"], false);
    assert_eq!(receipt["error"]["code"], "command_failed");
    assert_eq!(
        receipt["error"]["message"],
        "--pid must be greater than zero"
    );
}

#[rstest]
fn mcp_server_rejects_an_untrusted_process_parent_before_reading_stdin() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .arg("mcp-server")
        .output()
        .expect("dcc-cua should start");

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "untrusted embedding unexpectedly exposed MCP output: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("trusted embedding unavailable"),
        "unexpected rejection: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
