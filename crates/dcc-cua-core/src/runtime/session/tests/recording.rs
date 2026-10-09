use std::path::Path;

use rstest::rstest;

use super::*;

#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn prepared_source_replacement_pending_cannot_begin_capture(#[case] source_pending: bool) {
    let (mut session, calls) = counting_session();
    session.live_observation = None;
    session.pixel_observation_route = Some(PixelObservationRoute::ExplicitPixelsOnly);
    session.local_cleanup.source_pending = source_pending;
    session.local_cleanup.recorder_pending = !source_pending;
    let request = dcc_cua_protocol::capture_preparation::CapturePreparationBeginRequest {
        window_state_id: "fresh-state".into(),
        lifetime_ms: 1000,
    };
    let authorization = dcc_cua_protocol::capture_preparation::CapturePreparationAuthorization {
        journal_directory: "unused-before-native-entry".into(),
        max_lifetime_ms: 30000,
    };
    let error = session
        .capture_preparation_begin(&request, &authorization, u64::MAX)
        .await
        .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::CompletionUnknown);
    assert!(session.live_observation.is_none());
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[tokio::test]
async fn prepared_source_replacement_cancel_keeps_owned_cleanup_pending() {
    let (mut session, calls) = counting_session();
    let (source, entered, release) = LiveObservation::from_test_shutdown_gate();
    session.live_observation = Some(source);
    {
        let replacement = session.stop_live_observation_for_replacement();
        tokio::pin!(replacement);
        tokio::select! {
            result = entered => result.unwrap(),
            result = &mut replacement => panic!("shutdown gate unexpectedly completed: {result:?}"),
        }
    }
    assert!(session.live_observation.is_none());
    assert!(session.local_cleanup.source_pending);
    assert_eq!(
        session.ensure_local_cleanup_reusable().unwrap_err().code,
        ComputerUseErrorCode::CompletionUnknown,
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    let _ = release.send(());
}

#[tokio::test]
async fn prepared_source_replacement_requires_actual_shutdown_ack() {
    let (mut session, calls) = counting_session();
    let (source, _entered, release) = LiveObservation::from_test_shutdown_gate();
    session.live_observation = Some(source);
    release.send(()).unwrap();
    session
        .stop_live_observation_for_replacement()
        .await
        .unwrap();
    assert!(session.live_observation.is_none());
    assert!(!session.local_cleanup.source_pending);
    assert_eq!(
        session.local_cleanup.last_source.as_ref().unwrap()["cleanup_complete"],
        true
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn native_video_state_and_stop_never_probe_upstream_and_preserve_an_existing_stream() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-native-stop-{}", uuid::Uuid::new_v4()));
    let (mut session, calls) = counting_session();
    session.pixel_observation_route = Some(PixelObservationRoute::ExplicitPixelsOnly);
    let _sender = attach_test_showcase(&mut session, &directory).await;
    let active = session.recording_state().await.unwrap();
    assert_eq!(active["backend"], "native_pixels_video");
    assert_eq!(active["expected_components"], json!(["video"]));
    assert_eq!(active["trajectory_available"], false);
    assert_eq!(active["trajectory"], Value::Null);
    // Losing the target must not prevent authoritative session-owned teardown.
    session.target = None;
    let stopped = session.recording_stop().await.unwrap();
    assert_eq!(stopped["status"], "stopped");
    assert_eq!(stopped["video"]["finalized"], true);
    assert!(
        session.live_observation.is_some(),
        "recording did not create this stream"
    );
    assert!(session.recording_keepalive.is_none());
    assert_eq!(
        session.recording_state().await.unwrap()["video"],
        stopped["video"]
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    session.stop().await.unwrap();
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn native_video_stop_failure_keeps_real_partial_outcome_and_removes_recording_owner() {
    let directory = std::env::temp_dir().join(format!(
        "dcc-cua-native-failed-stop-{}",
        uuid::Uuid::new_v4()
    ));
    let (mut session, calls) = counting_session();
    session.pixel_observation_route = Some(PixelObservationRoute::ExplicitPixelsOnly);
    let _sender = attach_test_showcase(&mut session, &directory).await;
    // Deterministic no-overwrite collision after startup exercises the real encoder.
    std::fs::write(directory.join("showcase.mp4"), b"previous evidence").unwrap();
    let error = session.recording_stop().await.unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::CaptureFailed);
    let recovered = session.recording_state().await.unwrap();
    assert_eq!(recovered["status"], "failed");
    assert_eq!(recovered["active"], false);
    assert_eq!(recovered["video"]["finalized"], false);
    assert!(
        !recovered["video"]["error"]["message"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    assert!(!recovered["video"]["current_partial"].is_null());
    assert!(session.showcase.is_none());
    assert!(!session.recording_active);
    assert_eq!(
        std::fs::read(directory.join("showcase.mp4")).unwrap(),
        b"previous evidence"
    );
    assert!(directory.join("showcase.partial.mp4").exists());
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn native_video_rejects_trajectory_only_before_any_driver_or_capture_call() {
    let (mut session, calls) = counting_session();
    session.pixel_observation_route = Some(PixelObservationRoute::ExplicitPixelsOnly);
    let error = session
        .recording_start(&ComputerUseRecordingStartRequest {
            output_dir: "not-created".into(),
            record_video: false,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::InvalidAction);
    assert!(session.showcase.is_none());
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn native_video_terminal_invalidation_awaits_encoder_and_borrowed_source_without_upstream() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-invalidate-{}", uuid::Uuid::new_v4()));
    let (mut session, calls) = counting_session();
    session.pixel_observation_route = Some(PixelObservationRoute::ExplicitPixelsOnly);
    let _sender = attach_test_showcase(&mut session, &directory).await;
    session.live_observation = Some(crate::live_observation::LiveObservation::from_test_frame(
        17, 1,
    ));

    session.invalidate_local_session().await;

    assert!(!session.active);
    assert!(session.target.is_none());
    assert!(session.observation.is_none());
    assert!(session.pixel_observation_route.is_none());
    assert!(session.showcase.is_none());
    assert!(session.live_observation.is_none());
    let terminal_video = session
        .last_recording_video
        .as_ref()
        .unwrap()
        .state()
        .clone();
    assert_eq!(terminal_video["finalized"], true);
    assert!(directory.join("showcase.mp4").is_file());
    assert!(directory.join("showcase.capture.jsonl").is_file());
    let first = session.stop().await.unwrap();
    let second = session.stop().await.unwrap();
    assert!(first.success);
    let video = first.recording_video.as_ref().unwrap();
    assert!(video.finalized);
    assert_eq!(
        video.path.as_deref(),
        directory.join("showcase.mp4").to_str()
    );
    assert_eq!(
        video.capture_sidecar.as_ref().unwrap().frames,
        terminal_video["capture_provenance"]["frames"]
            .as_u64()
            .unwrap()
    );
    assert!(first.live_observation.as_ref().unwrap().cleanup_complete);
    session
        .ensure_local_cleanup_reusable()
        .expect("normal completed cleanup preserves reusable-session contract");
    assert_eq!(first, second);
    assert_eq!(
        session.last_recording_video.as_ref().unwrap().state(),
        &terminal_video
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn native_video_terminal_invalidation_retains_partial_failure_across_double_stop() {
    let directory = std::env::temp_dir().join(format!(
        "dcc-cua-invalidate-failed-{}",
        uuid::Uuid::new_v4()
    ));
    let (mut session, calls) = counting_session();
    session.pixel_observation_route = Some(PixelObservationRoute::ExplicitPixelsOnly);
    let _sender = attach_test_showcase(&mut session, &directory).await;
    std::fs::write(directory.join("showcase.mp4"), b"existing evidence").unwrap();

    session.invalidate_local_session().await;

    let video = session
        .last_recording_video
        .as_ref()
        .unwrap()
        .state()
        .clone();
    assert_eq!(video["finalized"], false);
    assert!(!video["current_partial"].is_null());
    assert!(directory.join("showcase.partial.mp4").exists());
    assert_eq!(
        std::fs::read(directory.join("showcase.mp4")).unwrap(),
        b"existing evidence"
    );
    let first = session.stop().await.unwrap();
    let second = session.stop().await.unwrap();
    assert!(!first.success);
    assert!(!first.cleanup_pending);
    assert_eq!(first.cleanup_issues.len(), 1);
    assert_eq!(
        first.cleanup_issues[0].phase,
        ComputerUseCleanupPhase::RecordingStop
    );
    let failed_video = first.recording_video.as_ref().unwrap();
    assert!(!failed_video.finalized);
    assert_eq!(
        failed_video.current_partial.as_deref(),
        directory.join("showcase.partial.mp4").to_str()
    );
    assert_eq!(
        failed_video.error_code,
        Some(ComputerUseErrorCode::CaptureFailed)
    );
    assert_eq!(
        session.start().await.unwrap_err().code,
        ComputerUseErrorCode::CaptureFailed
    );
    assert_eq!(
        session.start_pixels_only().await.unwrap_err().code,
        ComputerUseErrorCode::CaptureFailed
    );
    assert_eq!(first, second);
    assert_eq!(
        session.last_recording_video.as_ref().unwrap().state(),
        &video
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn native_video_terminal_source_join_failure_remains_unknown_across_double_stop() {
    let (mut session, calls) = counting_session();
    session.live_observation =
        Some(crate::live_observation::LiveObservation::from_test_panicking_worker());
    session.invalidate_local_session().await;
    let first = session.stop().await.unwrap();
    let second = session.stop().await.unwrap();
    assert!(!first.success);
    assert!(first.cleanup_pending);
    assert_eq!(first.cleanup_issues.len(), 1);
    assert_eq!(
        first.cleanup_issues[0].phase,
        ComputerUseCleanupPhase::LiveObservationStop
    );
    assert_eq!(
        first.cleanup_issues[0].code,
        ComputerUseErrorCode::CompletionUnknown
    );
    assert!(first.live_observation.as_ref().unwrap().cleanup_pending);
    assert!(!first.live_observation.as_ref().unwrap().cleanup_complete);
    assert_eq!(first, second);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn native_video_cancelled_terminal_cleanup_revokes_input_and_retains_pending() {
    let (mut session, calls) = counting_session();
    let (source, entered, release) =
        crate::live_observation::LiveObservation::from_test_shutdown_gate();
    session.live_observation = Some(source);
    {
        let cleanup = session.invalidate_local_session();
        tokio::pin!(cleanup);
        tokio::select! {
            result = entered => result.expect("worker received shutdown"),
            () = &mut cleanup => panic!("gated cleanup cannot finish before acknowledgement"),
        }
        // Drop the pending cleanup future, as a timed-out transport can do.
    }
    assert!(!session.active);
    assert!(session.target.is_none());
    assert!(session.observation.is_none());
    assert!(session.pixel_observation_route.is_none());
    assert!(session.live_observation.is_none());
    let first = session.stop().await.unwrap();
    let second = session.stop().await.unwrap();
    assert!(!first.success);
    assert!(first.cleanup_pending);
    assert_eq!(first, second);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    let _ = release.send(());
    assert_eq!(
        session.start().await.unwrap_err().code,
        ComputerUseErrorCode::CompletionUnknown
    );
    assert_eq!(
        session.start_pixels_only().await.unwrap_err().code,
        ComputerUseErrorCode::CompletionUnknown
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    assert_eq!(
        first.cleanup_issues[0].phase,
        ComputerUseCleanupPhase::LiveObservationStop
    );
    assert!(first.live_observation.as_ref().unwrap().cleanup_pending);
}

fn native_recorder_test_frames() -> (
    tokio::sync::watch::Sender<dcc_cua_showcase::LiveObservationStatus>,
    tokio::sync::watch::Receiver<dcc_cua_showcase::LiveObservationStatus>,
) {
    let mut status = dcc_cua_showcase::LiveObservationStatus::default();
    status.publish_frame(
        dcc_cua_showcase::LiveObservationFrame::new(
            1,
            vec![90; 16 * 16 * 4],
            16,
            16,
            std::time::Instant::now(),
        ),
        Duration::ZERO,
        "portable_encoder_test",
    );
    tokio::sync::watch::channel(status)
}

#[rstest]
#[tokio::test]
async fn native_video_cancelled_startup_stays_unknown_without_an_attached_owner() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-cancel-start-{}", uuid::Uuid::new_v4()));
    let (mut session, calls) = counting_session();
    let (sender, receiver) =
        tokio::sync::watch::channel(dcc_cua_showcase::LiveObservationStatus::default());
    let cancelled = tokio::time::timeout(
        Duration::from_millis(20),
        session.start_owned_native_recorder(receiver, directory.to_str().unwrap(), 10, false),
    )
    .await;
    assert!(
        cancelled.is_err(),
        "no frame can acknowledge a recorder startup"
    );
    assert!(session.showcase.is_none());
    assert!(session.local_cleanup.recorder_pending);
    session.invalidate_local_session().await;
    let first = session.stop().await.unwrap();
    let second = session.stop().await.unwrap();
    assert_eq!(first, second);
    assert!(!first.success);
    assert!(first.cleanup_pending);
    assert_eq!(
        first.cleanup_issues[0].phase,
        ComputerUseCleanupPhase::RecordingStop
    );
    let video = first.recording_video.unwrap();
    assert!(!video.finalized);
    assert!(video.current_partial.is_none() && video.path.is_none());
    assert_eq!(
        video.error_code,
        Some(ComputerUseErrorCode::CompletionUnknown)
    );
    assert_eq!(
        session.start().await.unwrap_err().code,
        ComputerUseErrorCode::CompletionUnknown
    );
    assert_eq!(
        session.start_pixels_only().await.unwrap_err().code,
        ComputerUseErrorCode::CompletionUnknown
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    // The canceled start drops its producer stop sender. The existing producer
    // closes naturally; this read-only wait is not a cleanup acknowledgement.
    tokio::time::timeout(Duration::from_secs(1), async {
        while sender.receiver_count() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !directory.exists(),
        "no frame was admitted, so no path is claimed"
    );
}

#[rstest]
#[tokio::test]
async fn native_video_startup_attaches_ready_owner_before_clearing_pending() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-ready-start-{}", uuid::Uuid::new_v4()));
    let (mut session, calls) = counting_session();
    let (_sender, receiver) = native_recorder_test_frames();
    session
        .start_owned_native_recorder(receiver, directory.to_str().unwrap(), 10, false)
        .await
        .unwrap();
    assert!(!session.local_cleanup.recorder_pending);
    let recorder = &session
        .showcase
        .as_ref()
        .expect("ready ACK transferred ownership")
        .recorder;
    assert_eq!(recorder.first_frame().sequence(), 1);
    let partial = recorder.state()["current_partial"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(std::path::Path::new(&partial).is_file());
    session.finalize_owned_recording_video(None).await.unwrap();
    session.invalidate_local_session().await;
    let stopped = session.stop().await.unwrap();
    assert!(stopped.success);
    assert!(stopped.recording_video.unwrap().finalized);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn native_video_joined_start_failure_preserves_actual_partial_and_clears_pending() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-joined-start-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let sidecar = directory.join("showcase.capture.partial.jsonl");
    std::fs::write(&sidecar, b"previous evidence\n").unwrap();
    let (mut session, calls) = counting_session();
    let (_sender, receiver) = native_recorder_test_frames();
    let error = session
        .start_owned_native_recorder(receiver, directory.to_str().unwrap(), 10, false)
        .await
        .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::CaptureFailed);
    assert!(!session.local_cleanup.recorder_pending);
    assert!(session.showcase.is_none());
    let video = session.last_recording_video.as_ref().unwrap().state();
    assert_eq!(video["finalized"], false);
    assert_eq!(video["startup_error"]["message"], error.message);
    let partial = std::path::Path::new(video["current_partial"].as_str().unwrap());
    assert!(
        !std::fs::read(partial).unwrap().is_empty(),
        "returned start error joined the actual encoder"
    );
    assert_eq!(std::fs::read(sidecar).unwrap(), b"previous evidence\n");
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn native_video_refused_ready_start_preserves_refusal_and_real_drain_failure() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-refused-start-{}", uuid::Uuid::new_v4()));
    let (mut session, calls) = counting_session();
    let (_sender, receiver) = native_recorder_test_frames();
    session
        .start_owned_native_recorder(receiver, directory.to_str().unwrap(), 10, false)
        .await
        .unwrap();
    std::fs::write(directory.join("showcase.mp4"), b"previous video").unwrap();
    let error = session
        .refuse_native_recording_startup(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "controlled post-ready identity refusal",
        ))
        .await
        .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
    assert!(
        error
            .message
            .contains("controlled post-ready identity refusal")
    );
    assert!(error.message.contains("recorder cleanup failed"));
    let video = session.last_recording_video.as_ref().unwrap().state();
    assert_eq!(video["finalized"], false);
    assert_eq!(video["startup_error"]["code"], "stale_observation");
    assert_eq!(video["error"]["code"], "capture_failed");
    assert!(std::path::Path::new(video["current_partial"].as_str().unwrap()).is_file());
    session.invalidate_local_session().await;
    let first = session.stop().await.unwrap();
    assert!(!first.success && !first.cleanup_pending);
    assert_eq!(first, session.stop().await.unwrap());
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn native_video_cancelled_refused_start_keeps_final_media_and_unknown_owned_source() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-cancel-refusal-{}", uuid::Uuid::new_v4()));
    let (mut session, calls) = counting_session();
    let (_sender, receiver) = native_recorder_test_frames();
    let (source, entered, release) =
        crate::live_observation::LiveObservation::from_test_shutdown_gate();
    session.live_observation = Some(source);
    session
        .start_owned_native_recorder(receiver, directory.to_str().unwrap(), 10, true)
        .await
        .unwrap();
    {
        let cleanup = session.refuse_native_recording_startup(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "controlled startup refusal before source drain",
        ));
        tokio::pin!(cleanup);
        tokio::select! {
            result = entered => result.unwrap(),
            result = &mut cleanup => panic!("source shutdown gate unexpectedly completed: {result:?}"),
        }
    }
    let video = session.last_recording_video.as_ref().unwrap().state();
    assert_eq!(video["finalized"], true);
    assert_eq!(video["startup_error"]["code"], "stale_observation");
    assert!(directory.join("showcase.mp4").is_file());
    assert!(!session.local_cleanup.recorder_pending);
    assert!(session.local_cleanup.source_pending);
    session.invalidate_local_session().await;
    let first = session.stop().await.unwrap();
    assert!(!first.success && first.cleanup_pending);
    assert!(first.recording_video.as_ref().unwrap().finalized);
    assert_eq!(first, session.stop().await.unwrap());
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    let _ = release.send(());
    std::fs::remove_dir_all(directory).unwrap();
}

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
    let recorder = ShowcaseRecorder::start(
        crate::live_observation::showcase_subscription(receiver),
        output_dir.to_str().unwrap(),
        10,
    )
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
