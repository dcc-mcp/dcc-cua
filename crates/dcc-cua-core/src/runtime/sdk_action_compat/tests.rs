use super::*;
use rstest::rstest;

fn fixture() -> (ComputerUseSession, ComputerUseObservation) {
    let driver = ComputerUseDriver::create().unwrap();
    let session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(42),
                window_handle: Some(77),
                ..Default::default()
            },
            "test",
            "token-session",
        )
        .unwrap();
    let observation = ComputerUseObservation {
        observation_id: "observation-1".into(),
        window_handle: 77,
        process_id: 42,
        window_title: "test".into(),
        width: 800,
        height: 600,
        source_rect: [100, 200, 800, 600],
        capture_backend: "test".into(),
        capture_provenance: json!({}),
        session_id: "token-session".into(),
    };
    (session, observation)
}

fn indexed_action() -> ComputerUseAction {
    ComputerUseAction {
        action: "click".into(),
        element_index: Some(7),
        ..Default::default()
    }
}

#[rstest]
fn index_translation_keeps_the_actual_snapshot_qualified_token() {
    let (mut session, observation) = fixture();
    session.remember_sdk_element_tokens(
        &json!({"elements":[{"element_index":7,"element_token":"snapshot-a:7"}]}),
        &observation,
    );
    let mapped = session
        .resolve_sdk_element_action(&indexed_action(), &observation)
        .unwrap()
        .unwrap();
    assert_eq!(mapped.element_token.as_deref(), Some("snapshot-a:7"));
    assert_eq!(mapped.element_index, None);
    // Another session publishing the same row cannot change A's saved token.
    let (mut other, mut other_observation) = fixture();
    other_observation.observation_id = "observation-b".into();
    other.remember_sdk_element_tokens(
        &json!({"elements":[{"element_index":7,"element_token":"snapshot-b:7"}]}),
        &other_observation,
    );
    assert_eq!(
        session
            .resolve_sdk_element_action(&indexed_action(), &observation)
            .unwrap()
            .unwrap()
            .element_token,
        mapped.element_token
    );
    assert_eq!(
        other
            .resolve_sdk_element_action(&indexed_action(), &other_observation)
            .unwrap()
            .unwrap()
            .element_token
            .as_deref(),
        Some("snapshot-b:7")
    );
}

#[rstest]
#[case(json!({"elements":[]}))]
#[case(json!({"elements":[{"element_index":7}]}))]
#[case(json!({"elements":[{"element_index":7,"element_token":""}]}))]
#[case(json!({"elements":[{"element_index":7,"element_token":"a"},{"element_index":7,"element_token":"b"}]}))]
#[case(json!({"elements":[{"element_index":7,"element_token":"a"},{"element_index":7,"element_token":"a"}]}))]
fn ambiguous_or_tokenless_rows_fail_closed(#[case] accessibility: Value) {
    let (mut session, observation) = fixture();
    session.remember_sdk_element_tokens(&accessibility, &observation);
    assert_eq!(
        session
            .resolve_sdk_element_action(&indexed_action(), &observation)
            .unwrap_err()
            .code,
        ComputerUseErrorCode::StaleObservation
    );
}

#[rstest]
fn changing_observation_or_invalidating_evidence_revokes_index_translation() {
    let (mut session, mut observation) = fixture();
    session.remember_sdk_element_tokens(
        &json!({"elements":[{"element_index":7,"element_token":"snapshot-a:7"}]}),
        &observation,
    );
    observation.observation_id = "observation-2".into();
    assert_eq!(
        session
            .resolve_sdk_element_action(&indexed_action(), &observation)
            .unwrap_err()
            .code,
        ComputerUseErrorCode::StaleObservation
    );
    observation.observation_id = "observation-1".into();
    session.invalidate_action_observations();
    assert_eq!(
        session
            .resolve_sdk_element_action(&indexed_action(), &observation)
            .unwrap_err()
            .code,
        ComputerUseErrorCode::StaleObservation
    );
}

#[rstest]
fn explicit_token_keeps_the_existing_precedence_over_an_index() {
    let (session, observation) = fixture();
    let mut action = indexed_action();
    action.element_token = Some("explicit-token".into());
    assert!(
        session
            .resolve_sdk_element_action(&action, &observation)
            .unwrap()
            .is_none()
    );
}

#[rstest]
fn overlay_move_scales_screenshot_coordinates_exactly_once_on_native_overlay_adapters() {
    let (_, mut observation) = fixture();
    observation.width = 400;
    observation.height = 300;
    let target = WindowTarget {
        pid: 42,
        window_id: 77,
        title: "test".into(),
        app_name: "test".into(),
        bounds: observation.source_rect,
        is_on_screen: true,
        is_minimized: false,
        z_index: None,
        is_foreground: true,
    };
    let action = ComputerUseAction {
        action: "move".into(),
        x: Some(200.0),
        y: Some(150.0),
        ..Default::default()
    };
    if cfg!(target_os = "macos") {
        assert!(!ComputerUseSession::action_uses_local_window_coordinates(
            &action
        ));
        let args = action_arguments(&action, "session", &target)
            .unwrap()
            .arguments;
        assert_eq!(
            (args["x"].as_f64(), args["y"].as_f64()),
            (Some(200.0), Some(150.0))
        );
        assert_eq!(
            args["target"],
            json!({"kind":"window", "pid":42, "window_id":77})
        );
        assert!(args.get("scope").is_none());
    } else {
        assert!(ComputerUseSession::action_uses_local_window_coordinates(
            &action
        ));
        let local = action_for_window_visual_fallback(&action, &observation).unwrap();
        let args = action_arguments(&local, "session", &target)
            .unwrap()
            .arguments;
        assert_eq!(
            (args["x"].as_f64(), args["y"].as_f64()),
            (Some(500.0), Some(500.0))
        );
        assert_eq!(args["scope"], "window");
        assert!(args.get("target").is_none());
        assert!(args.get("pid").is_none());
        assert!(args.get("window_id").is_none());
    }
}

#[rstest]
#[case("pid")]
#[case("window")]
#[case("session")]
fn row_translation_requires_the_same_observation_owner(#[case] changed: &str) {
    let (mut session, mut observation) = fixture();
    session.remember_sdk_element_tokens(
        &json!({"elements":[{"element_index":7,"element_token":"snapshot-a:7"}]}),
        &observation,
    );
    match changed {
        "pid" => observation.process_id += 1,
        "window" => observation.window_handle += 1,
        "session" => observation.session_id.push_str("-other"),
        _ => unreachable!(),
    }
    assert_eq!(
        session
            .resolve_sdk_element_action(&indexed_action(), &observation)
            .unwrap_err()
            .code,
        ComputerUseErrorCode::StaleObservation
    );
}

#[rstest]
fn already_scaled_visual_overlay_move_uses_screen_points_without_sdk_rescaling() {
    let (_, mut observation) = fixture();
    observation.width = 400;
    observation.height = 300;
    observation.capture_provenance = json!({
        "accessibility_available": false,
        "observation_mode": "live_observation",
    });
    let target = WindowTarget {
        pid: 42,
        window_id: 77,
        title: "test".into(),
        app_name: "test".into(),
        bounds: observation.source_rect,
        is_on_screen: true,
        is_minimized: false,
        z_index: None,
        is_foreground: true,
    };
    let action = ComputerUseAction {
        action: "move".into(),
        x: Some(200.0),
        y: Some(150.0),
        ..Default::default()
    };
    let local_coordinates = observation.capture_provenance["accessibility_available"] == false
        || ComputerUseSession::action_uses_local_window_coordinates(&action);
    assert!(local_coordinates);
    let local = action_for_window_visual_fallback(&action, &observation).unwrap();
    let command =
        ComputerUseSession::sdk_action_command(&local, "session", &target, local_coordinates)
            .unwrap();
    assert_eq!(command.arguments["x"], 500.0);
    assert_eq!(command.arguments["y"], 500.0);
    assert_eq!(command.arguments["scope"], "window");
    assert!(command.arguments.get("target").is_none());
    assert!(command.arguments.get("pid").is_none());
    assert!(command.arguments.get("window_id").is_none());
    assert!(command.arguments.get("delivery_mode").is_none());
}
