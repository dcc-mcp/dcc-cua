use crate::extension_transport::endpoint;
use rstest::rstest;

#[rstest]
fn endpoint_is_derived_only_from_a_connection_uuid() {
    assert!(endpoint("../../other.sock").is_none());
    assert!(endpoint("mcp-connection-invalid").is_none());
    let id = format!("mcp-connection-{}", uuid::Uuid::new_v4());
    assert_eq!(endpoint(&id), endpoint(&id));
    assert_ne!(
        endpoint(&id),
        endpoint(&format!("mcp-connection-{}", uuid::Uuid::new_v4()))
    );
}

#[rstest]
fn distinct_connection_uuids_do_not_alias_under_case_folding() {
    let first = endpoint("mcp-connection-00000000-0000-4000-8000-000000000000").unwrap();
    let second = endpoint("mcp-connection-68000000-0000-4000-8000-000000000000").unwrap();
    assert_ne!(first.to_ascii_lowercase(), second.to_ascii_lowercase());
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[rstest]
fn unix_endpoint_preserves_uuid_and_refuses_native_path_overflow() {
    use std::path::Path;

    let id = uuid::Uuid::new_v4();
    let endpoint =
        crate::extension_transport::unix_endpoint(Path::new("/runtime/host.sock"), id).unwrap();
    let filename = Path::new(&endpoint).file_name().unwrap().to_str().unwrap();
    assert_eq!(filename, id.simple().to_string());
    assert_eq!(uuid::Uuid::parse_str(filename).unwrap(), id);
    let capacity = std::mem::size_of::<libc::sockaddr_un>()
        - std::mem::offset_of!(libc::sockaddr_un, sun_path);
    #[cfg(target_os = "macos")]
    assert_eq!(capacity, 104);
    let parent_component = "a".repeat(capacity - filename.len() - 3);
    let base = format!("/{parent_component}/host.sock");
    let allowed = crate::extension_transport::unix_endpoint(Path::new(&base), id).unwrap();
    assert_eq!(allowed.len(), capacity - 1);
    let too_long = format!("/{parent_component}a/host.sock");
    assert!(crate::extension_transport::unix_endpoint(Path::new(&too_long), id).is_none());
}

#[rstest]
#[tokio::test]
async fn private_provider_endpoint_checks_peer_and_rejects_task_requests() {
    use dcc_cua_client::HostClient;
    use serde_json::json;
    use std::time::Duration;
    let root = tempfile::tempdir().unwrap();
    let diagnostics = crate::connection_diagnostics::ConnectionDiagnostics::new(
        crate::connection_diagnostics::RuntimeBuildIdentity {
            runtime_version: env!("CARGO_PKG_VERSION").into(),
            source_revision: None,
            source_dirty: None,
            build_profile: None,
            target: None,
        },
        Some(root.path().to_owned()),
    );
    let record = diagnostics.snapshot();
    assert!(
        record.bridge.creation_id.is_some(),
        "native process birth missing"
    );
    assert!(record.bridge.is_live(), "native process birth must be live");
    let address = endpoint(&record.connection_id).unwrap();
    let listener = crate::extension_transport::ProviderListener::start(
        dcc_cua_core::ComputerUseDriver::create().unwrap(),
        &record.connection_id,
        "test-owner",
        &dcc_cua_host::TrustedTaskAuthorizationTarget::ExactWindow {
            process_id: 42,
            window_handle: 77,
        },
    )
    .unwrap();
    let mut client = None;
    let mut last_error = String::new();
    for _ in 0..50 {
        match crate::extension_transport::connect(&record).await {
            Ok(connected) => {
                client = Some(connected);
                break;
            }
            Err(error) => last_error = error,
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut client = client.unwrap_or_else(|| panic!("private listener ready: {last_error}"));
    assert!(
        HostClient::connect_to_process(&address, "wrong-peer", u32::MAX)
            .await
            .is_err()
    );
    let binding = client
        .request("browser_extension_transport_binding", json!({}))
        .await
        .unwrap()
        .value;
    assert_eq!(binding["process_id"], 42);
    assert_eq!(binding["window_handle"], 77);
    assert_eq!(binding.as_object().unwrap().len(), 3);
    assert!(client.request("register_browser_extension", json!({"hello":{}, "invocation_origin":"chrome-extension://published-extension/", "browser_process_id":42,"browser_window_handle":78})).await.is_err());
    let registered = client.request("register_browser_extension", json!({
        "hello":{"schema":"dcc-cua.browser-extension.v1","type":"hello","protocol":{"min":1,"max":1},
            "extension":{"id":"published-extension","version":"0.3.1"},"capabilities":["explicit_tab_pairing_v1"],
            "pairing":{"session_nonce":"owned-pair","tab_id":7,"window_id":9,"origin":"https://example.com","document_id":"doc-1"}},
        "invocation_origin":"chrome-extension://published-extension/", "browser_process_id":42,"browser_window_handle":77
    })).await.unwrap().value;
    for method in ["doctor", "desktop_snapshot", "interrupt_all"] {
        assert!(client.request(method, json!({})).await.is_err(), "{method}");
    }
    client.request("unregister_browser_extension", json!({"provider_id":registered["provider_id"],"provider_secret":registered["provider_secret"]})).await.unwrap();
    drop(client);
    drop(listener);
}

#[rstest]
fn incomplete_discovery_cannot_choose_a_pairing_owner() {
    let root = tempfile::tempdir().unwrap();
    let mut report = crate::connection_diagnostics::list_connections(Some(root.path().to_owned()));
    report.scan_truncated = true;
    assert!(crate::extension_transport::candidate_records(report).is_err());
}
