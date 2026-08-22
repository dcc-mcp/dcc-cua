use rstest::rstest;

use std::io::BufReader;
use std::sync::Arc;

use super::*;

fn encode_frames(
    frames: mpsc::Receiver<ShowcaseProducerEvent>,
    path: &Path,
    fps: u32,
    ready: oneshot::Sender<ShowcaseResult<()>>,
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
    let finalized = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let state = recorder.state();
            if state["finalized"] == true {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("showcase should finalize after its source closes");
    assert_eq!(finalized["frames"], 2);

    let stopped = recorder.stop().await.expect("finalized showcase recording");
    assert_eq!(stopped["frames"], 2);
    std::fs::remove_dir_all(directory).unwrap();
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
