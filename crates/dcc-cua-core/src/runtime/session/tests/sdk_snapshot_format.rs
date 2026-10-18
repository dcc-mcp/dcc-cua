use super::*;
use rstest::rstest;

#[rstest]
#[tokio::test]
async fn semantic_screenshot_requests_structured_elements_from_sdk() {
    let (mut session, _, names) = counting_session_with_names();
    session.live_observation = None;

    let screenshot = session
        .screenshot_with_bounds(128, 16)
        .await
        .expect("semantic screenshot");

    assert_eq!(
        screenshot.accessibility["elements"][0]["label"],
        "txt-input"
    );
    assert_eq!(
        screenshot.observation.capture_provenance["accessibility_available"],
        true
    );
    assert_eq!(
        (screenshot.observation.width, screenshot.observation.height),
        (1, 1)
    );
    assert!(
        names
            .lock()
            .unwrap()
            .iter()
            .any(|name| name == "get_window_state")
    );
}

#[rstest]
#[tokio::test]
async fn sdk_snapshot_binds_index_translation_to_its_actual_token() {
    let (mut session, _, names) = counting_session_with_names();
    session.live_observation = None;
    let screenshot = session.screenshot_with_bounds(128, 16).await.unwrap();
    let action = ComputerUseAction {
        action: "click".into(),
        element_index: Some(1),
        observation_id: Some(screenshot.observation.observation_id.clone()),
        ..Default::default()
    };
    let resolved = session
        .resolve_sdk_element_action(&action, &screenshot.observation)
        .unwrap()
        .unwrap();
    assert_eq!(resolved.element_token.as_deref(), Some("sdk-snapshot:1"));
    session.perform_action(&action).await.unwrap();
    assert!(names.lock().unwrap().iter().any(|name| name == "click"));
    assert!(
        session.sdk_element_tokens.is_none(),
        "dispatched action must revoke its row translation"
    );
}

#[cfg(not(windows))]
#[rstest]
#[tokio::test]
async fn accessibility_only_snapshot_binds_index_translation_without_pixels() {
    let (mut session, _, names) = counting_session_with_names();
    session.live_observation = None;
    let result = session.accessibility_snapshot(128, 16).await.unwrap();
    assert_eq!(result["elements"][0]["element_token"], "sdk-snapshot:1");
    let observation = session.observation.clone().unwrap();
    assert_eq!(observation.capture_provenance["pixels_captured"], false);
    let action = ComputerUseAction {
        action: "click".into(),
        element_index: Some(1),
        observation_id: Some(observation.observation_id.clone()),
        ..Default::default()
    };
    session.perform_action(&action).await.unwrap();
    assert!(names.lock().unwrap().iter().any(|name| name == "click"));
    assert!(session.sdk_element_tokens.is_none());
}
