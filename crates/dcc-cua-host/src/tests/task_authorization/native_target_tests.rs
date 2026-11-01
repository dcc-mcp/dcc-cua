use super::*;
use dcc_cua_core::{ComputerUseErrorCode, ComputerUseTargetScope};
use rstest::rstest;

async fn lease_for(
    methods: Vec<String>,
    action: TrustedTaskActionScope,
) -> (
    TrustedTaskAuthorizationIssuer,
    Arc<dyn TrustedTaskAuthorizationHost>,
    TrustedTaskAuthorizationLease,
) {
    let (issuer, host) = trusted_task_authorization_broker();
    let mut registration = browser_credential_registration(unix_time_millis() + 60_000);
    registration.allowed_host_methods = methods;
    registration.allowed_browser_origins = action.browser_origin.iter().cloned().collect();
    registration.allowed_actions = vec![action];
    let receipt = issuer.register(registration).unwrap();
    let binding = TaskAuthorizationBinding::window(
        "connection-test",
        &receipt.authorization_id,
        "session-1",
        "grant-1",
        "Chrome Web Store upload",
        &receipt.window_capability,
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 7,
        },
    );
    let lease = issue_task_authorization(Some(host.as_ref()), binding)
        .await
        .unwrap();
    (issuer, host, lease)
}

fn native_scope(category: &str) -> TrustedTaskActionScope {
    TrustedTaskActionScope {
        action: "click".into(),
        input_kind: "semantic".into(),
        secret_input: false,
        authorization_category: category.into(),
        browser_origin: None,
    }
}

#[rstest]
#[case("credential", "execute_action", true)]
#[case("account_access", "execute_action", true)]
#[case("credential", "snapshot", false)]
#[case("account_access", "browser_type", false)]
#[case("account_security", "execute_action", false)]
#[case("destructive", "execute_action", false)]
#[tokio::test]
async fn native_target_permission_requires_the_matching_login_action_and_method(
    #[case] category: &str,
    #[case] method: &str,
    #[case] expected: bool,
) {
    let (_, host, lease) = lease_for(vec![method.into()], native_scope(category)).await;
    assert_eq!(host.native_target_authorization(&lease).is_some(), expected);
}

#[rstest]
#[tokio::test]
async fn native_target_permission_accepts_only_the_retained_exact_lease() {
    let (_, host, lease) =
        lease_for(vec!["execute_action".into()], native_scope("credential")).await;
    assert!(host.native_target_authorization(&lease).is_some());
    for field in 0..11 {
        let mut foreign = lease.clone();
        match field {
            0 => foreign.connection_id.push('x'),
            1 => foreign.session_id.push('x'),
            2 => foreign.task_grant_id.push('x'),
            3 => foreign.window_capability.push('x'),
            4 => foreign.target_process_id += 1,
            5 => foreign.target_window_handle += 1,
            6 => foreign.request_digest.push('x'),
            7 => foreign.expires_at_unix_ms += 1,
            8 => foreign.allowed_actions[0].authorization_category = "account_access".into(),
            9 => foreign.allowed_host_methods.push("clipboard_read".into()),
            _ => foreign.application_label.push('x'),
        }
        assert!(
            host.native_target_authorization(&foreign).is_none(),
            "field {field}"
        );
    }
}

#[rstest]
#[tokio::test]
async fn native_target_permission_is_session_bound_and_revoked_before_provider_dispatch() {
    let (issuer, host, lease) = lease_for(
        vec!["execute_action".into()],
        native_scope("account_access"),
    )
    .await;
    let authorization = host.native_target_authorization(&lease).unwrap();
    let (driver, calls) = observation_only_test_driver();
    let scope = ComputerUseTargetScope {
        process_id: Some(42),
        window_handle: Some(7),
        window_title: None,
    };
    let mut session = driver
        .session(scope.clone(), "Application login", "runtime-1")
        .unwrap();
    session
        .set_native_task_authorization(Arc::clone(&authorization))
        .unwrap();
    let mut foreign = driver
        .session(scope, "Application login", "runtime-2")
        .unwrap();
    assert!(
        foreign
            .set_native_task_authorization(authorization)
            .is_err()
    );
    issuer.revoke(&lease.authorization_id).unwrap();
    assert!(host.native_target_authorization(&lease).is_none());
    let error = session.screenshot().await.unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::InvalidTarget);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[rstest]
#[tokio::test]
async fn browser_credential_permission_requires_its_secret_typing_method() {
    let action = browser_credential_registration(unix_time_millis() + 60_000)
        .allowed_actions
        .remove(0);
    let (_, host, lease) = lease_for(vec!["browser_type".into()], action.clone()).await;
    assert!(host.native_target_authorization(&lease).is_some());
    let (_, host, lease) = lease_for(vec!["browser_snapshot".into()], action).await;
    assert!(host.native_target_authorization(&lease).is_none());
}

#[rstest]
#[case("semantic", "content_change", "content_change")]
#[case("semantic", "task_grant", "task_grant")]
#[case("semantic", "task_grant", "credential")]
#[case("raw_input", "raw_input", "raw_input")]
#[tokio::test]
async fn routine_click_requires_its_explicit_task_scope_before_core_dispatch(
    #[case] input_kind: &str,
    #[case] category: &str,
    #[case] granted_category: &str,
) {
    for granted_action in ["type", "click"] {
        let permitted = granted_action == "click" && category == granted_category;
        let scope = TrustedTaskActionScope {
            action: granted_action.into(),
            input_kind: input_kind.into(),
            secret_input: false,
            authorization_category: granted_category.into(),
            browser_origin: None,
        };
        let (_, authority, lease) = lease_for(vec!["execute_action".into()], scope).await;
        let (driver, calls) = observation_only_test_driver();
        let mut host = cached_host_session(&driver);
        host.target_window_handle = 7;
        host.capability = lease.window_capability.clone();
        host.task_authorization = Some(lease.clone());
        host.task_authorization_host = Some(Arc::clone(&authority));
        host.latest_accessibility_root = Some(json!({"elements":[{
            "element_index":7, "element_token":"ordinary-control", "policy_tier":"task_grant",
            "policy_category": if category == "task_grant" { Value::Null } else { json!(category) }
        }]}));
        host.session
            .seed_test_observation(dcc_cua_core::ComputerUseObservation {
                observation_id: "observation-before-transition".into(),
                window_handle: 7,
                process_id: 42,
                window_title: "Synthetic target".into(),
                width: 800,
                height: 600,
                source_rect: [0, 0, 800, 600],
                capture_backend: "synthetic-test".into(),
                capture_provenance: json!({"source":"test-only"}),
                session_id: "runtime-session-1".into(),
            });
        let mut sessions = ConnectionSessions {
            connection_id: "connection-test".into(),
            ..ConnectionSessions::default()
        };
        sessions.windows.insert("session-1".into(), host);
        let action = if input_kind == "semantic" {
            json!({"action":"click", "input_kind":"semantic", "intent":"navigate", "element_index":7,
                "element_token":"ordinary-control", "delivery_mode":"background"})
        } else {
            json!({"action":"click", "input_kind":"raw_input", "intent":"navigate", "x":10,"y":10,
                "delivery_mode":"foreground"})
        };
        let request = serde_json::from_value(json!({"method":"execute_action", "params":{
            "session_id":"session-1", "task_grant_id":"grant-1", "window_capability":lease.window_capability,
            "observation_id":"observation-before-transition", "accessibility_state_id":"accessibility-before-transition",
            "action":action, "capture_after":false
        }})).unwrap();
        let security = HostSecurityServices::default().with_task_authorization_host(authority);
        let response = handle_request_with_security_services(
            &driver,
            &security,
            &mut sessions,
            &mut Some(SnapshotTransport::BinaryFrame),
            &mut None,
            &CancellationRegistry::default(),
            request,
        )
        .await;
        if permitted {
            // The scope is admitted, then this deliberately inactive synthetic
            // Core session refuses dispatch; no native operation can occur.
            assert!(
                matches!(response, Err(HostError::ComputerUse(error)) if error.code == ComputerUseErrorCode::BackendUnavailable)
            );
        } else {
            let (response, _) = response.unwrap();
            assert_eq!(response["error"], "task_authorization_out_of_scope");
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    }
}

#[rstest]
#[tokio::test]
async fn semantic_session_raw_input_revalidates_revocation_after_waiting_for_its_turn() {
    let mut scope = native_scope("credential");
    scope.input_kind = "raw_input".into();
    scope.authorization_category = "raw_input".into();
    let (issuer, authority, lease) = lease_for(vec!["execute_action".into()], scope).await;
    let (driver, calls) = observation_only_test_driver();
    let mut host = cached_host_session(&driver);
    host.target_window_handle = 7;
    host.capability = lease.window_capability.clone();
    host.task_authorization = Some(lease.clone());
    host.task_authorization_host = Some(Arc::clone(&authority));
    host.session
        .seed_test_observation(dcc_cua_core::ComputerUseObservation {
            observation_id: "observation-before-transition".into(),
            window_handle: 7,
            process_id: 42,
            window_title: "Synthetic target".into(),
            width: 800,
            height: 600,
            source_rect: [0, 0, 800, 600],
            capture_backend: "synthetic-test".into(),
            capture_provenance: json!({"source":"test-only"}),
            session_id: "runtime-session-1".into(),
        });
    let mut sessions = ConnectionSessions {
        connection_id: "connection-test".into(),
        ..ConnectionSessions::default()
    };
    sessions.windows.insert("session-1".into(), host);
    let request = serde_json::from_value(json!({"method":"execute_action", "params":{
        "session_id":"session-1", "task_grant_id":"grant-1", "window_capability":lease.window_capability,
        "observation_id":"observation-before-transition", "capture_after":false,
        "action":{"action":"click", "input_kind":"raw_input", "intent":"navigate", "x":10,"y":10,
            "delivery_mode":"foreground"}
    }})).unwrap();
    let security = HostSecurityServices::default().with_task_authorization_host(authority);
    let held_turn = RAW_INPUT_QUEUE.lock().await;
    let mut transport = Some(SnapshotTransport::BinaryFrame);
    let mut image = None;
    let cancellations = CancellationRegistry::default();
    let queued = handle_request_with_security_services(
        &driver,
        &security,
        &mut sessions,
        &mut transport,
        &mut image,
        &cancellations,
        request,
    );
    tokio::pin!(queued);
    std::future::poll_fn(|context| {
        assert!(std::future::Future::poll(queued.as_mut(), context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    issuer.revoke(&lease.authorization_id).unwrap();
    drop(held_turn);
    assert!(matches!(
        queued.await,
        Err(HostError::CodedProtocol {
            code: HostProtocolErrorCode::TaskAuthorizationRevoked,
            ..
        })
    ));
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
}
