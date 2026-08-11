use std::path::Path;

use rstest::rstest;

use super::*;

async fn attach_test_showcase(
    session: &mut ComputerUseSession,
    output_dir: &Path,
) -> tokio::sync::watch::Sender<crate::live_observation::LiveObservationStatus> {
    let mut status = crate::live_observation::LiveObservationStatus::default();
    status.publish_frame(
        crate::live_observation::LiveObservationFrame::new(
            1,
            vec![1; 16 * 16 * 4],
            16,
            16,
            std::time::Instant::now(),
        ),
        Duration::ZERO,
        "test_capture",
    );
    let (status_sender, receiver) = tokio::sync::watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, output_dir.to_str().unwrap(), 10)
        .await
        .expect("start test showcase recorder");
    session.showcase = Some(ActiveShowcase {
        recorder,
        owns_live_observation: false,
    });
    session.recording_active = true;
    session.recording_expected_video = true;
    let health = RecordingHealth::new(session.session_id.as_str());
    assert!(health.observe_trajectory(&json!({
        "enabled": true,
        "owner": session.session_id,
    })));
    session.recording_health = Some(health);
    status_sender
}

#[rstest]
#[tokio::test]
async fn recording_state_recovers_finalized_video_after_stop_response_is_lost() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-session-stop-{}", uuid::Uuid::new_v4()));
    let (mut session, _) = counting_session();
    let _status_sender = attach_test_showcase(&mut session, &directory).await;

    let discarded_stop_response = session.recording_stop().await.expect("stop recording");
    assert_eq!(discarded_stop_response["video"]["finalized"], true);

    let recovered = session
        .recording_state()
        .await
        .expect("recover terminal recording state");
    assert_eq!(recovered["status"], "stopped");
    assert_eq!(recovered["video"], discarded_stop_response["video"]);

    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn an_accepted_next_recording_clears_previous_terminal_video_evidence() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-session-restart-{}", uuid::Uuid::new_v4()));
    let (mut session, _) = counting_session();
    let _status_sender = attach_test_showcase(&mut session, &directory).await;
    session
        .recording_stop()
        .await
        .expect("stop first recording");
    session.last_upstream_session_refresh = Some(Instant::now());

    session
        .recording_start_after_target_validation(&ComputerUseRecordingStartRequest {
            output_dir: directory.to_string_lossy().into_owned(),
            record_video: false,
        })
        .await
        .expect("accept next recording");
    let restarted = session
        .recording_state()
        .await
        .expect("read restarted recording state");

    assert_eq!(restarted["status"], "active");
    assert_eq!(restarted["expected_components"], json!(["trajectory"]));
    assert_eq!(restarted["video"], Value::Null);

    session.recording_stop().await.expect("stop next recording");
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn a_failed_video_restart_preserves_previous_terminal_video_evidence() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-session-restart-{}", uuid::Uuid::new_v4()));
    let (mut session, _, names) = counting_session_with_names();
    let _status_sender = attach_test_showcase(&mut session, &directory).await;
    let stopped = session
        .recording_stop()
        .await
        .expect("stop first recording");
    let previous_video = stopped["video"].clone();
    names.lock().expect("clear prior tool names").clear();
    session.scope.window_handle = None;
    session.last_upstream_session_refresh = Some(Instant::now());

    let restart_error = session
        .recording_start_after_target_validation(&ComputerUseRecordingStartRequest {
            output_dir: directory.to_string_lossy().into_owned(),
            record_video: true,
        })
        .await
        .expect_err("existing finalized video must reject the restart");
    assert!(matches!(
        restart_error.code,
        ComputerUseErrorCode::MissingWindow | ComputerUseErrorCode::CaptureFailed
    ));

    let recovered = session
        .recording_state()
        .await
        .expect("recover previous terminal recording state");
    assert_eq!(recovered["status"], "stopped");
    assert_eq!(recovered["video"], previous_video);
    let names = names.lock().expect("read restart tool names");
    assert_eq!(names.first().map(String::as_str), Some("start_recording"));
    assert!(names.iter().any(|name| name == "stop_recording"));
    assert_eq!(
        names.last().map(String::as_str),
        Some("get_recording_state")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn session_stop_reports_recording_finalize_failure_as_a_typed_cleanup_issue() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-session-cleanup-{}", uuid::Uuid::new_v4()));
    let (mut session, _, names) = counting_session_with_names();
    let _status_sender = attach_test_showcase(&mut session, &directory).await;
    std::fs::create_dir(directory.join("showcase.mp4"))
        .expect("inject a final-segment rename failure");

    let stopped: ComputerUseSessionStopResult = session
        .stop()
        .await
        .expect("return bounded cleanup outcome");

    assert!(!stopped.success);
    assert!(!stopped.active);
    assert!(!stopped.cleanup_pending);
    assert_eq!(stopped.cleanup_issues.len(), 1);
    assert_eq!(
        stopped.cleanup_issues[0].phase,
        ComputerUseCleanupPhase::RecordingStop
    );
    assert_eq!(
        stopped.cleanup_issues[0].code,
        ComputerUseErrorCode::CaptureFailed
    );
    assert!(
        !stopped.cleanup_issues[0].message.is_empty(),
        "cleanup issue must preserve the finalize error"
    );
    assert_eq!(
        *names.lock().expect("read cleanup tool names"),
        ["get_recording_state", "stop_recording", "end_session"]
    );

    std::fs::remove_dir_all(directory).unwrap();
}
