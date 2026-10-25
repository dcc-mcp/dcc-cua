//! In-memory transport regression: no OS child, Host, SDK or application is started.
use std::time::Duration;

use rstest::rstest;
use serde_json::Value;
use tokio::io::{AsyncWriteExt, BufReader};

use super::super::*;

fn isolated_server(path: &std::path::Path) -> TaskAuthorizationServer {
    let mut server = super::test_server();
    server.diagnostics = Some(ConnectionDiagnostics::new(
        runtime_build(),
        Some(path.into()),
    ));
    server
}

fn published(path: &std::path::Path) -> Value {
    serde_json::to_value(crate::connection_diagnostics::list_connections(Some(
        path.into(),
    )))
    .unwrap()["connections"][0]
        .clone()
}

#[rstest]
#[tokio::test]
async fn consecutive_eof_and_failed_requests_finish_cleanup_without_new_processes() {
    let temporary = tempfile::tempdir().unwrap();
    for round in 0..12 {
        let path = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join(round.to_string());
        let server = isolated_server(&path);
        let process = server.diagnostics.as_ref().unwrap().snapshot().bridge;
        assert_eq!(process.pid, std::process::id());
        // Minimal initialization, then recoverable method/JSON failures.
        let bytes = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n{invalid\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"unknown\"}\n";
        let mut input = BufReader::new(&bytes[..]);
        let mut output = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(1),
            run_connection(server, &mut input, &mut output),
        )
        .await
        .unwrap()
        .unwrap();
        let record = published(&path);
        assert_eq!(record["bridge"]["pid"], process.pid);
        assert_eq!(record["close_reason"], "stdin_eof");
        assert_eq!(record["shutdown"]["cleanup_status"], "completed");
        assert_eq!(record["shutdown"]["failed_tasks"], 0);
        assert!(record["shutdown"]["cleanup_finished_at_unix_ms"].is_u64());
        assert!(record["associated_hosts"].as_array().unwrap().is_empty());
        assert_eq!(record["tasks"]["active"], 0);
        let responses = String::from_utf8(output).unwrap();
        assert!(responses.contains("-32700") && responses.contains("-32601"));
    }
}

#[rstest]
#[tokio::test]
async fn dropped_response_transport_reports_output_error_and_finishes_cleanup() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().canonicalize().unwrap().join("connections");
    let server = isolated_server(&path);
    let (writer, reader) = tokio::io::duplex(64);
    drop(reader);
    let mut writer = writer;
    let mut input = BufReader::new(&b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n"[..]);
    assert!(
        run_connection(server, &mut input, &mut writer)
            .await
            .is_err()
    );
    let record = published(&path);
    assert_eq!(record["close_reason"], "output_error");
    assert_eq!(record["shutdown"]["cleanup_status"], "completed");
}

#[rstest]
#[tokio::test]
async fn client_timeout_preserves_open_connection_until_explicit_disconnect() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().canonicalize().unwrap().join("connections");
    let server = isolated_server(&path);
    let (mut client, stream) = tokio::io::duplex(256);
    let mut input = BufReader::new(stream);
    let mut output = Vec::new();
    let future = run_connection(server, &mut input, &mut output);
    tokio::pin!(future);
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut future)
            .await
            .is_err()
    );
    assert_eq!(published(&path)["state"], "connected");
    client.shutdown().await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), future)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(published(&path)["shutdown"]["cleanup_status"], "completed");
}

#[rstest]
#[tokio::test]
async fn cancelling_server_future_does_not_fabricate_cleanup_completion() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().canonicalize().unwrap().join("connections");
    let server = isolated_server(&path);
    let (_client, stream) = tokio::io::duplex(256);
    let mut input = BufReader::new(stream);
    let mut output = Vec::new();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(10),
            run_connection(server, &mut input, &mut output)
        )
        .await
        .is_err()
    );
    let record = published(&path);
    assert_eq!(record["close_reason"], "server_dropped");
    assert_eq!(record["shutdown"]["cleanup_status"], "unknown");
    assert!(record["shutdown"]["cleanup_finished_at_unix_ms"].is_null());
}

#[rstest]
#[tokio::test]
async fn eof_with_unknown_owned_host_cleanup_cannot_report_success() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().canonicalize().unwrap().join("connections");
    let mut server = isolated_server(&path);
    let task = server.prepare_task(super::pixels_task()).unwrap();
    let proposal = server
        .proposals
        .get_mut(task["task_id"].as_str().unwrap())
        .unwrap();
    // Model an interrupted startup without constructing or calling a native Host.
    proposal.session_open_attempted = true;
    proposal.host_task = Some(OwnedHostTask::spawn(async { Ok(()) }));
    let mut input = BufReader::new(&b""[..]);
    let mut output = Vec::new();
    assert!(
        run_connection(server, &mut input, &mut output)
            .await
            .is_err()
    );
    let record = published(&path);
    assert_eq!(record["shutdown"]["cleanup_status"], "failed");
    assert_eq!(record["shutdown"]["failed_tasks"], 1);
}
