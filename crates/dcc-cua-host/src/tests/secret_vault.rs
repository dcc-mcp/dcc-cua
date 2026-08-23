use rstest::rstest;

use crate::secret_vault::extract_clipboard_secret;

use super::*;

fn secret_action(text: Option<&str>, secret_handle: Option<&str>) -> HostAction {
    HostAction {
        action: "set_text".into(),
        element_index: Some(3),
        element_token: Some("credential-input".into()),
        delivery_mode: None,
        input_backend_id: None,
        input_kind: "semantic".into(),
        intent: "credential_input".into(),
        x: None,
        y: None,
        button: None,
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: Vec::new(),
        text: text.map(str::to_owned),
        secret_handle: secret_handle.map(str::to_owned),
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
#[case("")]
#[case(" leading")]
#[case("contains/slash")]
#[case("x y")]
#[case("x".repeat(MAX_SECRET_HANDLE_CHARS + 1))]
fn secret_handles_are_bounded_and_opaque(#[case] handle: impl AsRef<str>) {
    assert!(validate_secret_handle(handle.as_ref()).is_err());
}

#[rstest]
fn secret_values_never_reveal_their_contents_through_debug() {
    let value = SecretValue::new("model-must-not-see-this").unwrap();
    assert_eq!(format!("{value:?}"), "SecretValue([REDACTED])");
}

#[rstest]
fn host_actions_reject_mixed_plaintext_and_secret_handle_sources() {
    let action = secret_action(Some("plaintext"), Some("edge.api-key"));
    assert!(action.validate_secret_source().is_err());
}

#[rstest]
fn secret_handles_never_widen_missing_or_hard_denied_semantic_evidence() {
    let action = secret_action(None, Some("edge.api-key"));
    assert_eq!(action.safety_tier(None), HostActionSafetyTier::HardDeny);
    assert_eq!(
        action.safety_tier(Some(&json!({
            "elements": [{
                "element_index": 3,
                "element_token": "credential-input",
                "policy_tier": "hard_deny"
            }]
        }))),
        HostActionSafetyTier::HardDeny
    );
}

#[rstest]
fn trusted_confirmation_binds_only_the_secret_handle() {
    let action = secret_action(None, Some("edge.api-key"));
    action.validate_secret_source().unwrap();
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
        &action,
    )
    .unwrap();

    assert_eq!(request.action["secret_handle"], "edge.api-key");
    assert!(request.action.get("text").is_none());
    assert!(
        !serde_json::to_string(&request)
            .unwrap()
            .contains("model-must-not-see-this")
    );
}

#[rstest]
fn clipboard_secret_extraction_accepts_only_non_empty_structured_text() {
    let value = extract_clipboard_secret(json!({
        "structuredContent": {
            "supported": true,
            "text": "captured-secret",
            "privacy_sensitive": true,
            "content_redacted_from_telemetry": true
        }
    }))
    .unwrap();
    assert_eq!(value.expose(), "captured-secret");

    assert!(
        extract_clipboard_secret(json!({
            "structuredContent": {"supported": true, "text": ""}
        }))
        .is_err()
    );
    assert!(
        extract_clipboard_secret(json!({
            "text": "unstructured-secret"
        }))
        .is_err()
    );
}
