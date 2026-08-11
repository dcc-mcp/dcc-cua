use rstest::rstest;

use super::*;

#[rstest]
fn clipboard_and_recording_requests_are_bounded() {
    assert!(
        validate_clipboard_write_request(&ComputerUseClipboardWriteRequest::default()).is_err()
    );
    assert!(
        validate_clipboard_write_request(&ComputerUseClipboardWriteRequest {
            text: Some("hello".into()),
            image_path: Some("C:\\image.png".into()),
            file_path: None,
        })
        .is_err()
    );
    assert!(
        validate_recording_start_request(&ComputerUseRecordingStartRequest {
            output_dir: "relative/output".into(),
            record_video: false,
        })
        .is_err()
    );
    assert!(
        validate_recording_start_request(&ComputerUseRecordingStartRequest {
            output_dir: "~/cua-recordings".into(),
            record_video: false,
        })
        .is_ok()
    );
}

#[rstest]
fn recording_state_aggregates_a_lost_trajectory_lease_without_hiding_raw_state() {
    let trajectory = json!({
        "recording": true,
        "enabled": false,
        "owner": "recording-session",
        "output_dir": "C:\\recordings\\showcase",
    });

    let state =
        aggregate_recording_state(true, false, &trajectory, None, &["trajectory_lease_lost"]);

    assert_eq!(state["status"], "degraded");
    assert_eq!(state["healthy"], false);
    assert_eq!(state["expected_components"], json!(["trajectory"]));
    assert_eq!(state["issues"], json!(["trajectory_lease_lost"]));
    assert_eq!(state["trajectory"], trajectory);
    assert_eq!(state["video"], Value::Null);
}

#[rstest]
fn recording_state_reports_healthy_video_and_clean_terminal_states() {
    let trajectory = json!({"structuredContent": {"enabled": true}});
    let video = json!({"active": true, "path": "C:\\recordings\\showcase.mp4"});
    let active = aggregate_recording_state(true, true, &trajectory, Some(&video), &[]);
    assert_eq!(active["status"], "active");
    assert_eq!(active["healthy"], true);
    assert_eq!(
        active["expected_components"],
        json!(["trajectory", "video"])
    );
    assert_eq!(active["video"], video);

    let stopped = aggregate_recording_state(false, true, &trajectory, None, &[]);
    assert_eq!(stopped["status"], "stopped");
    assert_eq!(stopped["healthy"], true);
    assert_eq!(stopped["video"], Value::Null);
}

#[rstest]
fn recording_state_preserves_finalized_video_terminal_reason() {
    let trajectory = json!({
        "structuredContent": {
            "enabled": true,
            "owner": "recording-session",
        }
    });
    let video = json!({
        "active": false,
        "backend": "embedded-openh264",
        "path": "C:\\recordings\\showcase.mp4",
        "finalized": true,
        "frames": 3_528,
        "terminal_reason": {
            "code": "interactive_desktop_unavailable",
            "message": "Windows interactive session disconnected",
            "timestamp_ms": 1_786_286_360_894_u64,
            "last_sequence": 3_528,
        }
    });

    let state =
        aggregate_recording_state(true, true, &trajectory, Some(&video), &["video_stopped"]);

    assert_eq!(state["status"], "degraded");
    assert_eq!(state["issues"], json!(["video_stopped"]));
    assert_eq!(state["video"], video);
}

#[rstest]
fn recording_state_projects_a_transient_video_pause_without_latching_a_failure() {
    let trajectory = json!({
        "structuredContent": {
            "enabled": true,
            "owner": "recording-session",
        }
    });
    let paused_video = json!({
        "active": true,
        "paused": true,
        "pause_reason": {
            "code": "interactive_desktop_unavailable",
            "message": "Windows interactive session disconnected",
        }
    });
    let health = RecordingHealth::new("recording-session");

    health.observe_video(Some(&paused_video), true);
    let paused = aggregate_recording_state(
        true,
        true,
        &trajectory,
        Some(&paused_video),
        &health.issue_names(),
    );

    assert_eq!(paused["status"], "paused");
    assert_eq!(paused["healthy"], false);
    assert_eq!(paused["issues"], json!(["video_paused"]));
    assert_eq!(health.issue_names(), Vec::<&str>::new());

    let resumed_video = json!({"active": true, "paused": false});
    health.observe_video(Some(&resumed_video), true);
    let resumed = aggregate_recording_state(
        true,
        true,
        &trajectory,
        Some(&resumed_video),
        &health.issue_names(),
    );
    assert_eq!(resumed["status"], "active");
    assert_eq!(resumed["healthy"], true);
    assert_eq!(resumed["issues"], json!([]));
}

#[rstest]
fn recording_health_latches_owner_and_component_failures() {
    let health = RecordingHealth::new("recording-session");
    health.observe_trajectory(&json!({"enabled": true, "owner": "other-session"}));
    health.observe_video(Some(&json!({"active": false})), true);
    health.observe_trajectory(&json!({"enabled": true, "owner": "recording-session"}));

    assert_eq!(
        health.issue_names(),
        vec!["owner_mismatch", "video_stopped"]
    );
}

#[rstest]
fn recording_health_reads_the_upstream_structured_content_envelope() {
    let health = RecordingHealth::new("recording-session");

    assert!(health.observe_trajectory(&json!({
        "content": [{"type": "text", "text": "recording: enabled"}],
        "structuredContent": {
            "enabled": true,
            "owner": "recording-session",
        },
    })));
    assert_eq!(health.issue_names(), Vec::<&str>::new());
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn recording_keepalive_probes_only_the_same_session_and_stops_cleanly() {
    let health = RecordingHealth::new("recording-session");
    let probed_sessions = Arc::new(Mutex::new(Vec::new()));
    let probe_count = Arc::new(Mutex::new(0_u32));
    let sessions = Arc::clone(&probed_sessions);
    let counts = Arc::clone(&probe_count);
    let mut keepalive = RecordingKeepalive::spawn(
        "recording-session".into(),
        Duration::from_secs(30),
        health.clone(),
        move |session_id| {
            let sessions = Arc::clone(&sessions);
            let counts = Arc::clone(&counts);
            async move {
                sessions.lock().unwrap().push(session_id);
                let mut count = counts.lock().unwrap();
                *count += 1;
                let enabled = *count == 1;
                Ok::<_, ()>(json!({
                    "enabled": enabled,
                    "owner": "recording-session",
                }))
            }
        },
    );

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    tokio::task::yield_now().await;
    assert_eq!(health.issue_names(), Vec::<&str>::new());

    tokio::time::advance(Duration::from_secs(30)).await;
    tokio::task::yield_now().await;
    assert_eq!(health.issue_names(), vec!["trajectory_lease_lost"]);
    let state = aggregate_recording_state(
        true,
        false,
        &json!({"enabled": false, "owner": "recording-session"}),
        None,
        &health.issue_names(),
    );
    assert_eq!(state["status"], "degraded");
    assert_eq!(
        probed_sessions.lock().unwrap().as_slice(),
        ["recording-session", "recording-session"]
    );

    tokio::time::advance(Duration::from_secs(120)).await;
    tokio::task::yield_now().await;
    assert_eq!(probed_sessions.lock().unwrap().len(), 2);

    keepalive.stop().await;
    tokio::time::advance(Duration::from_secs(120)).await;
    tokio::task::yield_now().await;
    assert_eq!(probed_sessions.lock().unwrap().len(), 2);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn dropping_recording_keepalive_prevents_future_probes() {
    let probes = Arc::new(Mutex::new(0_u32));
    let recorded = Arc::clone(&probes);
    let keepalive = RecordingKeepalive::spawn(
        "recording-session".into(),
        Duration::from_secs(30),
        RecordingHealth::new("recording-session"),
        move |_| {
            let recorded = Arc::clone(&recorded);
            async move {
                *recorded.lock().unwrap() += 1;
                Ok::<_, ()>(json!({
                    "enabled": true,
                    "owner": "recording-session",
                }))
            }
        },
    );

    tokio::task::yield_now().await;
    drop(keepalive);
    tokio::time::advance(Duration::from_secs(120)).await;
    tokio::task::yield_now().await;
    assert_eq!(*probes.lock().unwrap(), 0);
}
