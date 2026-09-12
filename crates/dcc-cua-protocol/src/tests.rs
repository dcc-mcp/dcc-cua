use rstest::rstest;

use super::*;

#[rstest]
fn protocol_limits_are_ordered_and_bounded() {
    const {
        assert!(HOST_PROTOCOL_VERSION == 1);
        assert!(MAX_BINARY_FRAME_BYTES > MAX_JSON_FRAME_BYTES);
        assert!(MAX_HOST_CONNECTIONS > 0);
        assert!(MAX_SESSIONS_PER_CONNECTION > 0);
        assert!(MAX_SESSIONS_PER_CONNECTION <= MAX_HOST_CONNECTIONS);
        assert!(MAX_PARALLEL_DISCOVERY_REQUESTS > 0);
    }
}

#[rstest]
fn request_envelope_owns_shared_id_method_and_params_validation() {
    let envelope = RequestEnvelope::from_value(&serde_json::json!({
        "request_id": "request-1",
        "method": "list_apps",
    }))
    .unwrap();
    assert_eq!(envelope.request_id.as_deref(), Some("request-1"));
    assert_eq!(envelope.method, "list_apps");
    assert_eq!(envelope.params, serde_json::json!({}));

    for invalid in [
        serde_json::json!([]),
        serde_json::json!({"request_id": "", "method": "list_apps"}),
        serde_json::json!({"method": ""}),
        serde_json::json!({"method": "list_apps", "params": []}),
    ] {
        assert!(RequestEnvelope::from_value(&invalid).is_err());
    }
}

#[rstest]
fn method_traits_are_one_closed_cross_component_taxonomy() {
    let action = host_method_traits("execute_action");
    assert!(action.action);
    assert!(!action.pipeline_safe);

    let snapshot = host_method_traits("snapshot");
    assert!(snapshot.standalone_snapshot);
    assert!(snapshot.pipeline_safe);

    let discovery = host_method_traits("list_apps");
    assert!(discovery.parallel_discovery);
    assert!(discovery.pipeline_safe);

    let semantic = host_method_traits("session_health");
    assert!(semantic.semantic_observation);
    assert!(semantic.pipeline_safe);

    assert_eq!(host_method_traits("unknown"), HostMethodTraits::default());
    assert!(host_method_traits("clipboard_capture_secret").action);
}

#[rstest]
fn secret_handles_are_shared_bounded_opaque_identifiers() {
    assert!(validate_secret_handle("edge.api-key").is_ok());
    for invalid in ["", " leading", "contains/slash", "contains space"] {
        assert!(validate_secret_handle(invalid).is_err());
    }
    assert!(validate_secret_handle(&"x".repeat(MAX_SECRET_HANDLE_CHARS + 1)).is_err());
}

#[rstest]
#[tokio::test]
async fn shared_frame_codec_round_trips_and_enforces_one_limit() {
    let (mut client, mut server) = tokio::io::duplex(64);
    let writer = tokio::spawn(async move { write_frame(&mut client, b"frame", 16).await });
    let body = read_frame(&mut server, 16).await.unwrap().unwrap();
    writer.await.unwrap().unwrap();
    assert_eq!(body, b"frame");

    let (mut client, _server) = tokio::io::duplex(64);
    assert!(write_frame(&mut client, b"too long", 4).await.is_err());
}

#[rstest]
fn shared_local_path_shape_is_absolute_bounded_and_nul_free() {
    #[cfg(windows)]
    let absolute = r"C:\temp\artifact.bin";
    #[cfg(unix)]
    let absolute = "/tmp/artifact.bin";

    assert_eq!(
        validate_absolute_local_path(absolute).unwrap(),
        std::path::Path::new(absolute)
    );
    assert!(validate_absolute_local_path("relative.bin").is_err());
    assert!(validate_absolute_local_path("bad\0path").is_err());
    assert!(validate_absolute_local_path(&"x".repeat(MAX_LOCAL_PATH_CHARS + 1)).is_err());
}

#[cfg(windows)]
#[rstest]
fn windows_endpoint_is_local_and_session_scoped() {
    let endpoint = default_endpoint();
    assert!(endpoint.starts_with(r"\\.\pipe\dcc-cua-v1"));
}

#[cfg(unix)]
#[rstest]
fn private_xdg_runtime_directory_owns_the_default_socket() {
    use std::os::unix::fs::PermissionsExt;

    let runtime_dir = std::env::temp_dir().join(format!(
        "dcc-cua-protocol-{}-{}",
        effective_user_id(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir(&runtime_dir);
    std::fs::create_dir(&runtime_dir).unwrap();
    std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700)).unwrap();

    let endpoint = default_unix_endpoint_from(
        Some(runtime_dir.as_os_str()),
        &std::env::temp_dir(),
        effective_user_id(),
    );
    assert_eq!(endpoint, runtime_dir.join(UNIX_SOCKET_NAME));

    std::fs::remove_dir(runtime_dir).unwrap();
}

#[cfg(unix)]
#[rstest]
fn insecure_xdg_runtime_directory_falls_back_to_a_user_namespace() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = std::env::temp_dir();
    let runtime_dir = temp_dir.join(format!(
        "dcc-cua-protocol-insecure-{}-{}",
        effective_user_id(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir(&runtime_dir);
    std::fs::create_dir(&runtime_dir).unwrap();
    std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    let endpoint = default_unix_endpoint_from(
        Some(runtime_dir.as_os_str()),
        &temp_dir,
        effective_user_id(),
    );
    assert_eq!(
        endpoint,
        temp_dir
            .join(format!("dcc-cua-{}", effective_user_id()))
            .join(UNIX_SOCKET_NAME)
    );

    std::fs::remove_dir(runtime_dir).unwrap();
}

fn continuity_target() -> continuity::TargetBinding {
    continuity::TargetBinding {
        process_id: 42,
        window_handle: 7,
    }
}

fn continuity_receipt() -> continuity::ActionReceipt {
    continuity::ActionReceipt {
        receipt_id: "r1".into(),
        action_id: "harvest:plot-1".into(),
        target: continuity_target(),
        completed: true,
        post_observation_id: "obs-1".into(),
        post_action_evidence_epoch: 10,
        transition_fence: "fence-1".into(),
    }
}

fn continuity_next() -> continuity::ObservationFrame {
    continuity::ObservationFrame {
        frame_id: "frame-2".into(),
        observation_id: "obs-2".into(),
        action_evidence_epoch: 11,
        target: continuity_target(),
        parent_frame_id: Some("obs-1".into()),
        image_ref: Some("qq-classic-farm://frame-2".into()),
        semantic_state_id: Some("farm-grid-v2".into()),
    }
}

#[rstest]
fn accepts_chained_farm_observation() {
    assert!(
        continuity::validate_next_observation(&continuity_receipt(), &continuity_next()).is_ok()
    );
}

#[rstest]
fn accepts_multi_action_batch_only_when_each_step_chains() {
    let mut receipt = continuity_receipt();
    for step in 1..=3 {
        let mut frame = continuity_next();
        frame.frame_id = format!("frame-{step}");
        frame.observation_id = format!("obs-{}", step + 1);
        frame.parent_frame_id = Some(receipt.post_observation_id.clone());
        frame.action_evidence_epoch = receipt.post_action_evidence_epoch + 1;
        assert!(continuity::validate_next_observation(&receipt, &frame).is_ok());

        receipt.action_id = format!("harvest:plot-{step}");
        receipt.post_observation_id = frame.observation_id.clone();
        receipt.post_action_evidence_epoch = frame.action_evidence_epoch;
    }
}

#[rstest]
fn rejects_stale_or_unrelated_observation() {
    let mut frame = continuity_next();
    frame.parent_frame_id = Some("obs-0".into());
    assert_eq!(
        continuity::validate_next_observation(&continuity_receipt(), &frame),
        Err(continuity::ChainError::ObservationNotChained)
    );
    frame.parent_frame_id = Some("obs-1".into());
    frame.action_evidence_epoch = 10;
    assert_eq!(
        continuity::validate_next_observation(&continuity_receipt(), &frame),
        Err(continuity::ChainError::EpochNotAdvanced)
    );
    frame.action_evidence_epoch = 12;
    assert_eq!(
        continuity::validate_next_observation(&continuity_receipt(), &frame),
        Err(continuity::ChainError::EpochNotAdvanced)
    );
}

#[rstest]
fn rejects_window_switch() {
    let mut frame = continuity_next();
    frame.target.window_handle = 8;
    assert_eq!(
        continuity::validate_next_observation(&continuity_receipt(), &frame),
        Err(continuity::ChainError::TargetChanged)
    );
}

#[rstest]
fn bounded_batch_rejects_over_limit() {
    let receipt = continuity_receipt();
    let frame = continuity_next();
    assert_eq!(
        continuity::validate_batch(
            continuity::BatchPolicy {
                max_actions: 0,
                abort_on_failure: true
            },
            &frame,
            &[(receipt, frame.clone())]
        ),
        Err(continuity::ChainError::BatchLimitExceeded)
    );
}

#[rstest]
fn state_applies_deltas_without_repeating_completed_targets() {
    let frame = continuity_next();
    let mut state = continuity::ContinuityState::default();
    state.apply_observation(
        &frame,
        continuity::ObservationDelta {
            added_candidates: vec!["plot-1".into(), "plot-2".into()],
            removed_candidates: Vec::new(),
            changed: true,
        },
    );
    state.mark_completed("plot-1".into());
    state.apply_observation(
        &frame,
        continuity::ObservationDelta {
            added_candidates: vec!["plot-1".into(), "plot-3".into()],
            removed_candidates: vec!["plot-2".into()],
            changed: true,
        },
    );
    assert_eq!(state.pending_candidates, vec!["plot-3"]);
    assert_eq!(state.completed_targets, vec!["plot-1"]);
}

#[rstest]
fn metrics_accumulate_token_usage_safely() {
    let mut metrics = continuity::ContinuityMetrics::default();
    metrics.record_model_call(120, 30);
    metrics.record_model_call(80, 20);
    assert_eq!(metrics.model_calls, 2);
    assert_eq!(metrics.total_tokens(), 250);
}
