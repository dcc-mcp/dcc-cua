use std::fs;

use rstest::rstest;
use serde_json::json;
use tempfile::TempDir;

use super::*;

fn build() -> RuntimeBuildIdentity {
    RuntimeBuildIdentity {
        runtime_version: "test-version".into(),
        source_revision: Some("0123456789abcdef".into()),
        source_dirty: Some(false),
        build_profile: Some("test".into()),
        target: Some("test-target".into()),
    }
}

fn directory() -> (TempDir, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().canonicalize().unwrap().join("connections");
    (temporary, path)
}

#[rstest]
fn captures_actual_process_instance_and_embedded_host_identity() {
    let (_temporary, path) = directory();
    let diagnostics = ConnectionDiagnostics::new(build(), Some(path));
    let record = diagnostics.snapshot();
    assert_eq!(record.bridge.pid, std::process::id());
    assert_eq!(
        record.runtime.source_revision.as_deref(),
        Some("0123456789abcdef")
    );
    #[cfg(any(windows, target_os = "linux"))]
    {
        assert!(record.bridge.creation_id.is_some());
        assert!(record.bridge.creation_time_unix_ms.is_some());
        assert_eq!(record.process_status, ProcessStatus::Live);
    }
    assert_eq!(
        AssociatedHost::embedded(Some("connection-1".into())).process,
        record.bridge
    );
}

#[rstest]
fn initialize_records_only_allowlisted_client_metadata_and_binds_once() {
    let (_temporary, path) = directory();
    let mut diagnostics = ConnectionDiagnostics::new(build(), Some(path.clone()));
    diagnostics.initialized(&json!({
        "clientInfo": {"name":"Codex Desktop", "version":"1.2.3", "token":"do-not-copy"},
        "capabilities": {"credentials":"do-not-copy"},
        "_meta": {"dcc-cua": {"chat_id":"chat-123", "task_id":"task:456", "secret":"do-not-copy"}}
    }));
    let record = diagnostics.snapshot();
    assert_eq!(record.state, ConnectionState::Initialized);
    assert_eq!(
        record.client_info.unwrap().name.as_deref(),
        Some("Codex Desktop")
    );
    assert_eq!(record.client_metadata.chat_id.as_deref(), Some("chat-123"));
    assert_eq!(record.client_metadata.source, "client_supplied");
    let text = serde_json::to_string(&list_connections(Some(path))).unwrap();
    assert!(!text.contains("do-not-copy"));
    assert!(
        text.contains("initialized"),
        "first initialize must be published even in the startup millisecond"
    );
    diagnostics.initialized(&json!({"clientInfo":{"name":"Substitution"}}));
    assert_eq!(
        diagnostics.snapshot().client_info.unwrap().name.as_deref(),
        Some("Codex Desktop")
    );
}

#[rstest]
fn malformed_metadata_stays_unknown_and_does_not_copy_urls_or_controls() {
    let (_temporary, path) = directory();
    let mut diagnostics = ConnectionDiagnostics::new(build(), Some(path));
    diagnostics.initialized(&json!({
        "clientInfo": {"name":"https://user:password@example.test", "version":"1\nsecret"},
        "_meta": {"dcc-cua": {"chat_id":"https://example.test/chat", "task_id":42}}
    }));
    let record = diagnostics.snapshot();
    assert!(record.client_info.is_none());
    assert!(record.client_metadata.chat_id.is_none());
    assert_eq!(record.client_metadata.source, "unknown");
}

#[rstest]
fn unavailable_registry_preserves_current_resource_without_touching_transport() {
    let temporary = tempfile::tempdir().unwrap();
    let file = temporary.path().join("not-a-directory");
    fs::write(&file, b"untouched").unwrap();
    let mut diagnostics = ConnectionDiagnostics::new(build(), Some(file.clone()));
    diagnostics.initialized(&json!({"clientInfo":{"name":"test"}}));
    diagnostics.set_request_in_flight(true);
    let record = diagnostics.snapshot();
    assert!(!record.registry_available);
    assert_eq!(record.state, ConnectionState::Initialized);
    assert!(record.request_in_flight);
    assert_eq!(fs::read(file).unwrap(), b"untouched");
}

#[rstest]
fn querying_absent_registry_is_read_only_and_returns_an_explicit_limit() {
    let (_temporary, path) = directory();
    let report = list_connections(Some(path.clone()));
    assert!(!report.registry_available);
    assert!(report.connections.is_empty());
    assert!(!path.exists());
    assert!(
        report
            .limitations
            .iter()
            .any(|limit| limit.contains("older bridges"))
    );
}

#[rstest]
fn close_is_observed_idempotent_and_clears_in_flight() {
    let (_temporary, path) = directory();
    let mut diagnostics = ConnectionDiagnostics::new(build(), Some(path.clone()));
    diagnostics.set_request_in_flight(true);
    diagnostics.close(CloseReason::StdinEof);
    let first_closed = diagnostics.snapshot().closed_at_unix_ms;
    diagnostics.close(CloseReason::OutputError);
    let record = list_connections(Some(path)).connections.remove(0);
    assert_eq!(record.state, ConnectionState::Closed);
    assert_eq!(record.close_reason, Some(CloseReason::StdinEof));
    assert_eq!(record.close_reason_source.as_deref(), Some("observed"));
    assert_eq!(record.closed_at_unix_ms, first_closed);
    assert!(!record.request_in_flight);
}

#[rstest]
fn reused_pid_is_ended_without_fabricating_stdin_eof_or_exit_time() {
    let (_temporary, path) = directory();
    let diagnostics = ConnectionDiagnostics::new(build(), Some(path.clone()));
    let mut record = diagnostics.snapshot();
    record.bridge.creation_id = Some("previous-process-instance".into());
    registry::publish(&path, &record).unwrap();
    let observed = list_connections(Some(path)).connections.remove(0);
    #[cfg(any(windows, target_os = "linux"))]
    {
        assert_eq!(observed.process_status, ProcessStatus::Ended);
        assert_eq!(observed.state, ConnectionState::Closed);
        assert_eq!(observed.close_reason, Some(CloseReason::ProcessExited));
        assert_eq!(observed.close_reason_source.as_deref(), Some("inferred"));
        assert!(observed.closed_at_unix_ms.is_none());
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    assert_eq!(observed.process_status, ProcessStatus::Unknown);
}

#[rstest]
fn missing_creation_identity_is_unknown_even_for_an_existing_pid() {
    let process = ProcessIdentity {
        pid: std::process::id(),
        creation_time_unix_ms: None,
        creation_id: None,
    };
    assert_eq!(process_identity::status(&process), ProcessStatus::Unknown);
}

#[rstest]
fn parent_pid_reuse_does_not_attach_a_newer_process_identity() {
    let child = ProcessIdentity {
        pid: 1,
        creation_time_unix_ms: Some(100),
        creation_id: Some("child".into()),
    };
    let parent = ProcessIdentity {
        pid: 2,
        creation_time_unix_ms: Some(101),
        creation_id: Some("reused-parent".into()),
    };
    let fenced = process_identity::fence_parent_identity(parent, &child);
    assert_eq!(fenced.pid, 2);
    assert!(fenced.creation_id.is_none());
    assert!(fenced.creation_time_unix_ms.is_none());
}

#[rstest]
fn bounded_reader_ignores_foreign_malformed_and_oversized_files() {
    let (_temporary, path) = directory();
    let diagnostics = ConnectionDiagnostics::new(build(), Some(path.clone()));
    fs::write(path.join("foreign.json"), b"sensitive arbitrary content").unwrap();
    fs::write(
        path.join(format!("mcp-connection-{}.json", Uuid::new_v4())),
        b"invalid JSON",
    )
    .unwrap();
    fs::write(
        path.join(format!("mcp-connection-{}.json", Uuid::new_v4())),
        vec![b' '; 65 * 1024],
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for entry in fs::read_dir(&path).unwrap() {
            fs::set_permissions(entry.unwrap().path(), fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    let report = list_connections(Some(path));
    assert_eq!(report.connections.len(), 1);
    assert_eq!(
        report.connections[0].connection_id,
        diagnostics.snapshot().connection_id
    );
}

#[rstest]
fn report_limit_does_not_disable_terminal_retention() {
    let (_temporary, path) = directory();
    let diagnostics = ConnectionDiagnostics::new(build(), Some(path.clone()));
    let mut record = diagnostics.snapshot();
    record.state = ConnectionState::Closed;
    record.close_reason = Some(CloseReason::StdinEof);
    for index in 0..300 {
        record.connection_id = format!("mcp-connection-{}", Uuid::new_v4());
        record.closed_at_unix_ms = Some(index);
        registry::publish(&path, &record).unwrap();
    }
    let report = list_connections(Some(path.clone()));
    assert!(report.scan_truncated);
    assert_eq!(report.connections.len(), 256);
    registry::retain_recent_terminal_records(&path);
    let report = list_connections(Some(path));
    assert!(!report.scan_truncated);
    assert_eq!(
        report.connections.len(),
        129,
        "128 terminal records plus this valid live connection"
    );
}

#[rstest]
fn atomic_updates_do_not_expose_partial_records_to_concurrent_readers() {
    let (_temporary, path) = directory();
    let mut diagnostics = ConnectionDiagnostics::new(build(), Some(path.clone()));
    let reader = std::thread::spawn(move || {
        for _ in 0..50 {
            let report = list_connections(Some(path.clone()));
            assert!(report.registry_available);
            assert_eq!(report.connections.len(), 1);
        }
    });
    for _ in 0..50 {
        diagnostics.set_request_in_flight(true);
        diagnostics.set_request_in_flight(false);
    }
    reader.join().unwrap();
}

#[cfg(unix)]
#[rstest]
fn registry_requires_private_owned_directory_and_refuses_links() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temporary = tempfile::tempdir().unwrap();
    let public = temporary.path().join("public");
    fs::create_dir(&public).unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    let diagnostics = ConnectionDiagnostics::new(build(), Some(public));
    assert!(!diagnostics.snapshot().registry_available);
    let destination = temporary.path().join("private");
    let linked = temporary.path().join("linked");
    fs::create_dir(&destination).unwrap();
    symlink(&destination, &linked).unwrap();
    let diagnostics = ConnectionDiagnostics::new(build(), Some(linked));
    assert!(!diagnostics.snapshot().registry_available);
    assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
}
