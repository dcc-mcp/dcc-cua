use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rstest::rstest;

use super::*;
use crate::action_confirmation::ConfirmationBinding;
use crate::task_authorization::{
    TaskAuthorizationBinding, TaskAuthorizationOutcome, authorize_task_scoped_action,
    issue_task_authorization,
};

struct TaskAuthorizationHost {
    revoked: bool,
}

struct DenyingTaskAuthorizationHost;

#[async_trait::async_trait]
impl TrustedTaskAuthorizationHost for DenyingTaskAuthorizationHost {
    async fn authorize(
        &self,
        _request: TrustedTaskAuthorizationRequest,
    ) -> Result<TrustedTaskAuthorizationLease, TrustedTaskAuthorizationHostError> {
        Err(TrustedTaskAuthorizationHostError::Denied)
    }

    async fn validate(
        &self,
        _request: TrustedTaskAuthorizationValidationRequest,
    ) -> Result<TrustedTaskAuthorizationValidationDecision, TrustedTaskAuthorizationHostError> {
        panic!("a denied task authorization must not be validated")
    }
}

#[async_trait::async_trait]
impl TrustedTaskAuthorizationHost for TaskAuthorizationHost {
    async fn authorize(
        &self,
        request: TrustedTaskAuthorizationRequest,
    ) -> Result<TrustedTaskAuthorizationLease, TrustedTaskAuthorizationHostError> {
        let now = unix_time_millis();
        Ok(TrustedTaskAuthorizationLease {
            authorization_id: request.authorization_id,
            session_id: request.session_id,
            task_grant_id: request.task_grant_id,
            application_label: request.application_label,
            window_capability: request.window_capability,
            target_process_id: request.target_process_id,
            target_window_handle: request.target_window_handle,
            allowed_actions: vec![TrustedTaskActionScope {
                action: "type_chars".into(),
                input_kind: "raw_input".into(),
                secret_input: false,
                authorization_category: "raw_input".into(),
                browser_origin: None,
            }],
            issued_at_unix_ms: now,
            expires_at_unix_ms: now + 60_000,
            request_digest: request.request_digest,
        })
    }

    async fn validate(
        &self,
        request: TrustedTaskAuthorizationValidationRequest,
    ) -> Result<TrustedTaskAuthorizationValidationDecision, TrustedTaskAuthorizationHostError> {
        Ok(TrustedTaskAuthorizationValidationDecision {
            status: if self.revoked {
                TrustedTaskAuthorizationStatus::Revoked
            } else {
                TrustedTaskAuthorizationStatus::Active
            },
            request_digest: request.request_digest,
        })
    }
}

fn unix_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn task_binding<'a>() -> TaskAuthorizationBinding<'a> {
    TaskAuthorizationBinding::window(
        "authorization-1",
        "session-1",
        "grant-1",
        "CWS upload",
        "capability-1",
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 7,
        },
    )
}

fn type_chars_confirmation(secret_handle: Option<&str>) -> TrustedActionConfirmationRequest {
    type_chars_confirmation_for_target(secret_handle, "raw_input", 42, 7)
}

fn type_chars_confirmation_for_target(
    secret_handle: Option<&str>,
    authorization_category: &str,
    process_id: u32,
    window_handle: u64,
) -> TrustedActionConfirmationRequest {
    let action = HostAction {
        action: "type_chars".into(),
        element_index: None,
        element_token: None,
        delivery_mode: Some("foreground".into()),
        input_backend_id: None,
        input_kind: "raw_input".into(),
        intent: "ordinary_edit".into(),
        x: None,
        y: None,
        button: None,
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: Vec::new(),
        text: secret_handle.is_none().then(|| "bounded path".into()),
        secret_handle: secret_handle.map(str::to_owned),
        delay_ms: None,
        type_chars_only: true,
        checked: None,
        keys: Vec::new(),
        modifiers: Vec::new(),
        duration_ms: None,
        steps: None,
    };
    let mut action_value = serde_json::to_value(&action).unwrap();
    action_value["authorization_category"] = json!(authorization_category);
    TrustedActionConfirmationRequest::for_bound_window_action_value(
        ConfirmationBinding::window(
            "session-1",
            "grant-1",
            "capability-1",
            ConfirmationWindowIdentity {
                process_id,
                window_handle,
            },
            "observation-1",
            Some("accessibility-1"),
        ),
        &action.intent,
        action_value,
    )
    .unwrap()
}

fn browser_secret_confirmation(
    origin: &str,
    window_capability: &str,
) -> TrustedActionConfirmationRequest {
    TrustedActionConfirmationRequest::for_bound_window_action_value(
        ConfirmationBinding::window(
            "session-1",
            "grant-1",
            window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
            "snapshot-1",
            Some("tab-1"),
        ),
        "credential_input",
        json!({
            "method": "browser_type",
            "browser_origin": origin,
            "request": {
                "target_id": "target-1",
                "tab_id": "tab-1",
                "snapshot_id": "snapshot-1",
                "ref": "field-1",
                "secret_handle": "chrome-web-store.refresh-token"
            }
        }),
    )
    .unwrap()
}

fn browser_credential_registration(
    expires_at_unix_ms: u64,
) -> TrustedTaskAuthorizationRegistration {
    TrustedTaskAuthorizationRegistration {
        task_grant_id: "grant-1".into(),
        application_label: "Chrome Web Store upload".into(),
        target_process_id: 42,
        target_window_handle: 7,
        allowed_actions: vec![TrustedTaskActionScope {
            action: "browser_type".into(),
            input_kind: "browser".into(),
            secret_input: true,
            authorization_category: "credential".into(),
            browser_origin: Some("https://chromewebstore.google.com".into()),
        }],
        expires_at_unix_ms,
    }
}

#[rstest]
#[tokio::test]
async fn broker_turns_one_trusted_embedding_registration_into_an_exact_no_popup_lease() {
    let (issuer, host) = trusted_task_authorization_broker();
    let receipt = issuer
        .register(browser_credential_registration(unix_time_millis() + 60_000))
        .unwrap();
    assert!(receipt.window_capability.starts_with("cua-window-"));
    let binding = TaskAuthorizationBinding::window(
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
    let outcome = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &browser_secret_confirmation(
            "https://chromewebstore.google.com",
            &receipt.window_capability,
        ),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::Allowed);
    assert_eq!(lease.allowed_actions.len(), 1);
}

#[rstest]
#[tokio::test]
async fn broker_registration_is_single_use_and_cannot_be_replayed_into_a_second_session() {
    let (issuer, host) = trusted_task_authorization_broker();
    let receipt = issuer
        .register(browser_credential_registration(unix_time_millis() + 60_000))
        .unwrap();
    let first = TaskAuthorizationBinding::window(
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
    issue_task_authorization(Some(host.as_ref()), first)
        .await
        .unwrap();
    let replay = TaskAuthorizationBinding::window(
        &receipt.authorization_id,
        "session-2",
        "grant-1",
        "Chrome Web Store upload",
        &receipt.window_capability,
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 7,
        },
    );

    let error = issue_task_authorization(Some(host.as_ref()), replay)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        HostError::CodedProtocol {
            code: HostProtocolErrorCode::TaskAuthorizationDenied,
            ..
        }
    ));
}

#[rstest]
#[tokio::test]
async fn broker_rejects_a_different_exact_target_before_opening_the_session() {
    let (issuer, host) = trusted_task_authorization_broker();
    let receipt = issuer
        .register(browser_credential_registration(unix_time_millis() + 60_000))
        .unwrap();
    let changed_target = TaskAuthorizationBinding::window(
        &receipt.authorization_id,
        "session-1",
        "grant-1",
        "Chrome Web Store upload",
        &receipt.window_capability,
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 8,
        },
    );

    let error = issue_task_authorization(Some(host.as_ref()), changed_target)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        HostError::CodedProtocol {
            code: HostProtocolErrorCode::TaskAuthorizationDenied,
            ..
        }
    ));
}

#[rstest]
#[tokio::test]
async fn broker_revocation_stops_an_active_task_without_falling_back_to_a_popup() {
    let (issuer, host) = trusted_task_authorization_broker();
    let receipt = issuer
        .register(browser_credential_registration(unix_time_millis() + 60_000))
        .unwrap();
    let binding = TaskAuthorizationBinding::window(
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
    issuer.revoke(&receipt.authorization_id).unwrap();

    let outcome = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &browser_secret_confirmation(
            "https://chromewebstore.google.com",
            &receipt.window_capability,
        ),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::Revoked);
}

#[rstest]
fn broker_rejects_expired_or_ambiguous_scope_before_issuing_an_authorization_id() {
    let (issuer, _host) = trusted_task_authorization_broker();
    let expired = issuer.register(browser_credential_registration(
        unix_time_millis().saturating_sub(1),
    ));
    assert!(matches!(
        expired,
        Err(TrustedTaskAuthorizationBrokerError::InvalidRegistration { .. })
    ));

    let mut duplicate = browser_credential_registration(unix_time_millis() + 60_000);
    duplicate
        .allowed_actions
        .push(duplicate.allowed_actions[0].clone());
    assert!(matches!(
        issuer.register(duplicate),
        Err(TrustedTaskAuthorizationBrokerError::InvalidRegistration { .. })
    ));
}

#[rstest]
#[tokio::test]
async fn active_task_authorization_allows_the_exact_scoped_action_without_a_popup() {
    let host: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: false });
    let lease = issue_task_authorization(Some(host.as_ref()), task_binding())
        .await
        .unwrap();

    let outcome = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &type_chars_confirmation(None),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::Allowed);
}

#[rstest]
#[tokio::test]
async fn explicit_task_start_denial_is_typed_and_never_opens_a_session() {
    let host: Arc<dyn TrustedTaskAuthorizationHost> = Arc::new(DenyingTaskAuthorizationHost);
    let error = issue_task_authorization(Some(host.as_ref()), task_binding())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        HostError::CodedProtocol {
            code: HostProtocolErrorCode::TaskAuthorizationDenied,
            ..
        }
    ));
}

#[rstest]
#[tokio::test]
async fn task_authorization_does_not_widen_from_plaintext_to_secret_input() {
    let host: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: false });
    let lease = issue_task_authorization(Some(host.as_ref()), task_binding())
        .await
        .unwrap();

    let outcome = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &type_chars_confirmation(Some("secret-handle-1")),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::OutOfScope);
}

#[rstest]
#[tokio::test]
async fn revoked_task_authorization_fails_closed_without_falling_back_to_a_popup() {
    let issuing: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: false });
    let lease = issue_task_authorization(Some(issuing.as_ref()), task_binding())
        .await
        .unwrap();
    let revoked: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: true });

    let outcome = authorize_task_scoped_action(
        Some(revoked.as_ref()),
        Some(&lease),
        &type_chars_confirmation(None),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::Revoked);
}

#[rstest]
#[tokio::test]
async fn expired_task_authorization_fails_before_constructor_host_validation() {
    let host: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: false });
    let mut lease = issue_task_authorization(Some(host.as_ref()), task_binding())
        .await
        .unwrap();
    lease.expires_at_unix_ms = unix_time_millis().saturating_sub(1);

    let outcome = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &type_chars_confirmation(None),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::Expired);
}

#[rstest]
#[tokio::test]
async fn target_change_requires_a_new_task_authorization() {
    let host: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: false });
    let lease = issue_task_authorization(Some(host.as_ref()), task_binding())
        .await
        .unwrap();

    let outcome = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &type_chars_confirmation_for_target(None, "raw_input", 42, 8),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::OutOfScope);
}

#[rstest]
#[tokio::test]
async fn risk_category_cannot_widen_from_raw_input_to_payment() {
    let host: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: false });
    let lease = issue_task_authorization(Some(host.as_ref()), task_binding())
        .await
        .unwrap();

    let outcome = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &type_chars_confirmation_for_target(None, "payment", 42, 7),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::OutOfScope);
}

#[rstest]
#[tokio::test]
async fn browser_credential_scope_requires_the_exact_observed_origin() {
    let host: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: false });
    let mut lease = issue_task_authorization(Some(host.as_ref()), task_binding())
        .await
        .unwrap();
    lease.allowed_actions = vec![TrustedTaskActionScope {
        action: "browser_type".into(),
        input_kind: "browser".into(),
        secret_input: true,
        authorization_category: "credential".into(),
        browser_origin: Some("https://chromewebstore.google.com".into()),
    }];

    let allowed = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &browser_secret_confirmation("https://chromewebstore.google.com", "capability-1"),
    )
    .await;
    let refused = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &browser_secret_confirmation("https://payments.google.com", "capability-1"),
    )
    .await;

    assert_eq!(allowed, TaskAuthorizationOutcome::Allowed);
    assert_eq!(refused, TaskAuthorizationOutcome::OutOfScope);
}

#[rstest]
fn a_bare_boolean_cannot_enable_task_authorization() {
    let grant = serde_json::from_value::<TaskGrant>(json!({
        "task_grant_id": "grant-1",
        "application_label": "CWS upload",
        "task_authorization": true
    }));
    assert!(grant.is_err());
}

#[rstest]
fn task_authorization_failures_are_machine_readable_and_non_modal() {
    for (outcome, expected) in [
        (
            ActionConfirmationOutcome::TaskAuthorizationRequired,
            "task_authorization_required",
        ),
        (
            ActionConfirmationOutcome::TaskAuthorizationOutOfScope,
            "task_authorization_out_of_scope",
        ),
        (
            ActionConfirmationOutcome::TaskAuthorizationExpired,
            "task_authorization_expired",
        ),
        (
            ActionConfirmationOutcome::TaskAuthorizationRevoked,
            "task_authorization_revoked",
        ),
    ] {
        let response = crate::request_contract::action_confirmation_refusal(outcome).0;
        assert_eq!(response["error"], expected);
        assert_eq!(response["success"], false);
    }
}
