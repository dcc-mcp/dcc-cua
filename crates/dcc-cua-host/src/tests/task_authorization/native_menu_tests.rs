use super::*;
use rstest::rstest;

#[rstest]
fn native_menu_scope_never_confers_other_window_operations_and_legacy_scope_is_preserved() {
    let menu:TrustedTaskActionScope = serde_json::from_value(json!({"action":"dismiss_menu","input_kind":"window_state","secret_input":false,"authorization_category":"window_state"})).unwrap();
    let frame:TrustedTaskActionScope = serde_json::from_value(json!({"action":"set_window_frame","input_kind":"window_state","secret_input":false,"authorization_category":"window_state"})).unwrap();
    for operation in [
        WindowOperation::Activate,
        WindowOperation::RestoreActivate,
        WindowOperation::Close,
    ] {
        assert!(
            crate::request_handler::native_menu::validate_window_operation_scope(
                std::slice::from_ref(&menu),
                &operation
            )
            .is_err()
        );
        assert!(
            crate::request_handler::native_menu::validate_window_operation_scope(&[], &operation)
                .is_ok()
        );
        assert!(
            crate::request_handler::native_menu::validate_window_operation_scope(
                std::slice::from_ref(&frame),
                &operation
            )
            .is_ok()
        );
    }
    assert!(
        crate::request_handler::native_menu::validate_window_operation_scope(
            &[menu],
            &WindowOperation::DismissMenu
        )
        .is_ok()
    );
}

#[rstest]
fn native_menu_scope_is_independent_of_frame_raw_and_semantic_actions() {
    let value = json!({"action":"dismiss_menu","input_kind":"window_state","secret_input":false,"authorization_category":"window_state"});
    let scope: TrustedTaskActionScope = serde_json::from_value(value.clone()).unwrap();
    assert!(scope.validate());
    assert!(scope.is_native_menu_dismiss());
    assert!(!scope.is_window_frame());
    assert!(!scope.is_window_minimize());
    assert!(!scope.is_pixels_input());
    for (field, replacement) in [
        ("input_kind", json!("raw_input")),
        ("secret_input", json!(true)),
        ("authorization_category", json!("raw_input")),
        ("browser_origin", json!("https://example.com")),
    ] {
        let mut invalid = value.clone();
        invalid[field] = replacement;
        let invalid: TrustedTaskActionScope = serde_json::from_value(invalid).unwrap();
        assert!(!invalid.validate());
    }
}

async fn lease() -> (
    TrustedTaskAuthorizationIssuer,
    Arc<dyn TrustedTaskAuthorizationHost>,
    TrustedTaskAuthorizationLease,
) {
    let (issuer, authority) = trusted_task_authorization_broker();
    let mut registration = browser_credential_registration(unix_time_millis() + 60_000);
    registration.application_label = "Test DCC".into();
    registration.target = TrustedTaskAuthorizationTarget::ExactWindow {
        process_id: 42,
        window_handle: 77,
    };
    registration.allowed_host_methods =
        vec!["get_window_state".into(), "change_window_state".into()];
    registration.allowed_actions = vec![serde_json::from_value(json!({"action":"dismiss_menu","input_kind":"window_state","secret_input":false,"authorization_category":"window_state"})).unwrap()];
    registration.allowed_browser_origins.clear();
    let receipt = issuer.register(registration).unwrap();
    let lease = issue_task_authorization(
        Some(authority.as_ref()),
        TaskAuthorizationBinding::window(
            "connection-test",
            &receipt.authorization_id,
            "session-1",
            "grant-1",
            "Test DCC",
            &receipt.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 77,
            },
        ),
    )
    .await
    .unwrap();
    (issuer, authority, lease)
}

#[rstest]
#[tokio::test]
async fn native_menu_host_refuses_missing_token_without_sdk_or_native_dispatch() {
    let (_issuer, authority, lease) = lease().await;
    let channel = ObservationOnlyTestChannel::default();
    let driver = ComputerUseDriver::from_test_remote_channel(Arc::new(channel.clone())).unwrap();
    let mut host = cached_host_session(&driver);
    host.observation_mode = TaskObservationMode::PixelsOnly;
    host.capability = lease.window_capability.clone();
    host.task_authorization_host = Some(authority);
    host.task_authorization = Some(lease.clone());
    host.require_native_menu_dismiss_grant("session-1").unwrap();
    assert!(host.require_window_frame_grant("session-1").is_err());
    assert!(host.require_minimize_grant("session-1").is_err());
    let mut sessions = ConnectionSessions {
        connection_id: "connection-test".into(),
        ..ConnectionSessions::default()
    };
    sessions.windows.insert("session-1".into(), host);
    let request:Request = serde_json::from_value(json!({"method":"change_window_state","params":{
        "session_id":"session-1","task_grant_id":"grant-1","window_capability":lease.window_capability,"operation":"dismiss_menu"}})).unwrap();
    let error = handle_request(
        &driver,
        &mut sessions,
        &mut Some(SnapshotTransport::BinaryFrame),
        &mut None,
        &CancellationRegistry::default(),
        request,
    )
    .await
    .unwrap_err();
    let HostError::ComputerUse(error) = error else {
        panic!("typed stale token failure required")
    };
    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
    assert_eq!(error.details.unwrap().action_attempted, Some(false));
    assert!(
        sessions.windows["session-1"]
            .latest_observation_id
            .is_none()
    );
    assert_eq!(
        channel.exchanges.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[rstest]
#[tokio::test]
async fn native_menu_grant_rejects_foreign_binding_frame_scope_or_missing_read() {
    let (_issuer, _authority, lease) = lease().await;
    let channel = ObservationOnlyTestChannel::default();
    let driver = ComputerUseDriver::from_test_remote_channel(Arc::new(channel)).unwrap();
    let mut host = cached_host_session(&driver);
    host.observation_mode = TaskObservationMode::PixelsOnly;
    host.capability = lease.window_capability.clone();
    for change in 0..8 {
        let mut altered = lease.clone();
        match change {
            0 => altered.session_id.push('x'),
            1 => altered.task_grant_id.push('x'),
            2 => altered.window_capability.push('x'),
            3 => altered.target_process_id += 1,
            4 => altered.target_window_handle += 1,
            5 => altered
                .allowed_host_methods
                .retain(|method| method != "get_window_state"),
            6 => altered.allowed_actions[0].action = "set_window_frame".into(),
            _ => altered.allowed_actions.clear(),
        }
        host.task_authorization = Some(altered);
        assert!(
            host.require_native_menu_dismiss_grant("session-1").is_err(),
            "change {change}"
        );
    }
}
