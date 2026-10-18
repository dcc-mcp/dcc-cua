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
