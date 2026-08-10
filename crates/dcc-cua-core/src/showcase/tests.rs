use rstest::rstest;

use std::io::BufReader;
use std::sync::Arc;

use super::*;

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
fn showcase_writes_a_readable_mp4() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let path = directory.join("showcase.mp4");
    let (sender, receiver) = sync_channel(2);
    let first_captured_at = std::time::Instant::now();
    for sequence in 1..=2 {
        sender
            .send(Arc::new(LiveObservationFrame::new(
                sequence,
                vec![sequence as u8; 16 * 16 * 4],
                16,
                16,
                first_captured_at + std::time::Duration::from_millis((sequence - 1) * 250),
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
    assert_eq!(error.code, ComputerUseErrorCode::CaptureFailed);
    drop(sender);
    std::fs::remove_dir_all(directory).unwrap();
}

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
        status.record_terminal_error(&ComputerUseError::new(
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
            "Windows interactive session disconnected",
        ));
    });

    let terminating = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let state = recorder.state();
            if state["terminal_reason"]["code"] == "interactive_desktop_unavailable" {
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
    assert_eq!(
        state["terminal_reason"]["code"],
        "interactive_desktop_unavailable"
    );
    assert_eq!(
        state["terminal_reason"]["message"],
        "Windows interactive session disconnected"
    );
    assert_eq!(state["terminal_reason"]["last_sequence"], 7);
    assert!(state["terminal_reason"]["timestamp_ms"].as_u64().is_some());

    let stopped = recorder.stop().await.expect("finalized showcase recording");
    assert_eq!(stopped, state);
    std::fs::remove_dir_all(directory).unwrap();
}

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

    assert_eq!(error.code, ComputerUseErrorCode::CaptureFailed);
    assert_eq!(std::fs::read(&path).unwrap(), b"existing-showcase");
    std::fs::remove_dir_all(directory).unwrap();
}
