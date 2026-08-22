use rstest::rstest;

use super::*;

#[rstest]
fn protocol_limits_are_ordered_and_bounded() {
    const {
        assert!(HOST_PROTOCOL_VERSION == 1);
        assert!(MAX_BINARY_FRAME_BYTES > MAX_JSON_FRAME_BYTES);
        assert!(MAX_HOST_CONNECTIONS > 0);
        assert!(MAX_SESSIONS_PER_CONNECTION > 0);
        assert!(MAX_SESSIONS_PER_CONNECTION <= MAX_HOST_CONNECTIONS);
        assert!(MAX_PARALLEL_DISCOVERY_REQUESTS > 0);
    }
}

#[rstest]
fn request_envelope_owns_shared_id_method_and_params_validation() {
    let envelope = RequestEnvelope::from_value(&serde_json::json!({
        "request_id": "request-1",
        "method": "list_apps",
    }))
    .unwrap();
    assert_eq!(envelope.request_id.as_deref(), Some("request-1"));
    assert_eq!(envelope.method, "list_apps");
    assert_eq!(envelope.params, serde_json::json!({}));

    for invalid in [
        serde_json::json!([]),
        serde_json::json!({"request_id": "", "method": "list_apps"}),
        serde_json::json!({"method": ""}),
        serde_json::json!({"method": "list_apps", "params": []}),
    ] {
        assert!(RequestEnvelope::from_value(&invalid).is_err());
    }
}

#[rstest]
fn method_traits_are_one_closed_cross_component_taxonomy() {
    let action = host_method_traits("execute_action");
    assert!(action.action);
    assert!(!action.pipeline_safe);

    let snapshot = host_method_traits("snapshot");
    assert!(snapshot.standalone_snapshot);
    assert!(snapshot.pipeline_safe);

    let discovery = host_method_traits("list_apps");
    assert!(discovery.parallel_discovery);
    assert!(discovery.pipeline_safe);

    let semantic = host_method_traits("session_health");
    assert!(semantic.semantic_observation);
    assert!(semantic.pipeline_safe);

    assert_eq!(host_method_traits("unknown"), HostMethodTraits::default());
}

#[tokio::test]
async fn shared_frame_codec_round_trips_and_enforces_one_limit() {
    let (mut client, mut server) = tokio::io::duplex(64);
    let writer = tokio::spawn(async move { write_frame(&mut client, b"frame", 16).await });
    let body = read_frame(&mut server, 16).await.unwrap().unwrap();
    writer.await.unwrap().unwrap();
    assert_eq!(body, b"frame");

    let (mut client, _server) = tokio::io::duplex(64);
    assert!(write_frame(&mut client, b"too long", 4).await.is_err());
}

#[cfg(windows)]
#[rstest]
fn windows_endpoint_is_local_and_session_scoped() {
    let endpoint = default_endpoint();
    assert!(endpoint.starts_with(r"\\.\pipe\dcc-cua-v1"));
}

#[cfg(unix)]
#[rstest]
fn private_xdg_runtime_directory_owns_the_default_socket() {
    use std::os::unix::fs::PermissionsExt;

    let runtime_dir = std::env::temp_dir().join(format!(
        "dcc-cua-protocol-{}-{}",
        effective_user_id(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir(&runtime_dir);
    std::fs::create_dir(&runtime_dir).unwrap();
    std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700)).unwrap();

    let endpoint = default_unix_endpoint_from(
        Some(runtime_dir.as_os_str()),
        &std::env::temp_dir(),
        effective_user_id(),
    );
    assert_eq!(endpoint, runtime_dir.join(UNIX_SOCKET_NAME));

    std::fs::remove_dir(runtime_dir).unwrap();
}

#[cfg(unix)]
#[rstest]
fn insecure_xdg_runtime_directory_falls_back_to_a_user_namespace() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = std::env::temp_dir();
    let runtime_dir = temp_dir.join(format!(
        "dcc-cua-protocol-insecure-{}-{}",
        effective_user_id(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir(&runtime_dir);
    std::fs::create_dir(&runtime_dir).unwrap();
    std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    let endpoint = default_unix_endpoint_from(
        Some(runtime_dir.as_os_str()),
        &temp_dir,
        effective_user_id(),
    );
    assert_eq!(
        endpoint,
        temp_dir
            .join(format!("dcc-cua-{}", effective_user_id()))
            .join(UNIX_SOCKET_NAME)
    );

    std::fs::remove_dir(runtime_dir).unwrap();
}
