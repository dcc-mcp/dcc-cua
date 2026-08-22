use rstest::rstest;

use super::*;

#[rstest]
#[case(ComputerUseErrorCode::InvalidTarget, "invalid_target")]
#[case(ComputerUseErrorCode::TargetMinimized, "target_minimized")]
#[case(ComputerUseErrorCode::TargetUnavailable, "target_unavailable")]
#[case(ComputerUseErrorCode::TargetModalChanged, "target_modal_changed")]
#[case(ComputerUseErrorCode::MissingWindow, "target_unavailable")]
fn target_error_codes_keep_wire_contract(
    #[case] code: ComputerUseErrorCode,
    #[case] expected: &str,
) {
    assert_eq!(
        error_code(&HostError::ComputerUse(ComputerUseError::new(code, "test"))),
        expected
    );
}

#[rstest]
#[case("hard_deny", HostActionSafetyTier::HardDeny)]
#[case("action_confirmation", HostActionSafetyTier::ActionConfirmation)]
#[case("pre_approval", HostActionSafetyTier::PreApproval)]
#[case("task_grant", HostActionSafetyTier::TaskGrant)]
fn semantic_action_safety_comes_from_the_fresh_accessibility_element(
    #[case] published: &str,
    #[case] expected: HostActionSafetyTier,
) {
    let action = HostAction {
        action: "click".into(),
        element_index: Some(7),
        element_token: Some("fresh-token".into()),
        delivery_mode: None,
        input_backend_id: None,
        input_kind: "semantic".into(),
        intent: "ordinary_edit".into(),
        x: None,
        y: None,
        button: None,
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
    let root = json!({"elements": [{
        "element_index": 7,
        "element_token": "fresh-token",
        "policy_tier": published,
    }]});

    assert_eq!(action.safety_tier(Some(&root)), expected);
    assert_eq!(action.safety_tier(None), HostActionSafetyTier::HardDeny);
    assert_eq!(
        action.safety_tier(Some(&json!({"elements": []}))),
        HostActionSafetyTier::HardDeny
    );
}

#[rstest]
fn post_snapshot_delay_is_bounded_and_requires_capture() {
    assert_eq!(post_snapshot_delay(true, 1_500).unwrap().as_millis(), 1_500);
    assert!(post_snapshot_delay(true, MAX_POST_SNAPSHOT_DELAY_MS + 1).is_err());
    assert!(post_snapshot_delay(false, 1).is_err());
}

#[rstest]
fn live_observation_requests_parse() {
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "live_observation_start",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {"fps": 15}
            }
        })),
        Ok(Request::LiveObservationStart { request, .. })
            if request.fps == 15 && request.max_dimension == 1_568
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "live_observation_state",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1"
            }
        })),
        Ok(Request::LiveObservationState { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "live_observation_stop",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1"
            }
        })),
        Ok(Request::LiveObservationStop { .. })
    ));
}

#[rstest]
fn open_session_bootstrap_activation_is_explicit_and_defaults_off() {
    let default_request = serde_json::from_value::<Request>(json!({
        "method": "open_session",
        "params": {
            "session_id": "session-default",
            "grant": {
                "task_grant_id": "task-1",
                "application_label": "Synthetic Test App",
                "process_id": 4242,
                "window_handle": 31337
            }
        }
    }))
    .unwrap();
    assert!(matches!(
        default_request,
        Request::OpenSession {
            activate_before: false,
            indicator_motion: IndicatorMotionPolicy::Auto,
            idle_timeout_ms: DEFAULT_SESSION_IDLE_TIMEOUT_MS,
            ..
        }
    ));

    let bootstrap_request = serde_json::from_value::<Request>(json!({
        "method": "open_session",
        "params": {
            "session_id": "session-bootstrap",
            "activate_before": true,
            "grant": {
                "task_grant_id": "task-1",
                "application_label": "Synthetic Test App",
                "process_id": 4242,
                "window_handle": 31337
            }
        }
    }))
    .unwrap();
    assert!(matches!(
        bootstrap_request,
        Request::OpenSession {
            activate_before: true,
            indicator_motion: IndicatorMotionPolicy::Auto,
            ..
        }
    ));

    let animated_request = serde_json::from_value::<Request>(json!({
        "method": "open_session",
        "params": {
            "session_id": "session-animated",
            "indicator_motion": "animate",
            "grant": {
                "task_grant_id": "task-1",
                "application_label": "Synthetic Test App",
                "process_id": 4242,
                "window_handle": 31337
            }
        }
    }))
    .unwrap();
    assert!(matches!(
        animated_request,
        Request::OpenSession {
            indicator_motion: IndicatorMotionPolicy::Animate,
            ..
        }
    ));
}

#[rstest]
#[case("", "Application")]
#[case(" task-1", "Application")]
#[case("task-1", "")]
#[case("task-1", "Application\nspoof")]
fn grant_identity_is_generic_bounded_and_banner_safe(
    #[case] task_grant_id: &str,
    #[case] application_label: &str,
) {
    let grant: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": task_grant_id,
        "application_label": application_label
    }))
    .unwrap();
    assert!(grant.validate_identity().is_err());
}

#[rstest]
fn grant_identity_rejects_oversized_and_legacy_fields() {
    let oversized: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": "task-1",
        "application_label": "x".repeat(crate::task_grant::MAX_APPLICATION_LABEL_CHARS + 1)
    }))
    .unwrap();
    assert!(oversized.validate_identity().is_err());
    assert!(
        serde_json::from_value::<TaskGrant>(json!({
            "task_grant_id": "task-1",
            "application_label": "Application",
            "dcc_type": "legacy"
        }))
        .is_err()
    );
}

#[rstest]
fn launch_ownership_requires_the_same_grant_and_process() {
    let launched = HostLaunchSession {
        runtime_session_id: "private-launch-session".into(),
        task_grant_id: "task-1".into(),
        application_label: "Unreal Editor".into(),
        process_id: 4242,
    };
    let mut matching: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": "task-1",
        "application_label": "Unreal Editor"
    }))
    .unwrap();
    bind_launched_process(&launched, &mut matching).unwrap();
    assert_eq!(matching.process_id, Some(4242));

    let mut wrong_process: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": "task-1",
        "application_label": "Unreal Editor",
        "process_id": 7
    }))
    .unwrap();
    assert!(bind_launched_process(&launched, &mut wrong_process).is_err());

    let mut wrong_grant: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": "task-2",
        "application_label": "Unreal Editor"
    }))
    .unwrap();
    assert!(bind_launched_process(&launched, &mut wrong_grant).is_err());

    let mut wrong_label: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": "task-1",
        "application_label": "Maya"
    }))
    .unwrap();
    assert!(bind_launched_process(&launched, &mut wrong_label).is_err());
}
