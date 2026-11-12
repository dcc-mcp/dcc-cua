use super::*;
use crate::extension_input::ExtensionObservation;
use crate::task_authorization::{TaskAuthorizationBinding, issue_task_authorization};
use rstest::rstest;

fn snapshot() -> Value {
    json!({"snapshot_id":"snapshot-1","document":{"origin":"https://example.com","id":"doc-1"},
        "refs":[{"ref":"p1:1","actions":["click","type"],"visibility":"in_viewport","states":{"disabled":false,"sensitive":false}}]})
}

#[rstest]
fn extension_evidence_refuses_foreign_provider_origin_snapshot_and_sensitive_type() {
    let evidence =
        ExtensionObservation::new("provider-1", "https://example.com", snapshot()).unwrap();
    let params =
        json!({"snapshot_id":"snapshot-1","ref":"p1:1","text":"public text","replace":true});
    let action = evidence
        .action("provider-1", "https://example.com", "type", &params)
        .unwrap();
    assert!(!action.to_string().contains("public text"));
    for (provider, origin, change) in [
        ("other", "https://example.com", params.clone()),
        ("provider-1", "https://evil.test", params.clone()),
        (
            "provider-1",
            "https://example.com",
            json!({"snapshot_id":"old","ref":"p1:1"}),
        ),
        (
            "provider-1",
            "https://example.com",
            json!({"snapshot_id":"snapshot-1","ref":"missing"}),
        ),
    ] {
        assert!(evidence.action(provider, origin, "type", &change).is_err());
    }
    for sensitive in [json!(true), json!(null)] {
        let mut data = snapshot();
        data["refs"][0]["states"]["sensitive"] = sensitive;
        let evidence =
            ExtensionObservation::new("provider-1", "https://example.com", data).unwrap();
        assert!(
            evidence
                .action("provider-1", "https://example.com", "type", &params)
                .is_err()
        );
    }
}

#[rstest]
#[case("click", "semantic")]
#[case("browser_extension_navigate", "browser")]
#[tokio::test]
async fn extension_input_checks_exact_action_lease_and_revocation_without_driver_calls(
    #[case] action_name: &str,
    #[case] input_kind: &str,
) {
    let (driver, exchanges) = task_authorization::observation_only_test_driver();
    let mut host = cached_host_session(&driver);
    let (issuer, authority) = trusted_task_authorization_broker();
    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 60_000;
    let receipt = issuer
        .register(TrustedTaskAuthorizationRegistration {
            connection_id: Some("connection-1".into()),
            task_id: Some("session-1".into()),
            task_grant_id: "grant-1".into(),
            application_label: "Test extension".into(),
            target: TrustedTaskAuthorizationTarget::ExactWindow {
                process_id: 42,
                window_handle: 77,
            },
            allowed_host_methods: vec!["browser_extension_call".into()],
            allowed_actions: vec![TrustedTaskActionScope {
                action: action_name.into(),
                input_kind: input_kind.into(),
                secret_input: false,
                authorization_category: "external_effect".into(),
                browser_origin: (input_kind == "browser").then(|| "https://example.com".into()),
            }],
            allowed_browser_origins: vec!["https://example.com".into()],
            browser_scope: None,
            recording_output_dir: None,
            capture_preparation: None,
            expires_at_unix_ms: expires,
        })
        .unwrap();
    let lease = issue_task_authorization(
        Some(authority.as_ref()),
        TaskAuthorizationBinding::window(
            "connection-1",
            &receipt.authorization_id,
            "session-1",
            "grant-1",
            "Test extension",
            &receipt.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 77,
            },
        ),
    )
    .await
    .unwrap();
    host.capability = lease.window_capability.clone();
    host.task_authorization = Some(lease);
    let services = HostSecurityServices::default().with_task_authorization_host(authority);
    let evidence =
        ExtensionObservation::new("provider-1", "https://example.com", snapshot()).unwrap();
    let params = if input_kind == "browser" {
        json!({"snapshot_id":"snapshot-1","url":"https://example.com/next"})
    } else {
        json!({"snapshot_id":"snapshot-1","ref":"p1:1","text":"public","replace":true})
    };
    let click = evidence
        .action(
            "provider-1",
            "https://example.com",
            if input_kind == "browser" {
                "navigate"
            } else {
                "click"
            },
            &params,
        )
        .unwrap();
    crate::extension_input::authorize(&host, &services, "session-1", click.clone(), "snapshot-1")
        .await
        .unwrap();
    if input_kind == "semantic" {
        let navigation = evidence
            .action(
                "provider-1",
                "https://example.com",
                "navigate",
                &json!({"snapshot_id":"snapshot-1","url":"https://example.com/next"}),
            )
            .unwrap();
        assert!(
            crate::extension_input::authorize(
                &host,
                &services,
                "session-1",
                navigation,
                "snapshot-1"
            )
            .await
            .is_err()
        );
    }
    let typing = evidence
        .action(
            "provider-1",
            "https://example.com",
            "type",
            &json!({"snapshot_id":"snapshot-1","ref":"p1:1","text":"public","replace":true}),
        )
        .unwrap();
    assert!(
        crate::extension_input::authorize(&host, &services, "session-1", typing, "snapshot-1")
            .await
            .is_err()
    );
    assert!(
        crate::extension_input::authorize(
            &host,
            &services,
            "other-session",
            click.clone(),
            "snapshot-1"
        )
        .await
        .is_err()
    );
    issuer.revoke(&receipt.authorization_id).unwrap();
    assert!(
        crate::extension_input::authorize(&host, &services, "session-1", click, "snapshot-1")
            .await
            .is_err()
    );
    assert_eq!(exchanges.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[rstest]
fn extension_navigation_requires_fresh_same_origin_non_secret_url_without_a_dom_ref() {
    let evidence =
        ExtensionObservation::new("provider-1", "https://example.com", snapshot()).unwrap();
    let params = json!({"snapshot_id":"snapshot-1","url":"https://example.com/next?public=1"});
    let action = evidence
        .action("provider-1", "https://example.com", "navigate", &params)
        .unwrap();
    assert_eq!(action["action"], "browser_extension_navigate");
    assert!(!action.to_string().contains("public=1"));
    for value in [
        json!({"snapshot_id":"old","url":"https://example.com/next"}),
        json!({"snapshot_id":"snapshot-1","url":"https://other.test/"}),
        json!({"snapshot_id":"snapshot-1","url":"https://user:secret@example.com/"}),
        json!({"snapshot_id":"snapshot-1","url":"file:///private"}),
        json!({"snapshot_id":"snapshot-1","url":"https://example.com/","deadline_unix_ms":999999}),
    ] {
        assert!(
            evidence
                .action("provider-1", "https://example.com", "navigate", &value)
                .is_err()
        );
    }
    assert!(
        evidence
            .action("foreign", "https://example.com", "navigate", &params)
            .is_err()
    );
}

#[rstest]
#[tokio::test]
async fn provider_transport_cannot_open_sessions_observe_or_issue_authorizations() {
    let (driver, exchanges) = task_authorization::observation_only_test_driver();
    let services = HostSecurityServices::browser_extension_transport();
    let mut sessions = ConnectionSessions::default();
    for method in [
        "open_session",
        "desktop_snapshot",
        "interrupt_all",
        "doctor",
        "list_windows",
        "browser_extension_status",
    ] {
        let value = match method {
            "open_session" => {
                json!({"method":method,"session_id":"forbidden","grant":{"task_grant_id":"g","application_label":"No"}})
            }
            "browser_extension_status" => {
                json!({"method":method,"session_id":"none","task_grant_id":"none","window_capability":"none"})
            }
            _ => json!({"method":method}),
        };
        let mut params = value.as_object().unwrap().clone();
        params.remove("method");
        let value = json!({"method":method,"params":params});
        if let Ok(request) = serde_json::from_value::<Request>(value) {
            let result = handle_request_with_security_services(
                &driver,
                &services,
                &mut sessions,
                &mut Some(SnapshotTransport::BinaryFrame),
                &mut None,
                &Arc::new(Mutex::new(HashMap::new())),
                request,
            )
            .await;
            assert!(result.is_err(), "{method}");
        } else {
            panic!("invalid fixture for {method}");
        }
    }
    assert!(sessions.windows.is_empty());
    assert_eq!(exchanges.load(std::sync::atomic::Ordering::Relaxed), 0);
}
