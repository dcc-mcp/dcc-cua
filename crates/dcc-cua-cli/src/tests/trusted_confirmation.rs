use dcc_cua_host::{TRUSTED_ACTION_CONFIRMATION_SCHEMA, TrustedActionConfirmationRequest};
use rstest::rstest;
use serde_json::json;

use crate::trusted_confirmation::{native_confirmation_host, prompt_text};

#[cfg(windows)]
#[rstest]
fn windows_cli_host_installs_the_native_confirmation_boundary() {
    assert!(native_confirmation_host().is_some());
}

#[cfg(not(windows))]
#[rstest]
fn cli_host_requires_an_embedding_confirmation_boundary_without_a_native_prompt() {
    assert!(native_confirmation_host().is_none());
}

#[rstest]
fn native_prompt_identifies_the_exact_target_without_echoing_action_text() {
    let request = TrustedActionConfirmationRequest {
        schema: TRUSTED_ACTION_CONFIRMATION_SCHEMA.to_owned(),
        request_id: "request-1".into(),
        session_id: "session-1".into(),
        task_grant_id: "grant-1".into(),
        window_capability: "capability-1".into(),
        target_process_id: Some(4242),
        target_window_handle: Some(0x1234),
        observation_id: "observation-1".into(),
        accessibility_state_id: Some("accessibility-1".into()),
        intent: "enter account secret".into(),
        action: json!({"action":"set_text", "text":"super-secret"}),
        request_digest: "sha256:1234567890abcdef".into(),
    };

    let prompt = prompt_text(&request);

    assert!(prompt.contains("PID: 4242"));
    assert!(prompt.contains("HWND: 0x1234"));
    assert!(prompt.contains("Action: set_text"));
    assert!(prompt.contains("Digest: sha256:1234567890abcdef"));
    assert!(!prompt.contains("super-secret"));
}
