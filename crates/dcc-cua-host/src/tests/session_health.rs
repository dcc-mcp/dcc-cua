use rstest::rstest;

use super::*;

#[rstest]
fn retries_when_evidence_or_transition_epoch_changes_during_probe() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut host = cached_host_session(&driver);
    let evidence_before = host.session.action_evidence_epoch();
    let sequence_before = host.input_events.latest_sequence();

    assert!(!request_handler::session_health_state_changed(
        evidence_before,
        sequence_before,
        evidence_before,
        sequence_before,
    ));
    host.session.invalidate_action_observations();
    assert!(request_handler::session_health_state_changed(
        evidence_before,
        sequence_before,
        host.session.action_evidence_epoch(),
        sequence_before,
    ));
    assert!(request_handler::session_health_state_changed(
        evidence_before,
        sequence_before,
        evidence_before,
        sequence_before + 1,
    ));
}

#[rstest]
#[tokio::test]
async fn returns_one_atomic_preflight_without_activating_or_input() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = ConnectionSessions::default();
    sessions
        .windows
        .insert("session-1".into(), cached_host_session(&driver));
    let mut snapshot_transport = Some(SnapshotTransport::BinaryFrame);
    let mut desktop_shared_image = None;
    let cancellation_registry = Arc::new(Mutex::new(HashMap::new()));

    let (response, attachment) = handle_request(
        &driver,
        &mut sessions,
        &mut snapshot_transport,
        &mut desktop_shared_image,
        &cancellation_registry,
        serde_json::from_value(json!({
            "method": "session_health",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "grant-1",
                "window_capability": "capability-1"
            }
        }))
        .unwrap(),
    )
    .await
    .expect("session health response");

    assert!(attachment.is_none());
    assert_eq!(response["type"], "session_health");
    assert_eq!(response["session_id"], "session-1");
    assert_eq!(response["health"]["authority"], "preflight_only");
    assert_eq!(response["health"]["automatic_activation"], false);
    assert_eq!(response["health"]["automatic_input"], false);
    assert_eq!(response["health"]["fresh_observation_required"], true);
    assert_eq!(
        response["health"]["target_state"]["target"]["process_id"],
        42
    );
    assert_eq!(
        response["health"]["target_state"]["target"]["window_handle"],
        77
    );
    assert!(response["health"]["action_evidence_epoch"].is_string());
    assert_eq!(
        response["health"]["transition_sequence"],
        sessions.windows["session-1"].input_events.latest_sequence()
    );
    assert!(
        response["health"]["blockers"]
            .as_array()
            .is_some_and(|blockers| blockers.contains(&json!("target_probe_failed")))
    );
}

#[rstest]
#[tokio::test]
async fn returns_interrupted_and_required_recording_blockers() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = ConnectionSessions::default();
    let mut host = cached_host_session(&driver);
    host.interrupted = true;
    sessions.windows.insert("session-1".into(), host);
    let mut snapshot_transport = Some(SnapshotTransport::BinaryFrame);
    let mut desktop_shared_image = None;
    let cancellation_registry = Arc::new(Mutex::new(HashMap::new()));

    let (response, _) = handle_request(
        &driver,
        &mut sessions,
        &mut snapshot_transport,
        &mut desktop_shared_image,
        &cancellation_registry,
        serde_json::from_value(json!({
            "method": "session_health",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "grant-1",
                "window_capability": "capability-1",
                "policy": {"require_recording_healthy": true}
            }
        }))
        .unwrap(),
    )
    .await
    .expect("blocked session health response");

    let blockers = response["health"]["blockers"]
        .as_array()
        .expect("typed health blockers");
    assert_eq!(response["health"]["safe_to_input"], false);
    assert!(blockers.contains(&json!("session_interrupted")));
    assert!(blockers.contains(&json!("recording_not_granted")));
}
