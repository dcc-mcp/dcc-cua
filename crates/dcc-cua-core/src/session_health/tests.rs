use super::*;
use crate::{
    ActionEvidenceEpoch, ComputerUseInputStatus, ComputerUseInputTarget,
    ComputerUseSessionInputState, ComputerUseSessionTargetState, ComputerUseTargetStatus,
};
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

#[rstest]
fn ready_foreground_target_and_advancing_healthy_recording_are_safe_to_preflight() {
    let health = ComputerUseSessionHealth::evaluate(healthy_evaluation());

    assert!(health.safe_to_input);
    assert!(health.blockers.is_empty());
    assert_eq!(health.authority, "preflight_only");
    assert!(!health.automatic_activation);
    assert!(!health.automatic_input);
    assert!(health.fresh_observation_required);
    assert_eq!(health.transition_sequence, 5);
    assert!(!health.action_evidence_epoch.is_empty());
}

#[rstest]
fn suspended_input_is_a_typed_preflight_blocker() {
    let mut evaluation = healthy_evaluation();
    evaluation.input_state.status = ComputerUseInputStatus::Suspended;
    evaluation.input_state.code = "interactive_desktop_unavailable".into();
    evaluation.input_state.reason = Some("desktop disconnected".into());

    let health = ComputerUseSessionHealth::evaluate(evaluation);

    assert!(!health.safe_to_input);
    assert_eq!(
        health.blockers,
        vec![ComputerUseSessionHealthBlocker::InputSuspended]
    );
}

#[rstest]
fn foreign_foreground_is_a_typed_preflight_blocker() {
    let mut evaluation = healthy_evaluation();
    evaluation.target_state.foreground = false;

    let health = ComputerUseSessionHealth::evaluate(evaluation);

    assert!(!health.safe_to_input);
    assert_eq!(
        health.blockers,
        vec![ComputerUseSessionHealthBlocker::TargetNotForeground]
    );
}

#[rstest]
fn minimized_target_is_a_typed_preflight_blocker() {
    let mut evaluation = healthy_evaluation();
    evaluation.target_state.status = ComputerUseTargetStatus::Minimized;
    evaluation.target_state.code = "target_minimized".into();
    evaluation.target_state.visible = false;
    evaluation.target_state.minimized = true;
    evaluation.target_state.foreground = false;

    let health = ComputerUseSessionHealth::evaluate(evaluation);

    assert!(!health.safe_to_input);
    assert_eq!(
        health.blockers,
        vec![ComputerUseSessionHealthBlocker::TargetMinimized]
    );
}

#[rstest]
fn missing_target_is_a_typed_preflight_blocker() {
    let mut evaluation = healthy_evaluation();
    evaluation.target_state.status = ComputerUseTargetStatus::Unavailable;
    evaluation.target_state.code = "target_unavailable".into();
    evaluation.target_state.visible = false;
    evaluation.target_state.foreground = false;

    let health = ComputerUseSessionHealth::evaluate(evaluation);

    assert!(!health.safe_to_input);
    assert_eq!(
        health.blockers,
        vec![ComputerUseSessionHealthBlocker::TargetUnavailable]
    );
}

#[rstest]
fn recording_state_projects_partial_size_and_mtime_without_exposing_its_path() {
    let partial_path = std::env::temp_dir().join(format!(
        "dcc-cua-session-health-{}.partial.mp4",
        Uuid::new_v4()
    ));
    std::fs::write(&partial_path, b"four").expect("write partial recording fixture");
    let raw = json!({
        "status": "active",
        "healthy": true,
        "issues": [],
        "trajectory": {"structuredContent": {"next_turn": 7}},
        "video": {
            "active": true,
            "current_partial": partial_path,
            "segments": [{"finalized": true}, {"finalized": false}]
        }
    });

    let recording = ComputerUseRecordingHealth::from_state(
        true,
        &raw,
        &ComputerUseSessionHealthPolicy::default(),
    );

    std::fs::remove_file(&partial_path).expect("remove partial recording fixture");
    assert_eq!(recording.status, ComputerUseRecordingStatus::Active);
    assert_eq!(recording.healthy, Some(true));
    assert_eq!(
        recording.progress_status,
        ComputerUseRecordingProgressStatus::NotRequested
    );
    let progress = recording.progress.expect("recording progress fingerprint");
    assert_eq!(progress.lane, ComputerUseRecordingProgressLane::Video);
    assert_eq!(progress.trajectory_turn, Some(7));
    assert_eq!(progress.finalized_segments, 1);
    assert_eq!(progress.current_partial_size_bytes, Some(4));
    assert!(
        progress
            .current_partial_modified_at_unix_ms
            .is_some_and(|modified| modified > 0)
    );
    assert!(
        !serde_json::to_string(&progress)
            .expect("serialize progress fingerprint")
            .contains("partial.mp4")
    );
}

#[rstest]
fn recording_progress_compares_current_partial_and_finalized_segments() {
    let partial_path = std::env::temp_dir().join(format!(
        "dcc-cua-session-health-progress-{}.partial.mp4",
        Uuid::new_v4()
    ));
    std::fs::write(&partial_path, b"progress").expect("write partial recording fixture");
    let raw = json!({
        "status": "active",
        "healthy": true,
        "issues": [],
        "trajectory": {"structuredContent": {"next_turn": 7}},
        "video": {
            "active": true,
            "current_partial": partial_path,
            "segments": [{"finalized": true}]
        }
    });
    let current = ComputerUseRecordingHealth::from_state(
        true,
        &raw,
        &ComputerUseSessionHealthPolicy::default(),
    )
    .progress
    .expect("current recording fingerprint");
    let baseline = ComputerUseRecordingHealth::from_state(
        true,
        &raw,
        &ComputerUseSessionHealthPolicy {
            require_recording_progress: true,
            ..ComputerUseSessionHealthPolicy::default()
        },
    );
    let stalled = ComputerUseRecordingHealth::from_state(
        true,
        &raw,
        &ComputerUseSessionHealthPolicy {
            require_recording_progress: true,
            previous_recording_progress: Some(current.clone()),
            ..ComputerUseSessionHealthPolicy::default()
        },
    );
    let mut older = current.clone();
    older.current_partial_size_bytes = older.current_partial_size_bytes.map(|size| size - 1);
    let advanced = ComputerUseRecordingHealth::from_state(
        true,
        &raw,
        &ComputerUseSessionHealthPolicy {
            require_recording_progress: true,
            previous_recording_progress: Some(older),
            ..ComputerUseSessionHealthPolicy::default()
        },
    );
    let mut future = current;
    future.finalized_segments += 1;
    let regressed = ComputerUseRecordingHealth::from_state(
        true,
        &raw,
        &ComputerUseSessionHealthPolicy {
            require_recording_progress: true,
            previous_recording_progress: Some(future),
            ..ComputerUseSessionHealthPolicy::default()
        },
    );

    std::fs::remove_file(&partial_path).expect("remove partial recording fixture");
    assert_eq!(
        baseline.progress_status,
        ComputerUseRecordingProgressStatus::BaselineRequired
    );
    assert_eq!(
        stalled.progress_status,
        ComputerUseRecordingProgressStatus::Stalled
    );
    assert_eq!(
        advanced.progress_status,
        ComputerUseRecordingProgressStatus::Advanced
    );
    assert_eq!(
        regressed.progress_status,
        ComputerUseRecordingProgressStatus::Regressed
    );
}

#[rstest]
fn video_lane_stall_is_not_hidden_by_trajectory_progress() {
    let previous = ComputerUseRecordingProgressFingerprint {
        lane: ComputerUseRecordingProgressLane::Video,
        trajectory_turn: Some(7),
        finalized_segments: 2,
        current_partial_size_bytes: Some(1_000),
        current_partial_modified_at_unix_ms: Some(80),
    };
    let current = ComputerUseRecordingProgressFingerprint {
        lane: ComputerUseRecordingProgressLane::Video,
        trajectory_turn: Some(8),
        ..previous.clone()
    };

    assert_eq!(
        compare_recording_progress(&previous, &current),
        ComputerUseRecordingProgressStatus::Stalled
    );
}

#[rstest]
fn trajectory_only_recording_uses_trajectory_progress() {
    let previous = ComputerUseRecordingProgressFingerprint {
        lane: ComputerUseRecordingProgressLane::Trajectory,
        trajectory_turn: Some(7),
        finalized_segments: 9,
        current_partial_size_bytes: Some(1_000),
        current_partial_modified_at_unix_ms: Some(80),
    };
    let current = ComputerUseRecordingProgressFingerprint {
        lane: ComputerUseRecordingProgressLane::Trajectory,
        trajectory_turn: Some(8),
        finalized_segments: 0,
        current_partial_size_bytes: None,
        current_partial_modified_at_unix_ms: None,
    };

    assert_eq!(
        compare_recording_progress(&previous, &current),
        ComputerUseRecordingProgressStatus::Advanced
    );
}

#[rstest]
#[case(
    false,
    ComputerUseRecordingStatus::NotGranted,
    Some(true),
    ComputerUseRecordingProgressStatus::Advanced,
    ComputerUseSessionHealthBlocker::RecordingNotGranted
)]
#[case(
    true,
    ComputerUseRecordingStatus::Stopped,
    Some(true),
    ComputerUseRecordingProgressStatus::Advanced,
    ComputerUseSessionHealthBlocker::RecordingInactive
)]
#[case(
    true,
    ComputerUseRecordingStatus::Paused,
    Some(false),
    ComputerUseRecordingProgressStatus::Advanced,
    ComputerUseSessionHealthBlocker::RecordingPaused
)]
#[case(
    true,
    ComputerUseRecordingStatus::Degraded,
    Some(false),
    ComputerUseRecordingProgressStatus::Advanced,
    ComputerUseSessionHealthBlocker::RecordingDegraded
)]
#[case(
    true,
    ComputerUseRecordingStatus::Unavailable,
    None,
    ComputerUseRecordingProgressStatus::Advanced,
    ComputerUseSessionHealthBlocker::RecordingProbeFailed
)]
#[case(
    true,
    ComputerUseRecordingStatus::Active,
    Some(true),
    ComputerUseRecordingProgressStatus::BaselineRequired,
    ComputerUseSessionHealthBlocker::RecordingProgressBaselineRequired
)]
#[case(
    true,
    ComputerUseRecordingStatus::Active,
    Some(true),
    ComputerUseRecordingProgressStatus::Stalled,
    ComputerUseSessionHealthBlocker::RecordingProgressStalled
)]
#[case(
    true,
    ComputerUseRecordingStatus::Active,
    Some(true),
    ComputerUseRecordingProgressStatus::Regressed,
    ComputerUseSessionHealthBlocker::RecordingProgressRegressed
)]
#[case(
    true,
    ComputerUseRecordingStatus::Active,
    Some(true),
    ComputerUseRecordingProgressStatus::Unavailable,
    ComputerUseSessionHealthBlocker::RecordingProgressUnavailable
)]
fn required_recording_failures_have_one_stable_typed_blocker(
    #[case] granted: bool,
    #[case] status: ComputerUseRecordingStatus,
    #[case] healthy: Option<bool>,
    #[case] progress_status: ComputerUseRecordingProgressStatus,
    #[case] expected: ComputerUseSessionHealthBlocker,
) {
    let mut evaluation = healthy_evaluation();
    evaluation.recording.granted = granted;
    evaluation.recording.status = status;
    evaluation.recording.healthy = healthy;
    evaluation.recording.progress_status = progress_status;

    let health = ComputerUseSessionHealth::evaluate(evaluation);

    assert!(!health.safe_to_input);
    assert_eq!(health.blockers, vec![expected]);
}

#[rstest]
fn stopped_recording_does_not_block_the_compatible_default_policy() {
    let mut evaluation = healthy_evaluation();
    evaluation.policy = ComputerUseSessionHealthPolicy::default();
    evaluation.recording.status = ComputerUseRecordingStatus::Stopped;
    evaluation.recording.healthy = Some(true);
    evaluation.recording.progress_status = ComputerUseRecordingProgressStatus::NotRequested;

    let health = ComputerUseSessionHealth::evaluate(evaluation);

    assert!(health.safe_to_input);
    assert!(health.blockers.is_empty());
}

#[rstest]
fn requiring_progress_also_requires_an_active_healthy_recording() {
    let mut evaluation = healthy_evaluation();
    evaluation.policy.require_recording_healthy = false;
    evaluation.recording.status = ComputerUseRecordingStatus::Degraded;
    evaluation.recording.healthy = Some(false);
    evaluation.recording.progress_status = ComputerUseRecordingProgressStatus::Advanced;

    let health = ComputerUseSessionHealth::evaluate(evaluation);

    assert!(!health.safe_to_input);
    assert_eq!(
        health.blockers,
        vec![ComputerUseSessionHealthBlocker::RecordingDegraded]
    );
}

#[rstest]
#[case(true, false, ComputerUseSessionHealthBlocker::TargetProbeFailed)]
#[case(false, true, ComputerUseSessionHealthBlocker::SessionInterrupted)]
fn failed_probes_and_interrupts_have_typed_blockers(
    #[case] target_probe_failed: bool,
    #[case] interrupted: bool,
    #[case] expected: ComputerUseSessionHealthBlocker,
) {
    let mut evaluation = healthy_evaluation();
    evaluation.target_probe_failed = target_probe_failed;
    evaluation.interrupted = interrupted;

    let health = ComputerUseSessionHealth::evaluate(evaluation);

    assert!(!health.safe_to_input);
    assert_eq!(health.blockers, vec![expected]);
}

#[rstest]
fn blockers_have_a_stable_cross_domain_order() {
    let mut evaluation = healthy_evaluation();
    evaluation.state_changed_during_probe = true;
    evaluation.input_state.status = ComputerUseInputStatus::Suspended;
    evaluation.target_state.status = ComputerUseTargetStatus::Unavailable;
    evaluation.target_state.visible = false;
    evaluation.target_state.foreground = false;
    evaluation.target_probe_failed = true;
    evaluation.interrupted = true;
    evaluation.recording.progress_status = ComputerUseRecordingProgressStatus::Stalled;

    let health = ComputerUseSessionHealth::evaluate(evaluation);

    assert_eq!(
        health.blockers,
        vec![
            ComputerUseSessionHealthBlocker::StateChangedDuringProbe,
            ComputerUseSessionHealthBlocker::InputSuspended,
            ComputerUseSessionHealthBlocker::TargetUnavailable,
            ComputerUseSessionHealthBlocker::TargetProbeFailed,
            ComputerUseSessionHealthBlocker::SessionInterrupted,
            ComputerUseSessionHealthBlocker::RecordingProgressStalled,
        ]
    );
}

#[rstest]
fn action_evidence_receipt_changes_with_its_epoch() {
    let first = ComputerUseSessionHealth::evaluate(healthy_evaluation());
    let mut later_evaluation = healthy_evaluation();
    later_evaluation.action_evidence_epoch = ActionEvidenceEpoch::default().advanced();
    let later = ComputerUseSessionHealth::evaluate(later_evaluation);

    assert_ne!(first.action_evidence_epoch, later.action_evidence_epoch);
}

#[rstest]
fn state_change_during_probe_is_a_typed_retry_blocker() {
    let mut evaluation = healthy_evaluation();
    evaluation.state_changed_during_probe = true;

    let health = ComputerUseSessionHealth::evaluate(evaluation);

    assert!(!health.safe_to_input);
    assert_eq!(
        health.blockers,
        vec![ComputerUseSessionHealthBlocker::StateChangedDuringProbe]
    );
}

fn healthy_evaluation() -> ComputerUseSessionHealthEvaluation {
    let target = ComputerUseInputTarget {
        session_id: "session-1".into(),
        process_id: 42,
        window_handle: 77,
    };
    let input_state = ComputerUseSessionInputState {
        status: ComputerUseInputStatus::Ready,
        code: "interactive_desktop_ready".into(),
        reason: None,
        observed_at: 100,
        sequence: 4,
        target: target.clone(),
    };
    let target_state = ComputerUseSessionTargetState {
        status: ComputerUseTargetStatus::Available,
        code: "target_available".into(),
        visible: true,
        minimized: false,
        foreground: true,
        observed_at: 100,
        sequence: 5,
        target,
    };
    let previous = ComputerUseRecordingProgressFingerprint {
        lane: ComputerUseRecordingProgressLane::Video,
        trajectory_turn: Some(9),
        finalized_segments: 2,
        current_partial_size_bytes: Some(1_000),
        current_partial_modified_at_unix_ms: Some(80),
    };
    let current = ComputerUseRecordingProgressFingerprint {
        lane: ComputerUseRecordingProgressLane::Video,
        trajectory_turn: Some(10),
        finalized_segments: 2,
        current_partial_size_bytes: Some(2_000),
        current_partial_modified_at_unix_ms: Some(90),
    };
    ComputerUseSessionHealthEvaluation {
        policy: ComputerUseSessionHealthPolicy {
            require_recording_healthy: true,
            require_recording_progress: true,
            previous_recording_progress: Some(previous),
        },
        input_state,
        target_state,
        recording: ComputerUseRecordingHealth {
            granted: true,
            status: ComputerUseRecordingStatus::Active,
            healthy: Some(true),
            issues: Vec::new(),
            progress: Some(current),
            progress_status: ComputerUseRecordingProgressStatus::Advanced,
        },
        action_evidence_epoch: ActionEvidenceEpoch::default(),
        transition_sequence: 5,
        state_changed_during_probe: false,
        interrupted: false,
        target_probe_failed: false,
    }
}
