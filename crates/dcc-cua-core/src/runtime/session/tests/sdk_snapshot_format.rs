use super::*;
use rstest::rstest;

fn assert_full_observation(snapshot: &Value) {
    let text = snapshot["tree_markdown"]
        .as_str()
        .expect("display-only text");
    for expected in ["menu_action=none", "render_status=idle", "Save changes?"] {
        assert!(
            text.contains(expected),
            "missing read-only observation: {snapshot}"
        );
    }
    assert_eq!(snapshot["snapshot_id"], "sdk-snapshot");
    assert_eq!(snapshot["elements"][0]["element_token"], "sdk-snapshot:1");
    assert_eq!(snapshot["elements"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["elements_complete"], false);
    assert_eq!(snapshot["truncated"], false);
}

#[rstest]
#[tokio::test]
async fn semantic_screenshot_preserves_display_text_and_actionable_sdk_elements() {
    let (mut session, _, names) = counting_session_with_names();
    session.live_observation = None;

    let screenshot = session
        .screenshot_with_bounds(128, 16)
        .await
        .expect("semantic screenshot");

    assert_full_observation(&screenshot.accessibility);
    let text_only_row = ComputerUseAction {
        action: "click".into(),
        element_index: Some(9),
        observation_id: Some(screenshot.observation.observation_id.clone()),
        ..Default::default()
    };
    assert_eq!(
        session
            .resolve_sdk_element_action(&text_only_row, &screenshot.observation)
            .unwrap_err()
            .code,
        ComputerUseErrorCode::StaleObservation,
    );
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
    assert_full_observation(&result);
    let observation = session.observation.clone().unwrap();
    assert_eq!(observation.capture_provenance["pixels_captured"], false);
    let action = ComputerUseAction {
        action: "click".into(),
        element_index: Some(1),
        observation_id: Some(observation.observation_id.clone()),
        ..Default::default()
    };
    let text_only_row = ComputerUseAction {
        element_index: Some(9),
        ..action.clone()
    };
    assert_eq!(
        session
            .resolve_sdk_element_action(&text_only_row, &observation)
            .unwrap_err()
            .code,
        ComputerUseErrorCode::StaleObservation,
        "read-only text must never manufacture an actionable SDK token",
    );
    session.perform_action(&action).await.unwrap();
    assert!(names.lock().unwrap().iter().any(|name| name == "click"));
    assert!(session.sdk_element_tokens.is_none());
}

#[rstest]
#[case(false, false, ComputerUseErrorCode::InvalidAction)]
#[case(true, false, ComputerUseErrorCode::StaleObservation)]
#[case(false, true, ComputerUseErrorCode::StaleObservation)]
#[tokio::test]
async fn tokenless_visual_actions_keep_terminal_refusal_and_observation_fences(
    #[case] accessibility_available: bool,
    #[case] stale_observation: bool,
    #[case] expected: ComputerUseErrorCode,
) {
    let (mut session, _, names) = counting_session_with_names();
    session.live_observation = None;
    session.screenshot_with_bounds(128, 16).await.unwrap();
    session.sdk_element_tokens = None;
    let observation = session.observation.as_mut().unwrap();
    observation.capture_provenance = json!({
        "accessibility_available": accessibility_available,
        "pixels_captured": true,
    });
    let action = ComputerUseAction {
        action: "click".into(),
        element_index: Some(1),
        observation_id: Some(if stale_observation {
            "old-observation".into()
        } else {
            observation.observation_id.clone()
        }),
        ..Default::default()
    };
    let error = session.perform_action(&action).await.unwrap_err();
    assert_eq!(error.code, expected);
    if !accessibility_available && !stale_observation {
        assert!(
            error
                .message
                .contains("visual fallback does not support semantic")
        );
    }
    assert!(!names.lock().unwrap().iter().any(|name| name == "click"));
    assert!(session.observation.is_some(), "no mutation was dispatched");
}
