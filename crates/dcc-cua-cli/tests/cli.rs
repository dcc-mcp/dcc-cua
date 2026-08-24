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
