use super::*;
use rstest::rstest;

#[rstest]
#[tokio::test]
async fn capture_popup_observation_requires_an_independent_live_bound_lease() {
    let mut host = pixel_input_host(&["keypress"]).await;
    assert!(
        host.require_native_capture_popup_keyboard_grant("session-1")
            .is_err()
    );
    let scope = TrustedTaskActionScope {
        action: "native_capture_popup_keyboard".into(),
        input_kind: "window_state".into(),
        secret_input: false,
        authorization_category: "window_state".into(),
        browser_origin: None,
    };
    assert!(scope.is_native_capture_popup_keyboard());
    assert!(!scope.is_pixels_input());
    host.task_authorization
        .as_mut()
        .unwrap()
        .allowed_actions
        .push(scope);
    host.require_native_capture_popup_keyboard_grant("session-1")
        .unwrap();
    assert!(
        host.require_native_capture_popup_keyboard_grant("other-session")
            .is_err()
    );
    host.observation_mode = crate::TaskObservationMode::Semantic;
    assert!(
        host.require_native_capture_popup_keyboard_grant("session-1")
            .is_err()
    );
}
