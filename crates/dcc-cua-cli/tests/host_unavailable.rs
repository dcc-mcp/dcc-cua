//! Exercise missing resident Hosts through the real CLI and local IPC transport.

use std::process::{Command, Output};

use rstest::rstest;

fn assert_host_not_running(output: Output) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["success"], false);
    assert_eq!(value["error"]["code"], "host_not_running");
    assert_eq!(value["error"]["hint"], "dcc-cua host-ensure");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("host-ensure"));
    assert!(message.contains("--endpoint"));
    assert!(!stdout.contains("PRIVATE_ENDPOINT"));
}

#[cfg(any(unix, windows))]
#[rstest]
#[case(&["ping"])]
#[case(&["host-call", "--method", "list_windows", "--json", "{}"])]
#[case(&["host-batch", "--json", "[]"])]
fn missing_endpoint_has_actionable_guidance(#[case] arguments: &[&str]) {
    #[cfg(unix)]
    let directory = tempfile::Builder::new()
        .prefix("cua-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(unix)]
    let endpoint = directory
        .path()
        .join("PRIVATE_ENDPOINT.sock")
        .to_string_lossy()
        .into_owned();
    #[cfg(windows)]
    let endpoint = format!(r"\\.\pipe\PRIVATE_ENDPOINT-{}", uuid::Uuid::new_v4());

    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .args(arguments)
        .args(["--endpoint", &endpoint])
        .output()
        .unwrap();
    assert_host_not_running(output);
    #[cfg(unix)]
    assert!(!std::path::Path::new(&endpoint).exists());
}

#[cfg(unix)]
#[rstest]
#[case(false)]
#[case(true)]
fn default_endpoint_missing_or_stale_has_actionable_guidance(#[case] stale: bool) {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    let directory = tempfile::Builder::new()
        .prefix("cua-")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let endpoint = directory.path().join("dcc-cua-v1.sock");
    if stale {
        drop(UnixListener::bind(&endpoint).unwrap());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .arg("ping")
        .env("XDG_RUNTIME_DIR", directory.path())
        .output()
        .unwrap();
    assert_host_not_running(output);
    assert_eq!(
        endpoint.exists(),
        stale,
        "diagnostics must not alter the socket"
    );
}

#[rstest]
fn missing_spawn_binary_is_not_reported_as_an_absent_resident_host() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .args(["ping", "--spawn"])
        .arg(directory.path().join("PRIVATE_ENDPOINT-missing-binary"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "host_transport_failed");
    assert!(value["error"].get("hint").is_none());
    assert!(!value.to_string().contains("PRIVATE_ENDPOINT"));
}

#[cfg(unix)]
#[rstest]
fn failed_handshake_is_not_reported_as_an_absent_resident_host() {
    use std::os::unix::net::UnixListener;
    use std::time::{Duration, Instant};

    let directory = tempfile::Builder::new()
        .prefix("cua-")
        .tempdir_in("/tmp")
        .unwrap();
    let endpoint = directory.path().join("PRIVATE_ENDPOINT.sock");
    let listener = UnixListener::bind(&endpoint).unwrap();
    listener.set_nonblocking(true).unwrap();
    let peer = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match listener.accept() {
                Ok((stream, _)) => return drop(stream),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "CLI did not connect to the fixture"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("fixture accept failed: {error}"),
            }
        }
    });
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .args(["ping", "--endpoint"])
        .arg(&endpoint)
        .output()
        .unwrap();
    peer.join().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(matches!(
        value["error"]["code"].as_str(),
        Some("host_transport_failed" | "host_protocol_failed")
    ));
    assert!(value["error"].get("hint").is_none());
    assert!(!value.to_string().contains("PRIVATE_ENDPOINT"));
}

#[rstest]
#[case("start_session", r#"{"session":"PRIVATE_SESSION"}"#, "invalid_request")]
#[case(
    "get_window_state",
    r#"{"pid":42,"window_id":7,"session":"PRIVATE_SESSION"}"#,
    "invalid_request"
)]
#[case("get_window_state", r#"{"session_id":"PRIVATE_SESSION","task_grant_id":"PRIVATE_GRANT","window_capability":"PRIVATE_CAP"}"#, "session_not_found")]
fn host_call_reports_namespace_and_session_errors_through_a_real_host_process(
    #[case] method: &str,
    #[case] params: &str,
    #[case] code: &str,
) {
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .args([
            "host-call",
            "--spawn",
            env!("CARGO_BIN_EXE_dcc-cua"),
            "--method",
            method,
            "--json",
            params,
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["error"]["code"], code);
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("connection")
    );
    assert!(!stdout.contains("PRIVATE_"));
}
