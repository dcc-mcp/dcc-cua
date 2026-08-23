use rstest::rstest;

use super::*;

#[rstest]
#[case("windows_security_or_privacy")]
#[case("human_verification")]
fn trusted_confirmation_intents_require_the_explicit_grant(#[case] intent: &str) {
    let action = HostAction {
        action: "click".into(),
        element_index: None,
        element_token: None,
        delivery_mode: None,
        input_backend_id: None,
        input_kind: "raw_input".into(),
        intent: intent.into(),
        x: Some(10.0),
        y: Some(10.0),
        button: Some("left".into()),
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: Vec::new(),
        text: None,
        delay_ms: None,
        type_chars_only: false,
        checked: None,
        keys: Vec::new(),
        modifiers: Vec::new(),
        duration_ms: None,
        steps: None,
    };
    let tier = action.safety_tier(None);
    assert!(tier.rejection().is_none());
    assert!(tier.requires_confirmation());
}

struct EchoingConfirmationHost;

#[async_trait::async_trait]
impl TrustedActionConfirmationHost for EchoingConfirmationHost {
    async fn confirm(
        &self,
        request: TrustedActionConfirmationRequest,
    ) -> Result<TrustedActionConfirmationDecision, TrustedActionConfirmationHostError> {
        Ok(TrustedActionConfirmationDecision {
            action: TrustedActionConfirmationAction::Allow,
            request_digest: request.request_digest,
        })
    }
}

struct ReplayingConfirmationHost {
    request_digest: String,
}

struct UnexpectedConfirmationHost;

#[async_trait::async_trait]
impl TrustedActionConfirmationHost for UnexpectedConfirmationHost {
    async fn confirm(
        &self,
        _request: TrustedActionConfirmationRequest,
    ) -> Result<TrustedActionConfirmationDecision, TrustedActionConfirmationHostError> {
        panic!("the constructor-owned host must not run without the task-grant gate")
    }
}

#[async_trait::async_trait]
impl TrustedActionConfirmationHost for ReplayingConfirmationHost {
    async fn confirm(
        &self,
        _request: TrustedActionConfirmationRequest,
    ) -> Result<TrustedActionConfirmationDecision, TrustedActionConfirmationHostError> {
        Ok(TrustedActionConfirmationDecision {
            action: TrustedActionConfirmationAction::Allow,
            request_digest: self.request_digest.clone(),
        })
    }
}

fn confirmation_action() -> HostAction {
    HostAction {
        action: "click".into(),
        element_index: Some(7),
        element_token: Some("submit-button".into()),
        delivery_mode: Some("foreground".into()),
        input_backend_id: None,
        input_kind: "semantic".into(),
        intent: "confirm".into(),
        x: None,
        y: None,
        button: Some("left".into()),
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: Vec::new(),
        text: None,
        delay_ms: None,
        type_chars_only: false,
        checked: None,
        keys: Vec::new(),
        modifiers: Vec::new(),
        duration_ms: None,
        steps: None,
    }
}

#[rstest]
fn trusted_confirmation_request_exposes_the_exact_window_identity() {
    let request = TrustedActionConfirmationRequest::for_window_action(
        "session-1",
        "grant-1",
        "capability-1",
        ConfirmationWindowIdentity {
            process_id: 4242,
            window_handle: 0x1234,
        },
        "observation-1",
        "accessibility-1",
        &confirmation_action(),
    )
    .unwrap();

    assert_eq!(request.target_process_id, Some(4242));
    assert_eq!(request.target_window_handle, Some(0x1234));
}

#[rstest]
#[tokio::test]
async fn trusted_confirmation_requires_a_constructor_owned_host() {
    let outcome = authorize_action_confirmation(
        None,
        true,
        TrustedActionConfirmationRequest::for_window_action(
            "session-1",
            "grant-1",
            "capability-1",
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
            "observation-1",
            "accessibility-1",
            &confirmation_action(),
        )
        .unwrap(),
    )
    .await;

    assert_eq!(outcome, ActionConfirmationOutcome::Required);
}

#[rstest]
#[tokio::test]
async fn trusted_confirmation_task_grant_gate_cannot_be_bypassed_by_the_host() {
    let host: Arc<dyn TrustedActionConfirmationHost> = Arc::new(UnexpectedConfirmationHost);
    let outcome = authorize_action_confirmation(
        Some(host.as_ref()),
        false,
        TrustedActionConfirmationRequest::for_window_action(
            "session-1",
            "grant-1",
            "capability-1",
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
            "observation-1",
            "accessibility-1",
            &confirmation_action(),
        )
        .unwrap(),
    )
    .await;

    assert_eq!(outcome, ActionConfirmationOutcome::Required);
}

#[rstest]
#[tokio::test]
async fn trusted_confirmation_accepts_an_exact_action_bound_decision() {
    let host: Arc<dyn TrustedActionConfirmationHost> = Arc::new(EchoingConfirmationHost);
    let outcome = authorize_action_confirmation(
        Some(host.as_ref()),
        true,
        TrustedActionConfirmationRequest::for_window_action(
            "session-1",
            "grant-1",
            "capability-1",
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
            "observation-1",
            "accessibility-1",
            &confirmation_action(),
        )
        .unwrap(),
    )
    .await;

    assert_eq!(outcome, ActionConfirmationOutcome::Allowed);
}

#[rstest]
#[tokio::test]
async fn trusted_confirmation_rejects_a_replayed_decision_for_new_evidence() {
    let first = TrustedActionConfirmationRequest::for_window_action(
        "session-1",
        "grant-1",
        "capability-1",
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 7,
        },
        "observation-1",
        "accessibility-1",
        &confirmation_action(),
    )
    .unwrap();
    let host: Arc<dyn TrustedActionConfirmationHost> = Arc::new(ReplayingConfirmationHost {
        request_digest: first.request_digest,
    });
    let second = TrustedActionConfirmationRequest::for_window_action(
        "session-1",
        "grant-1",
        "capability-1",
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 7,
        },
        "observation-2",
        "accessibility-2",
        &confirmation_action(),
    )
    .unwrap();

    let outcome = authorize_action_confirmation(Some(host.as_ref()), true, second).await;

    assert_eq!(outcome, ActionConfirmationOutcome::Required);
}
