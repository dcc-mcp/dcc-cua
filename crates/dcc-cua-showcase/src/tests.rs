use openh264::formats::YUVSource;
use rstest::rstest;

use std::io::BufReader;
use std::sync::Arc;

use super::*;

#[rstest]
#[tokio::test]
async fn first_encoded_acknowledgement_matches_the_actual_first_sample_sidecar() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-first-ack-{}", uuid::Uuid::new_v4()));
    let captured_at = std::time::Instant::now();
    let proof = FrameCaptureProvenance::NativeExactWindow(NativeFrameProvenance {
        source: NativeFrameSource::VerifiedVisible,
        process_id: 42,
        window_handle: 77,
        native_instance: NativeFrameInstance {
            process_creation_time_100ns: 123,
            window_thread_id: 8,
            window_class_hash: 90,
            owner_window_handle: 0,
        },
        native_window_bounds: [-10, 20, 32, 16],
        native_visible_bounds: [-10, 20, 32, 16],
        source_rect: [-10, 20, 32, 16],
        window_dpi: 144,
        capture_generation: 19,
        stream_id: 7,
    });
    let mut initial = LiveObservationStatus::default();
    initial.publish_frame(
        LiveObservationFrame::new(18, vec![0; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::ZERO,
        "test",
    );
    let (sender, receiver) = watch::channel(initial);
    // The producer watch advanced after a consumer's preparation read. The
    // acknowledgement must describe the frame actually encoded, not that read.
    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                19,
                vec![210; 32 * 16 * 4],
                32,
                16,
                captured_at + std::time::Duration::from_millis(1),
            )
            .with_provenance(proof.clone()),
            std::time::Duration::ZERO,
            "test",
        )
    });
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .unwrap();
    let receipt = recorder.first_frame().clone();
    assert_eq!(receipt.sequence(), 19);
    assert_eq!(receipt.provenance(), &proof);
    assert_eq!(
        receipt.captured_at(),
        captured_at + std::time::Duration::from_millis(1)
    );
    let stopped = recorder.stop().await.unwrap();
    let receipt_json = serde_json::to_value(&receipt).unwrap();
    assert_eq!(stopped["first_encoded_frame"], receipt_json);
    let rows = std::fs::read_to_string(directory.join("showcase.capture.jsonl")).unwrap();
    let first: Value = serde_json::from_str(rows.lines().next().unwrap()).unwrap();
    for field in [
        "source_sequence",
        "captured_at_ms",
        "source_width",
        "source_height",
        "capture_provenance",
    ] {
        assert_eq!(first[field], receipt_json[field], "{field}");
    }
    assert_eq!(first["media_sample_index"], 0);
    assert_independently_decodable_segment(&directory.join("showcase.mp4"));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn stop_preserves_both_producer_and_encoder_failures() {
    let error = combine_stop_results(
        Err(capture_error("producer failed")),
        Err(capture_error("encoder failed")),
    )
    .unwrap_err();
    assert!(error.message.contains("producer failed"));
    assert!(error.message.contains("encoder failed"));
    assert_eq!(
        combine_stop_results(Ok(()), Ok(json!({"finalized":true}))).unwrap()["finalized"],
        true
    );
}

#[tokio::test]
async fn startup_failure_joins_encoder_and_retains_actual_partial_outcome() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-start-failure-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let capture_partial = directory.join("showcase.capture.partial.jsonl");
    std::fs::write(&capture_partial, b"previous evidence\n").unwrap();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![190; 16 * 16 * 4], 16, 16, std::time::Instant::now()),
        std::time::Duration::ZERO,
        "pure-test",
    );
    let (_source, receiver) = watch::channel(status);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        ShowcaseRecorder::start_with_outcome(receiver, directory.to_str().unwrap(), 10),
    )
    .await
    .unwrap();
    let (error, outcome) = match result {
        Ok(_) => panic!("an existing sidecar must refuse startup"),
        Err(failure) => failure,
    };
    assert_eq!(outcome["active"], false);
    assert_eq!(outcome["finalized"], false);
    assert_eq!(outcome["error"]["message"], error.message);
    let video_partial = std::path::Path::new(outcome["current_partial"].as_str().unwrap());
    assert!(video_partial.is_file());
    // Both tasks are joined: this path can be read immediately without waiting
    // for a detached encoder to release or change it.
    let partial_bytes = std::fs::read(video_partial).unwrap();
    assert!(!partial_bytes.is_empty());
    assert_eq!(
        std::fs::read(capture_partial).unwrap(),
        b"previous evidence\n"
    );
    assert!(!directory.join("showcase.mp4").exists());
    assert!(!directory.join("showcase.capture.jsonl").exists());
    std::fs::remove_dir_all(directory).unwrap();
}

fn encode_frames(
    frames: mpsc::Receiver<ShowcaseProducerEvent>,
    path: &Path,
    fps: u32,
    ready: oneshot::Sender<ShowcaseResult<EncodedFirstFrameEvidence>>,
) -> ShowcaseResult<Value> {
    encode_frames_with_progress(
        frames,
        path,
        fps,
        ready,
        &Mutex::new(ShowcaseProgress::default()),
    )
}

fn assert_independently_decodable_segment(path: &Path) {
    let file = File::open(path).expect("finalized segment should be readable");
    let size = file.metadata().unwrap().len();
    let mut reader = mp4::Mp4Reader::read_header(BufReader::new(file), size)
        .expect("finalized segment should have a readable MP4 index");
    let track = &reader.tracks()[&1];
    assert_eq!(track.sequence_parameter_set().unwrap()[0] & 0x1f, 7);
    assert_eq!(track.picture_parameter_set().unwrap()[0] & 0x1f, 8);
    let first = reader.read_sample(1, 1).unwrap().unwrap();
    assert!(
        first.is_sync,
        "{} must start with a sync sample",
        path.display()
    );
    let mut bytes = first.bytes.as_ref();
    let mut has_idr = false;
    while bytes.len() >= 4 {
        let length = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
        bytes = &bytes[4..];
        assert!(
            length <= bytes.len(),
            "invalid AVCC NAL length in {}",
            path.display()
        );
        if length > 0 && bytes[0] & 0x1f == 5 {
            has_idr = true;
        }
        bytes = &bytes[length..];
    }
    assert!(
        has_idr,
        "{} must start with an IDR access unit",
        path.display()
    );
}

#[rstest]
fn showcase_dimensions_are_even_and_bounded() {
    assert_eq!(fit_dimensions(3840, 2400), (1440, 900));
    assert_eq!(fit_dimensions(1513, 949), (1434, 900));
    assert_eq!(
        fit_dimensions_with_bounds(3120, 2080, 1568, 1568),
        (1568, 1044)
    );
}

#[rstest]
fn resize_reuses_the_source_when_dimensions_are_unchanged() {
    let source = vec![1, 2, 3, 4, 5, 6, 7, 8];

    let resized = resize_bgra_if_needed(&source, 2, 1, 2, 1);

    assert!(matches!(resized, std::borrow::Cow::Borrowed(_)));
    assert_eq!(resized.as_ref(), source);
}

#[rstest]
fn annex_b_start_codes_are_removed() {
    assert_eq!(strip_start_code(&[0, 0, 0, 1, 0x67]), Some(&[0x67][..]));
    assert_eq!(strip_start_code(&[0, 0, 1, 0x68]), Some(&[0x68][..]));
    assert_eq!(strip_start_code(&[1, 2, 3]), None);
}

#[rstest]
fn showcase_forwards_each_live_frame_sequence_at_most_once() {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(7, vec![7; 4], 1, 1, std::time::Instant::now()),
        std::time::Duration::ZERO,
        "test_capture",
    );
    let (_watch_sender, receiver) = watch::channel(status);
    let (frame_sender, mut frame_receiver) = mpsc::channel(2);
    let mut last_forwarded_sequence = None;
    let snapshot = receiver.borrow().clone();

    assert!(send_latest(
        &snapshot,
        &frame_sender,
        &mut last_forwarded_sequence
    ));
    let ShowcaseProducerEvent::Frame(frame) = frame_receiver.blocking_recv().unwrap() else {
        panic!("showcase should forward a frame event");
    };
    assert_eq!(frame.sequence(), 7);
    assert!(!send_latest(
        &snapshot,
        &frame_sender,
        &mut last_forwarded_sequence
    ));
    assert!(matches!(
        frame_receiver.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    _watch_sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(6, vec![6; 4], 1, 1, std::time::Instant::now()),
            std::time::Duration::ZERO,
            "test_out_of_order_capture",
        );
    });
    let snapshot = receiver.borrow().clone();
    assert!(!send_latest(
        &snapshot,
        &frame_sender,
        &mut last_forwarded_sequence
    ));
    assert!(matches!(
        frame_receiver.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[rstest]
#[tokio::test]
async fn guaranteed_showcase_forwarding_rejects_a_cached_sequence() {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(7, vec![7; 4], 1, 1, std::time::Instant::now()),
        std::time::Duration::ZERO,
        "test_capture",
    );
    let (_watch_sender, receiver) = watch::channel(status);
    let (frame_sender, mut frame_receiver) = mpsc::channel(2);
    let mut last_forwarded_sequence = Some(7);
    let snapshot = receiver.borrow().clone();

    assert_eq!(
        send_latest_guaranteed(&snapshot, &frame_sender, &mut last_forwarded_sequence).await,
        GuaranteedFrameSend::NoNewFrame
    );
    assert!(matches!(
        frame_receiver.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[rstest]
fn showcase_writes_a_readable_mp4() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let path = directory.join("showcase.mp4");
    let (sender, receiver) = mpsc::channel(2);
    let first_captured_at = std::time::Instant::now();
    for sequence in 1..=2 {
        sender
            .blocking_send(ShowcaseProducerEvent::Frame(Arc::new(
                LiveObservationFrame::new(
                    sequence,
                    vec![sequence as u8; 16 * 16 * 4],
                    16,
                    16,
                    first_captured_at + std::time::Duration::from_millis((sequence - 1) * 250),
                ),
            )))
            .unwrap();
    }
    drop(sender);
    let (ready, _) = oneshot::channel();
    let result = encode_frames(receiver, &path, 10, ready).unwrap();
    assert_eq!(result["finalized"], true);
    assert_eq!(result["duration_ms"], 500);

    let file = File::open(&path).unwrap();
    let size = file.metadata().unwrap().len();
    let mut reader = mp4::Mp4Reader::read_header(BufReader::new(file), size).unwrap();
    assert_eq!(reader.tracks().len(), 1);
    assert_eq!(reader.tracks()[&1].sample_count(), 2);
    assert_eq!(reader.read_sample(1, 1).unwrap().unwrap().duration, 250);
    assert_eq!(reader.read_sample(1, 2).unwrap().unwrap().start_time, 250);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn active_showcase_exposes_a_finalized_readable_segment_before_stop() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");

    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                2,
                vec![2; 16 * 16 * 4],
                16,
                16,
                captured_at + std::time::Duration::from_secs(301),
            ),
            std::time::Duration::from_millis(4),
            "test_capture",
        );
    });

    let active = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let state = recorder.state();
            if state["segments"]
                .as_array()
                .is_some_and(|segments| !segments.is_empty())
            {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("an active long recording should finalize its first recovery segment");

    assert_eq!(active["active"], true);
    assert_eq!(active["segments"][0]["finalized"], true);
    assert_eq!(
        active["segments"][0]["path"],
        directory.join("showcase.mp4").to_string_lossy().as_ref()
    );
    assert_eq!(
        active["current_partial"],
        directory
            .join("showcase-0002.partial.mp4")
            .to_string_lossy()
            .as_ref()
    );
    assert!(directory.join("showcase-0002.partial.mp4").is_file());
    assert!(!directory.join("showcase-0002.mp4").exists());
    let segment_path = PathBuf::from(active["segments"][0]["path"].as_str().unwrap());
    let file = File::open(segment_path).expect("finalized segment should be readable");
    let size = file.metadata().unwrap().len();
    let reader = mp4::Mp4Reader::read_header(BufReader::new(file), size)
        .expect("finalized segment should have a readable MP4 index");
    assert_eq!(reader.tracks().len(), 1);

    recorder.stop().await.expect("finalized showcase recording");
    drop(sender);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn active_showcase_marks_only_the_unfinished_tail_as_partial() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let final_path = directory.join("showcase.mp4");
    let partial_path = directory.join("showcase.partial.mp4");
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1; 16 * 16 * 4], 16, 16, std::time::Instant::now()),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (_sender, receiver) = watch::channel(status);

    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");
    let active = recorder.state();

    assert_eq!(active["active"], true);
    assert_eq!(
        active["current_partial"],
        partial_path.to_string_lossy().as_ref()
    );
    assert!(partial_path.is_file());
    assert!(!final_path.exists());

    let stopped = recorder.stop().await.expect("finalized showcase recording");
    assert_eq!(
        stopped["segments"][0]["path"],
        final_path.to_string_lossy().as_ref()
    );
    assert!(final_path.is_file());
    assert!(!partial_path.exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn showcase_stop_writes_an_ordered_non_overlapping_segment_manifest() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");
    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                2,
                vec![2; 16 * 16 * 4],
                16,
                16,
                captured_at + std::time::Duration::from_secs(301),
            ),
            std::time::Duration::from_millis(4),
            "test_capture",
        );
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while recorder.state()["segments"].as_array().map_or(0, Vec::len) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first segment should roll before stop");

    let stopped = recorder.stop().await.expect("finalized showcase recording");
    let manifest_path = PathBuf::from(
        stopped["manifest_path"]
            .as_str()
            .expect("stop should publish the manifest path"),
    );
    let manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();

    assert_eq!(manifest_path, directory.join("showcase.manifest.json"));
    assert_eq!(manifest["finalized"], true);
    assert_eq!(manifest["current_partial"], Value::Null);
    assert_eq!(manifest["segments"], stopped["segments"]);
    assert_eq!(manifest["segments"][0]["index"], 0);
    assert_eq!(manifest["segments"][1]["index"], 1);
    assert_eq!(manifest["segments"][0]["duration_ms"], 301_000);
    assert_eq!(
        manifest["segments"][0]["path"],
        directory.join("showcase.mp4").to_string_lossy().as_ref()
    );
    assert_eq!(
        manifest["segments"][1]["path"],
        directory
            .join("showcase-0002.mp4")
            .to_string_lossy()
            .as_ref()
    );
    let first_end = manifest["segments"][0]["start_ms"].as_u64().unwrap()
        + manifest["segments"][0]["duration_ms"].as_u64().unwrap();
    assert!(first_end <= manifest["segments"][1]["start_ms"].as_u64().unwrap());
    for segment in manifest["segments"].as_array().unwrap() {
        assert_independently_decodable_segment(Path::new(segment["path"].as_str().unwrap()));
    }

    drop(sender);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn showcase_error_state_preserves_segments_finalized_before_the_tail_failed() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");
    let conflicting_tail = directory.join("showcase-0002.mp4");
    std::fs::write(&conflicting_tail, b"occupied").unwrap();

    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                2,
                vec![2; 16 * 16 * 4],
                16,
                16,
                captured_at + std::time::Duration::from_secs(301),
            ),
            std::time::Duration::from_millis(4),
            "test_capture",
        );
    });
    let failed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let state = recorder.state();
            if state["error"]["code"] == "capture_failed" {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("tail path collision should stop the encoder");

    assert_eq!(failed["active"], false);
    assert_eq!(failed["segments"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        failed["segments"][0]["path"],
        directory.join("showcase.mp4").to_string_lossy().as_ref()
    );
    assert_eq!(failed["current_partial"], Value::Null);
    assert_eq!(std::fs::read(conflicting_tail).unwrap(), b"occupied");

    let error = recorder
        .stop()
        .await
        .expect_err("failed tail should remain an error");
    assert_eq!(error.code, ShowcaseErrorCode::CaptureFailed);
    drop(sender);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
fn failed_segment_initialization_removes_its_partial_file() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let partial_path = directory.join("showcase.partial.mp4");

    let result: ShowcaseResult<()> = create_partial_file(&partial_path, |mut file| {
        file.write_all(b"incomplete mp4").map_err(capture_error)?;
        Err(capture_error("forced segment setup failure"))
    });

    let error = result.expect_err("injected setup failure should be preserved");
    assert!(error.message.contains("forced segment setup failure"));
    assert!(
        !partial_path.exists(),
        "a failed segment setup must not leave an unreported partial file"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn showcase_survives_a_live_pause_and_encodes_only_newer_sequences() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(7, vec![7; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");

    sender.send_modify(|status| {
        status.record_paused_error(&ShowcaseError::new(
            ShowcaseErrorCode::InteractiveDesktopUnavailable,
            "Windows interactive session disconnected",
        ));
    });
    sender.send_modify(|status| {
        status.record_paused_error(&ShowcaseError::new(
            ShowcaseErrorCode::InteractiveDesktopUnavailable,
            "Windows interactive session disconnected",
        ));
    });

    let paused = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let state = recorder.state();
            if state["paused"] == true {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("showcase should project the transient pause");
    assert_eq!(paused["active"], true);
    assert_eq!(paused["terminal_reason"], Value::Null);
    assert_eq!(
        paused["pause_reason"]["code"],
        "interactive_desktop_unavailable"
    );

    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                8,
                vec![8; 16 * 16 * 4],
                16,
                16,
                captured_at + std::time::Duration::from_millis(100),
            ),
            std::time::Duration::from_millis(4),
            "test_resume",
        );
    });

    let resumed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let state = recorder.state();
            if state["paused"] == false {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("showcase should clear the pause after a newer frame");
    assert_eq!(resumed["active"], true);
    assert_eq!(resumed["pause_reason"], Value::Null);

    drop(sender);
    let finalized = recorder
        .wait_for_finalization(std::time::Duration::from_secs(30))
        .await
        .expect("showcase should signal finalization after its source closes");
    assert_eq!(finalized["frames"], 2);

    let stopped = recorder.stop().await.expect("finalized showcase recording");
    assert_eq!(stopped["frames"], 2);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn showcase_finalization_wait_has_a_bounded_diagnostic() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1; 16 * 16 * 4], 16, 16, std::time::Instant::now()),
        std::time::Duration::from_millis(1),
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");

    let error = recorder
        .wait_for_finalization(std::time::Duration::ZERO)
        .await
        .expect_err("an open source must not report finalization");
    assert_eq!(error.code, ShowcaseErrorCode::CaptureFailed);
    assert_eq!(
        error.message,
        "showcase encoder did not finalize within 0 milliseconds"
    );

    drop(sender);
    recorder.stop().await.expect("stop showcase recorder");
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn showcase_finalization_signal_is_repeatable_without_polling() {
    for attempt in 0..8 {
        let directory =
            std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
        let mut status = LiveObservationStatus::default();
        status.publish_frame(
            LiveObservationFrame::new(
                attempt,
                vec![attempt as u8; 16 * 16 * 4],
                16,
                16,
                std::time::Instant::now(),
            ),
            std::time::Duration::from_millis(1),
            "test_capture",
        );
        let (sender, receiver) = watch::channel(status);
        let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
            .await
            .unwrap_or_else(|error| panic!("start showcase attempt {attempt}: {error}"));

        drop(sender);
        let finalized = recorder
            .wait_for_finalization(std::time::Duration::from_secs(30))
            .await
            .unwrap_or_else(|error| panic!("finalize showcase attempt {attempt}: {error}"));
        assert_eq!(finalized["frames"], 1, "attempt {attempt}: {finalized}");
        recorder
            .stop()
            .await
            .unwrap_or_else(|error| panic!("stop showcase attempt {attempt}: {error}"));
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[rstest]
#[tokio::test]
async fn showcase_pause_excludes_wall_clock_gap_and_resumes_in_a_new_idr_segment() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(7, vec![7; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");

    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                8,
                vec![8; 16 * 16 * 4],
                16,
                16,
                captured_at + std::time::Duration::from_millis(100),
            ),
            std::time::Duration::from_millis(4),
            "test_capture",
        );
    });
    sender.send_modify(|status| {
        status.record_paused_error(&ShowcaseError::new(
            ShowcaseErrorCode::InteractiveDesktopUnavailable,
            "Windows interactive session disconnected",
        ));
    });

    let paused = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let state = recorder.state();
            if state["paused"] == true {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("showcase should project the producer pause");
    assert_eq!(paused["segments"].as_array().map(Vec::len), Some(1));
    assert_eq!(paused["segments"][0]["duration_ms"], 200);
    assert_eq!(paused["segments"][0]["frames"], 2);
    assert_eq!(paused["current_partial"], Value::Null);

    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                9,
                vec![9; 16 * 16 * 4],
                16,
                16,
                captured_at
                    + std::time::Duration::from_secs(301)
                    + std::time::Duration::from_millis(100),
            ),
            std::time::Duration::from_millis(4),
            "test_resume",
        );
    });

    let resumed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let state = recorder.state();
            if state["paused"] == false && state["current_partial"].is_string() {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("strictly newer frame should resume the same recorder");
    assert_eq!(
        resumed["path"],
        directory.join("showcase.mp4").to_string_lossy().as_ref()
    );
    assert_eq!(resumed["segments"].as_array().map(Vec::len), Some(1));

    let stopped = recorder.stop().await.expect("finalized showcase recording");
    assert_eq!(stopped["frames"], 3);
    assert_eq!(stopped["duration_ms"], 300);
    assert_eq!(stopped["segments"].as_array().map(Vec::len), Some(2));
    assert_eq!(stopped["segments"][0]["start_ms"], 0);
    assert_eq!(stopped["segments"][0]["duration_ms"], 200);
    assert_eq!(stopped["segments"][1]["start_ms"], 200);
    assert_eq!(stopped["segments"][1]["duration_ms"], 100);
    for segment in stopped["segments"].as_array().unwrap() {
        assert!(segment["duration_ms"].as_u64().unwrap() <= SEGMENT_DURATION_MS);
        assert_independently_decodable_segment(Path::new(segment["path"].as_str().unwrap()));
    }
    assert!(!directory.join("showcase.partial.mp4").exists());
    assert!(!directory.join("showcase-0002.partial.mp4").exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn showcase_stays_paused_until_a_strictly_newer_sequence_arrives() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(7, vec![7; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");
    sender.send_modify(|status| {
        status.record_paused_error(&ShowcaseError::new(
            ShowcaseErrorCode::InteractiveDesktopUnavailable,
            "Windows interactive session disconnected",
        ));
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while recorder.state()["paused"] != true {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("showcase should enter pause");

    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                7,
                vec![70; 16 * 16 * 4],
                16,
                16,
                captured_at + std::time::Duration::from_secs(301),
            ),
            std::time::Duration::from_millis(4),
            "stale_resume_capture",
        );
    });
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    let still_paused = recorder.state();
    assert_eq!(still_paused["paused"], true);
    assert_eq!(still_paused["segments"].as_array().map(Vec::len), Some(1));
    assert_eq!(still_paused["current_partial"], Value::Null);

    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                8,
                vec![8; 16 * 16 * 4],
                16,
                16,
                captured_at + std::time::Duration::from_secs(302),
            ),
            std::time::Duration::from_millis(4),
            "strict_resume_capture",
        );
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let state = recorder.state();
            if state["paused"] == false && state["current_partial"].is_string() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("strictly newer sequence should resume the recorder");

    let stopped = recorder.stop().await.expect("finalized showcase recording");
    assert_eq!(stopped["frames"], 2);
    assert_eq!(stopped["duration_ms"], 200);
    assert_eq!(stopped["segments"].as_array().map(Vec::len), Some(2));
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn showcase_resume_stays_paused_until_the_new_idr_segment_is_ready() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(7, vec![7; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");
    sender.send_modify(|status| {
        status.record_paused_error(&ShowcaseError::new(
            ShowcaseErrorCode::InteractiveDesktopUnavailable,
            "Windows interactive session disconnected",
        ));
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while recorder.state()["paused"] != true {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("showcase should enter pause");

    // Hold the encoder immediately before it can publish the resumed partial
    // segment. Public pause state must remain latched throughout this
    // backpressure window.
    let progress_guard = lock_unpoisoned(&recorder.progress);
    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                8,
                vec![8; 16 * 16 * 4],
                16,
                16,
                captured_at + std::time::Duration::from_secs(301),
            ),
            std::time::Duration::from_millis(4),
            "test_resume",
        );
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
    let mut cleared_before_segment = false;
    while std::time::Instant::now() < deadline {
        if lock_unpoisoned(&recorder.pause_reason).is_none() {
            cleared_before_segment = true;
            break;
        }
        std::thread::yield_now();
    }
    drop(progress_guard);

    let stopped = recorder.stop().await.expect("finalized showcase recording");
    assert_eq!(stopped["segments"].as_array().map(Vec::len), Some(2));
    std::fs::remove_dir_all(directory).unwrap();
    assert!(
        !cleared_before_segment,
        "showcase must not report resumed before the new IDR segment exists"
    );
}

#[tokio::test]
async fn showcase_preserves_pause_boundary_conflated_with_fresh_resume() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::ZERO,
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .unwrap();
    // Exactly one watch notification; no consumer can observe the intermediate pause.
    sender.send_modify(|status| {
        status.record_paused_error(&capture_error("controlled source occlusion"));
        status.publish_frame(
            LiveObservationFrame::new(
                2,
                vec![2; 16 * 16 * 4],
                16,
                16,
                captured_at + std::time::Duration::from_secs(60),
            ),
            std::time::Duration::ZERO,
            "test_resume",
        );
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let progress = lock_unpoisoned(&recorder.progress).clone();
            if progress.segments.len() == 1 && progress.current_partial.is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("a conflated pause must finalize the old segment before resuming");
    let state = recorder.stop().await.unwrap();
    assert_eq!(state["segments"].as_array().map(Vec::len), Some(2));
    assert_eq!(state["duration_ms"], 200);
    assert_eq!(state["capture_provenance"]["frames"], 2);
    let rows =
        std::fs::read_to_string(state["capture_provenance"]["path"].as_str().unwrap()).unwrap();
    let rows = rows
        .lines()
        .map(|row| serde_json::from_str::<Value>(row).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(rows.iter().filter(|row| row["kind"] == "pause").count(), 1);
    assert_eq!(rows[2]["media_start_ms"], 100);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn showcase_pause_projection_uses_the_acknowledged_status_snapshot() {
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(7, vec![7; 4], 1, 1, captured_at),
        std::time::Duration::ZERO,
        "test_capture",
    );
    let (status_sender, status_receiver) = watch::channel(status);
    let (event_sender, mut event_receiver) = mpsc::channel(2);
    let (stop_sender, stop_receiver) = oneshot::channel();
    let pause_reason = Arc::new(Mutex::new(None));
    let producer_pause_reason = Arc::clone(&pause_reason);
    let producer = tokio::spawn(
        ShowcaseProducer {
            frames: status_receiver,
            sender: event_sender,
            last_forwarded_sequence: None,
            last_applied_pause_fence: None,
            paused: false,
            pause_reason: producer_pause_reason,
            terminal_reason: Arc::new(Mutex::new(None)),
        }
        .run(stop_receiver),
    );

    let ShowcaseProducerEvent::Frame(initial) = event_receiver.recv().await.unwrap() else {
        panic!("showcase should forward its initial frame");
    };
    assert_eq!(initial.sequence(), 7);

    status_sender.send_modify(|status| {
        status.record_paused_error(&ShowcaseError::new(
            ShowcaseErrorCode::InteractiveDesktopUnavailable,
            "Windows interactive session disconnected",
        ));
    });
    let ShowcaseProducerEvent::Paused(pause_acknowledgement) = event_receiver.recv().await.unwrap()
    else {
        panic!("showcase should serialize the pause boundary");
    };

    status_sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                8,
                vec![8; 4],
                1,
                1,
                captured_at + std::time::Duration::from_millis(100),
            ),
            std::time::Duration::ZERO,
            "test_resume",
        );
    });
    pause_acknowledgement.send(Ok(())).unwrap();

    let ShowcaseProducerEvent::ResumedFrame(resumed, resume_acknowledgement) =
        event_receiver.recv().await.unwrap()
    else {
        panic!("showcase should serialize the newer resume frame");
    };
    assert_eq!(resumed.sequence(), 8);
    assert!(
        lock_unpoisoned(&pause_reason).is_some(),
        "a newer watch value must not clear an unacknowledged resume"
    );
    resume_acknowledgement.send(Ok(())).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while lock_unpoisoned(&pause_reason).is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("acknowledged resume should clear the applied pause");

    let (acknowledged, acknowledgement) = oneshot::channel();
    assert!(
        stop_sender
            .send(ShowcaseProducerStop { acknowledged })
            .is_ok(),
        "running producer should accept its stop request"
    );
    acknowledgement.await.unwrap();
    producer.await.unwrap();
}

#[tokio::test]
async fn showcase_conflated_pause_resume_terminal_immediate_stop_preserves_sample_mapping() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(7, vec![30; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::ZERO,
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .unwrap();
    sender.send_modify(|status| {
        status.record_paused_error(&capture_error("controlled source occlusion"));
        status.publish_frame(
            LiveObservationFrame::new(
                9,
                vec![210; 32 * 16 * 4],
                32,
                16,
                captured_at + std::time::Duration::from_secs(60),
            ),
            std::time::Duration::ZERO,
            "test_resume",
        );
        status.record_terminal_error(&ShowcaseError::new(
            ShowcaseErrorCode::MissingWindow,
            "controlled target closed after fresh resume",
        ));
    });
    // No yield or acknowledgement wait before stop: its biased select must
    // drain the same watch snapshot through pause, fresh resume and terminal.
    let state = tokio::time::timeout(std::time::Duration::from_secs(3), recorder.stop())
        .await
        .expect("immediate stop must drain acknowledged pause and resume")
        .unwrap();
    assert_eq!(state["terminal_reason"]["code"], "missing_window");
    assert_eq!(state["terminal_reason"]["last_sequence"], 9);
    assert_eq!(
        state["terminal_reason"]["message"],
        "controlled target closed after fresh resume"
    );
    assert_eq!(state["frames"], 2);
    assert_eq!(state["duration_ms"], 200);
    assert_eq!(state["current_partial"], Value::Null);
    let bytes = std::fs::read(state["capture_provenance"]["path"].as_str().unwrap()).unwrap();
    use sha2::{Digest, Sha256};
    assert_eq!(
        state["capture_provenance"]["sha256"],
        format!("{:x}", Sha256::digest(&bytes))
    );
    let rows = std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .map(|row| serde_json::from_str::<Value>(row).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1]["kind"], "pause");
    assert_eq!(rows[1]["media_end_ms"], 100);
    assert_eq!(rows[1]["next_media_sample_index"], 1);
    let frames = rows
        .iter()
        .filter(|row| row["kind"] == "frame")
        .collect::<Vec<_>>();
    let segments = state["segments"].as_array().unwrap();
    assert_eq!(segments.len(), frames.len());
    for (index, (segment, row)) in segments.iter().zip(frames).enumerate() {
        let file = File::open(segment["path"].as_str().unwrap()).unwrap();
        let size = file.metadata().unwrap().len();
        let mut reader = mp4::Mp4Reader::read_header(BufReader::new(file), size).unwrap();
        assert_eq!(reader.tracks()[&1].sample_count(), 1);
        let mut annex_b = Vec::new();
        for parameter in [
            reader.tracks()[&1].sequence_parameter_set().unwrap(),
            reader.tracks()[&1].picture_parameter_set().unwrap(),
        ] {
            annex_b.extend_from_slice(&[0, 0, 0, 1]);
            annex_b.extend_from_slice(parameter);
        }
        let sample = reader.read_sample(1, 1).unwrap().unwrap();
        assert!(sample.is_sync);
        assert_eq!(sample.duration, 100);
        assert_eq!(row["media_sample_index"], index);
        assert_eq!(row["segment_index"], index);
        assert_eq!(row["source_sequence"], [7, 9][index]);
        assert_eq!(row["source_width"], [16, 32][index]);
        assert_eq!(row["encoded_width"], segment["width"]);
        assert_eq!(row["encoded_height"], segment["height"]);
        assert_eq!(
            row["media_start_ms"],
            segment["start_ms"].as_u64().unwrap() + sample.start_time
        );
        let mut nals = sample.bytes.as_ref();
        while !nals.is_empty() {
            assert!(nals.len() >= 4);
            let length = u32::from_be_bytes(nals[..4].try_into().unwrap()) as usize;
            nals = &nals[4..];
            assert!(length > 0 && length <= nals.len());
            annex_b.extend_from_slice(&[0, 0, 0, 1]);
            annex_b.extend_from_slice(&nals[..length]);
            nals = &nals[length..];
        }
        let mut decoder = openh264::decoder::Decoder::with_api_config(
            openh264::OpenH264API::from_source(),
            openh264::decoder::DecoderConfig::default().debug(false),
        )
        .unwrap();
        let decoded = decoder
            .decode(&annex_b)
            .unwrap()
            .expect("independent IDR decode");
        let mut rgb = vec![0; decoded.rgb8_len()];
        decoded.write_rgb8(&mut rgb);
        // Deliberately separated dark/bright markers survive lossy H264;
        // each actual decoded sample must match its own source sequence.
        assert!(rgb.iter().all(|value| if index == 0 {
            *value < 64
        } else {
            *value > 192
        }));
        assert_independently_decodable_segment(Path::new(segment["path"].as_str().unwrap()));
    }
    assert!(!directory.join("showcase.capture.partial.jsonl").exists());
    drop(sender);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn showcase_start_returns_a_typed_error_for_an_initial_pause_without_a_frame() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let mut status = LiveObservationStatus::default();
    status.record_paused_error(&ShowcaseError::new(
        ShowcaseErrorCode::InteractiveDesktopUnavailable,
        "Windows interactive session disconnected before the first frame",
    ));
    let (_sender, receiver) = watch::channel(status);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10),
    )
    .await
    .expect("an initial paused source must not hang showcase startup");
    let error = match result {
        Ok(_) => panic!("an initial pause without a frame must not start a recorder"),
        Err(error) => error,
    };

    assert_eq!(error.code, ShowcaseErrorCode::CaptureFailed);
    assert!(error.message.contains("paused before its first frame"));
    assert!(!directory.exists());

    let transition_directory =
        std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let (status_sender, receiver) = watch::channel(LiveObservationStatus::default());
    let output_dir = transition_directory.to_string_lossy().into_owned();
    let starting =
        tokio::spawn(async move { ShowcaseRecorder::start(receiver, &output_dir, 10).await });
    tokio::task::yield_now().await;
    status_sender.send_modify(|status| {
        status.record_paused_error(&ShowcaseError::new(
            ShowcaseErrorCode::InteractiveDesktopUnavailable,
            "Windows interactive session disconnected before the first frame",
        ));
    });

    let transitioned = tokio::time::timeout(std::time::Duration::from_secs(1), starting)
        .await
        .expect("a source that pauses while starting must not hang the API")
        .unwrap();
    let error = match transitioned {
        Ok(_) => panic!("a source paused before its first frame must not start a recorder"),
        Err(error) => error,
    };
    assert_eq!(error.code, ShowcaseErrorCode::CaptureFailed);
    assert!(!transition_directory.exists());
}

#[rstest]
#[tokio::test]
async fn showcase_source_close_flushes_the_retained_latest_frame_after_backpressure() {
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(7, vec![7; 4], 1, 1, captured_at),
        std::time::Duration::ZERO,
        "test_capture",
    );
    status.record_terminal_error(&ShowcaseError::new(
        ShowcaseErrorCode::MissingWindow,
        "exact target window closed",
    ));
    let (status_sender, status_receiver) = watch::channel(status);
    let (event_sender, mut event_receiver) = mpsc::channel(1);
    event_sender
        .send(ShowcaseProducerEvent::Frame(Arc::new(
            LiveObservationFrame::new(1, vec![1; 4], 1, 1, captured_at),
        )))
        .await
        .unwrap();
    let (_stop_sender, stop_receiver) = oneshot::channel();
    let terminal_reason = Arc::new(Mutex::new(None));
    let producer_terminal_reason = Arc::clone(&terminal_reason);
    let producer = tokio::spawn(
        ShowcaseProducer {
            frames: status_receiver,
            sender: event_sender,
            last_forwarded_sequence: None,
            last_applied_pause_fence: None,
            paused: false,
            pause_reason: Arc::new(Mutex::new(None)),
            terminal_reason: producer_terminal_reason,
        }
        .run(stop_receiver),
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while lock_unpoisoned(&terminal_reason).is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("producer should finish initialization under backpressure");

    drop(status_sender);
    let ShowcaseProducerEvent::Frame(prefill) = event_receiver.recv().await.unwrap() else {
        panic!("test queue should contain its prefill frame");
    };
    assert_eq!(prefill.sequence(), 1);
    producer.await.unwrap();

    let ShowcaseProducerEvent::Frame(flushed) = event_receiver.recv().await.unwrap() else {
        panic!("source close should guaranteed-forward the retained latest frame");
    };
    assert_eq!(flushed.sequence(), 7);
}

#[rstest]
#[tokio::test]
async fn showcase_stop_while_paused_finalizes_once_without_frames_or_partial_growth() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(11, vec![11; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");

    sender.send_modify(|status| {
        status.record_paused_error(&ShowcaseError::new(
            ShowcaseErrorCode::InteractiveDesktopUnavailable,
            "Windows interactive session disconnected",
        ));
    });
    let paused = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let state = recorder.state();
            if state["paused"] == true && state["segments"].as_array().map(Vec::len) == Some(1) {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pause should safely finalize the current segment");
    assert_eq!(paused["segments"][0]["frames"], 1);
    assert_eq!(paused["segments"][0]["duration_ms"], 100);
    assert_eq!(paused["current_partial"], Value::Null);

    for _ in 0..3 {
        sender.send_modify(|status| {
            status.record_paused_error(&ShowcaseError::new(
                ShowcaseErrorCode::InteractiveDesktopUnavailable,
                "Windows interactive session remains disconnected",
            ));
        });
        tokio::task::yield_now().await;
    }
    let repeated_pause = recorder.state();
    assert_eq!(repeated_pause["segments"], paused["segments"]);
    assert_eq!(repeated_pause["current_partial"], Value::Null);

    let stopped = recorder
        .stop()
        .await
        .expect("stop while paused should finalize the manifest");
    assert_eq!(stopped["path"], paused["path"]);
    assert_eq!(stopped["manifest_path"], paused["manifest_path"]);
    assert_eq!(stopped["frames"], 1);
    assert_eq!(stopped["duration_ms"], 100);
    assert_eq!(stopped["segments"].as_array().map(Vec::len), Some(1));
    assert_eq!(stopped["current_partial"], Value::Null);
    assert!(directory.join("showcase.mp4").is_file());
    assert!(directory.join("showcase.manifest.json").is_file());
    assert!(!directory.join("showcase.partial.mp4").exists());
    assert!(!directory.join("showcase-0002.partial.mp4").exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn showcase_pause_tail_never_pushes_a_segment_past_five_minutes() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");
    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                2,
                vec![2; 16 * 16 * 4],
                16,
                16,
                captured_at + std::time::Duration::from_millis(299_950),
            ),
            std::time::Duration::from_millis(4),
            "test_capture",
        );
    });
    sender.send_modify(|status| {
        status.record_paused_error(&ShowcaseError::new(
            ShowcaseErrorCode::InteractiveDesktopUnavailable,
            "Windows interactive session disconnected",
        ));
    });

    let paused = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let state = recorder.state();
            if state["paused"] == true && state["segments"].as_array().map(Vec::len) == Some(1) {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pause should finalize the near-limit segment");
    assert_eq!(paused["segments"][0]["duration_ms"], SEGMENT_DURATION_MS);
    assert_eq!(paused["current_partial"], Value::Null);

    let stopped = recorder.stop().await.expect("finalized showcase recording");
    assert_eq!(stopped["duration_ms"], SEGMENT_DURATION_MS);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test(flavor = "current_thread")]
async fn showcase_immediate_pause_then_stop_applies_the_pause_boundary() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let partial_path = directory.join("showcase.partial.mp4");
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");
    let initial_partial_size = std::fs::metadata(&partial_path).unwrap().len();

    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                2,
                vec![2; 16 * 16 * 4],
                16,
                16,
                captured_at + std::time::Duration::from_millis(299_950),
            ),
            std::time::Duration::from_millis(4),
            "test_capture",
        );
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while std::fs::metadata(&partial_path)
            .map(|metadata| metadata.len())
            .unwrap_or_default()
            <= initial_partial_size
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the near-boundary frame should reach the active encoder");

    sender.send_modify(|status| {
        status.record_paused_error(&ShowcaseError::new(
            ShowcaseErrorCode::InteractiveDesktopUnavailable,
            "Windows interactive session disconnected",
        ));
    });
    let stopped = recorder
        .stop()
        .await
        .expect("stop should serialize the unseen pause boundary");

    assert_eq!(stopped["frames"], 2);
    assert_eq!(stopped["duration_ms"], SEGMENT_DURATION_MS);
    assert_eq!(stopped["segments"].as_array().map(Vec::len), Some(1));
    assert_eq!(stopped["segments"][0]["duration_ms"], SEGMENT_DURATION_MS);
    assert_eq!(stopped["current_partial"], Value::Null);
    assert!(!partial_path.exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test(flavor = "current_thread")]
async fn showcase_stop_flushes_the_unseen_latest_frame() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");

    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                2,
                vec![2; 16 * 16 * 4],
                16,
                16,
                captured_at + std::time::Duration::from_millis(100),
            ),
            std::time::Duration::from_millis(4),
            "test_capture",
        );
    });
    let stopped = recorder
        .stop()
        .await
        .expect("stop should flush the latest source state");

    assert_eq!(stopped["frames"], 2);
    assert_eq!(stopped["duration_ms"], 200);
    assert_eq!(stopped["segments"].as_array().map(Vec::len), Some(1));
    drop(sender);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test(flavor = "current_thread")]
async fn dropping_showcase_flushes_the_unseen_latest_frame_and_finalizes_files() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let manifest_path = directory.join("showcase.manifest.json");
    let partial_path = directory.join("showcase.partial.mp4");
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");

    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(
                2,
                vec![2; 16 * 16 * 4],
                16,
                16,
                captured_at + std::time::Duration::from_millis(100),
            ),
            std::time::Duration::from_millis(4),
            "test_capture",
        );
    });
    drop(recorder);

    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !manifest_path.is_file() || partial_path.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("drop should request graceful producer and encoder shutdown");
    let manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["finalized"], true);
    assert_eq!(manifest["frames"], 2);
    assert_eq!(manifest["duration_ms"], 200);
    assert_eq!(manifest["current_partial"], Value::Null);
    assert!(directory.join("showcase.mp4").is_file());
    assert!(!partial_path.exists());
    drop(sender);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn showcase_preserves_live_observation_terminal_reason() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let captured_at = std::time::Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(7, vec![7; 16 * 16 * 4], 16, 16, captured_at),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");

    sender.send_modify(|status| {
        status.record_terminal_error(&ShowcaseError::new(
            ShowcaseErrorCode::MissingWindow,
            "exact target window closed",
        ));
    });

    let terminating = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let state = recorder.state();
            if state["terminal_reason"]["code"] == "missing_window" {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("showcase recorder should expose the terminal reason before finalization");
    assert_eq!(terminating["active"], false);
    assert_eq!(terminating["finalized"], false);
    assert_eq!(terminating["terminal_reason"]["last_sequence"], 7);
    assert!(
        terminating["terminal_reason"]["timestamp_ms"]
            .as_u64()
            .is_some()
    );

    drop(sender);
    let state = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let state = recorder.state();
            if state["finalized"] == true {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("showcase recorder should finalize after live observation stops");

    assert_eq!(state["active"], false);
    assert_eq!(state["finalized"], true);
    assert_eq!(state["terminal_reason"]["code"], "missing_window");
    assert_eq!(
        state["terminal_reason"]["message"],
        "exact target window closed"
    );
    assert_eq!(state["terminal_reason"]["last_sequence"], 7);
    assert!(state["terminal_reason"]["timestamp_ms"].as_u64().is_some());

    let stopped = recorder.stop().await.expect("finalized showcase recording");
    assert_eq!(stopped, state);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn showcase_refuses_to_overwrite_an_existing_mp4() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("showcase.mp4");
    std::fs::write(&path, b"existing-showcase").unwrap();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1; 16 * 16 * 4], 16, 16, std::time::Instant::now()),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (_sender, receiver) = watch::channel(status);

    let error = match ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10).await {
        Ok(_) => panic!("an existing showcase must not be overwritten"),
        Err(error) => error,
    };

    assert_eq!(error.code, ShowcaseErrorCode::CaptureFailed);
    assert_eq!(std::fs::read(&path).unwrap(), b"existing-showcase");
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
#[tokio::test]
async fn showcase_finalize_does_not_overwrite_a_concurrently_created_mp4() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let final_path = directory.join("showcase.mp4");
    let partial_path = directory.join("showcase.partial.mp4");
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1; 16 * 16 * 4], 16, 16, std::time::Instant::now()),
        std::time::Duration::from_millis(4),
        "test_capture",
    );
    let (_sender, receiver) = watch::channel(status);
    let recorder = ShowcaseRecorder::start(receiver, directory.to_str().unwrap(), 10)
        .await
        .expect("showcase recorder");
    assert!(partial_path.is_file());

    std::fs::write(&final_path, b"concurrent-showcase").unwrap();
    let error = recorder
        .stop()
        .await
        .expect_err("finalization must not replace a concurrently created showcase");

    assert_eq!(error.code, ShowcaseErrorCode::CaptureFailed);
    assert_eq!(std::fs::read(&final_path).unwrap(), b"concurrent-showcase");
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
fn recording_manifest_covers_encoded_samples_across_pause_resize_and_source_gaps() {
    use sha2::{Digest, Sha256};
    let directory = std::env::temp_dir().join(format!(
        "dcc-cua-showcase-provenance-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("showcase.mp4");
    let captured_at = std::time::Instant::now();
    let make_frame = |sequence, width, color, elapsed_ms| {
        let proof = FrameCaptureProvenance::NativeExactWindow(NativeFrameProvenance {
            source: NativeFrameSource::VerifiedVisible,
            process_id: 42,
            window_handle: 500,
            native_instance: NativeFrameInstance {
                process_creation_time_100ns: 1000,
                window_thread_id: 8,
                window_class_hash: 90,
                owner_window_handle: 0,
            },
            native_window_bounds: [-104, 12, width as i32 + 8, 30],
            native_visible_bounds: [-100, 20, width as i32, 16],
            source_rect: [-100, 20, width as i32, 16],
            window_dpi: 144,
            capture_generation: sequence,
            stream_id: 7,
        });
        Arc::new(
            LiveObservationFrame::new(
                sequence,
                vec![color; width as usize * 16 * 4],
                width,
                16,
                captured_at + std::time::Duration::from_millis(elapsed_ms),
            )
            .with_provenance(proof),
        )
    };
    let (sender, receiver) = mpsc::channel(8);
    sender
        .blocking_send(ShowcaseProducerEvent::Frame(make_frame(9, 16, 30, 0)))
        .unwrap();
    sender
        .blocking_send(ShowcaseProducerEvent::Frame(make_frame(11, 16, 120, 100)))
        .unwrap();
    let (paused, _) = oneshot::channel();
    sender
        .blocking_send(ShowcaseProducerEvent::Paused(paused))
        .unwrap();
    let (resumed, _) = oneshot::channel();
    sender
        .blocking_send(ShowcaseProducerEvent::ResumedFrame(
            make_frame(12, 32, 210, 3_600_000),
            resumed,
        ))
        .unwrap();
    drop(sender);
    let (ready, _) = oneshot::channel();
    let state = encode_frames(receiver, &path, 10, ready).unwrap();
    assert_eq!(state["frames"], 3);
    let provenance_path = PathBuf::from(state["capture_provenance"]["path"].as_str().unwrap());
    let bytes = std::fs::read(&provenance_path).unwrap();
    assert_eq!(
        state["capture_provenance"]["sha256"],
        format!("{:x}", Sha256::digest(&bytes))
    );
    let rows: Vec<Value> = std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[2]["kind"], "pause");
    assert_eq!(rows[2]["media_end_ms"], 200);
    let frames: Vec<&Value> = rows.iter().filter(|row| row["kind"] == "frame").collect();
    assert_eq!(
        frames
            .iter()
            .map(|row| row["source_sequence"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![9, 11, 12]
    );
    for (index, row) in frames.iter().enumerate() {
        assert_eq!(row["media_sample_index"], index);
        assert_eq!(
            row["capture_provenance"]["native_instance"]["window_thread_id"],
            8
        );
        assert_eq!(row["capture_provenance"]["source"], "verified_visible");
    }
    assert_eq!(frames[2]["segment_index"], 1);
    assert_eq!(frames[2]["media_start_ms"], 200);
    assert_eq!(
        frames[2]["capture_provenance"]["source_rect"],
        json!([-100, 20, 32, 16])
    );
    assert_eq!(frames[2]["source_to_encoded_scale"]["x_numerator"], 16);
    assert_eq!(frames[2]["source_to_encoded_scale"]["x_denominator"], 32);
    let mut actual_samples = 0_u64;
    for segment in state["segments"].as_array().unwrap() {
        let segment_path = Path::new(segment["path"].as_str().unwrap());
        assert_independently_decodable_segment(segment_path);
        let file = File::open(segment_path).unwrap();
        let size = file.metadata().unwrap().len();
        let reader = mp4::Mp4Reader::read_header(BufReader::new(file), size).unwrap();
        actual_samples += u64::from(reader.tracks()[&1].sample_count());
    }
    assert_eq!(actual_samples, frames.len() as u64);
    assert_eq!(state["capture_provenance"]["frames"], actual_samples);
    assert!(!directory.join("showcase.capture.partial.jsonl").exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
fn provenance_sidecar_failure_cannot_finalize_incomplete_sample_mapping() {
    let directory = std::env::temp_dir().join(format!(
        "dcc-cua-showcase-provenance-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let mut manifest = CaptureManifest::create(&directory.join("showcase.mp4")).unwrap();
    let frame = LiveObservationFrame::new(1, vec![0; 4], 1, 1, std::time::Instant::now());
    assert!(manifest.frame(&frame, 1, 0, 0, 1, 1).is_err());
    assert!(manifest.finish(1).is_err());
    assert!(!directory.join("showcase.capture.jsonl").exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn provenance_sidecar_create_and_publish_failures_preserve_owned_partial_evidence() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let video = directory.join("showcase.mp4");
    let partial = directory.join("showcase.capture.partial.jsonl");
    let final_path = directory.join("showcase.capture.jsonl");
    std::fs::write(&partial, b"previous partial evidence\n").unwrap();
    assert!(CaptureManifest::create(&video).is_err());
    assert_eq!(
        std::fs::read(&partial).unwrap(),
        b"previous partial evidence\n"
    );
    assert!(!final_path.exists());
    std::fs::remove_file(&partial).unwrap();
    std::fs::write(&final_path, b"existing final evidence\n").unwrap();
    let mut manifest = CaptureManifest::create(&video).unwrap();
    manifest
        .frame(
            &LiveObservationFrame::new(7, vec![7; 4], 1, 1, std::time::Instant::now()),
            0,
            0,
            0,
            1,
            1,
        )
        .unwrap();
    assert!(manifest.finish(1).is_err());
    assert_eq!(
        std::fs::read(&final_path).unwrap(),
        b"existing final evidence\n"
    );
    let preserved = std::fs::read_to_string(&partial).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(preserved.trim()).unwrap()["source_sequence"],
        7
    );
    assert!(!directory.join("showcase.manifest.json").exists());
    std::fs::remove_dir_all(directory).unwrap();
}
