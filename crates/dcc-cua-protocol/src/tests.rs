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
