use rstest::rstest;

use super::*;
use dcc_cua_showcase::{NativeFrameInstance, NativeFrameSource};

fn target() -> WindowTarget {
    WindowTarget {
        pid: 42,
        window_id: 77,
        title: String::new(),
        app_name: String::new(),
        bounds: [0, 0, 16, 16],
        is_foreground: true,
        is_minimized: false,
        is_on_screen: true,
        z_index: None,
    }
}

fn proof() -> NativeFrameProvenance {
    NativeFrameProvenance {
        source: NativeFrameSource::VerifiedVisible,
        process_id: 42,
        window_handle: 77,
        native_instance: NativeFrameInstance {
            process_creation_time_100ns: 100,
            window_thread_id: 2,
            window_class_hash: 3,
            owner_window_handle: 0,
        },
        native_window_bounds: [0, 0, 16, 16],
        native_visible_bounds: [0, 0, 16, 16],
        source_rect: [0, 0, 16, 16],
        window_dpi: 96,
        capture_generation: 1,
        stream_id: 9,
        wgc_geometry: None,
    }
}

#[rstest]
fn native_video_first_frame_requires_actual_fresh_exact_stream_provenance() {
    let now = Instant::now();
    let frame = |proof| {
        crate::live_observation::LiveObservationFrame::new(4, vec![0; 16 * 16 * 4], 16, 16, now)
            .with_provenance(FrameCaptureProvenance::NativeExactWindow(proof))
    };
    assert!(validate_native_recording_frame(&frame(proof()), &target(), 9, now).is_ok());
    for change in 0..6 {
        let mut invalid = proof();
        match change {
            0 => invalid.process_id = 43,
            1 => invalid.window_handle = 78,
            2 => invalid.stream_id = 10,
            3 => invalid.capture_generation = 0,
            4 => invalid.native_instance.process_creation_time_100ns = 0,
            _ => invalid.native_instance.window_thread_id = 0,
        }
        assert_eq!(
            validate_native_recording_frame(&frame(invalid), &target(), 9, now)
                .unwrap_err()
                .code,
            ComputerUseErrorCode::StaleObservation
        );
    }
    assert!(
        validate_native_recording_frame(
            &frame(proof()),
            &target(),
            9,
            now + Duration::from_millis(1)
        )
        .is_err()
    );
    let portable =
        crate::live_observation::LiveObservationFrame::new(4, vec![0; 16 * 16 * 4], 16, 16, now);
    assert_eq!(
        validate_native_recording_frame(&portable, &target(), 9, now)
            .unwrap_err()
            .code,
        ComputerUseErrorCode::CaptureFailed
    );
}

#[rstest]
fn native_video_reports_pause_terminal_and_failed_finalization_without_trajectory() {
    let paused =
        project_native_recording_state(true, Some(&json!({"active":true,"paused":true})), None);
    assert_eq!(paused["status"], "paused");
    assert_eq!(paused["trajectory"], Value::Null);
    let terminal = project_native_recording_state(
        true,
        Some(&json!({"active":false,"finalized":true,
        "terminal_reason":{"code":"missing_window"}})),
        None,
    );
    assert_eq!(terminal["status"], "degraded");
    assert_eq!(terminal["healthy"], false);
    let failed = project_native_recording_state(
        false,
        Some(&json!({"active":false,"finalized":false,
        "current_partial":"test.partial.mp4","error":{"code":"capture_failed"}})),
        None,
    );
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["video"]["finalized"], false);
}
