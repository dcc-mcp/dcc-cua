use rstest::rstest;

use super::*;

pub(crate) struct LiveObservationTestPublisher {
    sender: watch::Sender<LiveObservationStatus>,
}

impl LiveObservationTestPublisher {
    pub(crate) fn publish_frame(&self, sequence: u64, capture_mode: &'static str) {
        self.sender.send_modify(|status| {
            status.publish_frame(
                LiveObservationFrame::new(sequence, vec![0; 4], 1, 1, std::time::Instant::now()),
                Duration::ZERO,
                capture_mode,
            );
        });
    }

    pub(crate) fn publish_measured_frame(
        &self,
        sequence: u64,
        capture_mode: &'static str,
        capture_duration: Duration,
        measurement: FrameCaptureMeasurement,
    ) {
        self.sender.send_modify(|status| {
            status.publish_measured_frame(
                LiveObservationFrame::new(sequence, vec![0; 4], 1, 1, std::time::Instant::now()),
                capture_duration,
                capture_mode,
                measurement,
            );
        });
    }

    pub(crate) fn pause(&self, error: ComputerUseError) {
        self.sender
            .send_modify(|status| status.record_paused_error(&error));
    }

    pub(crate) fn record_retry_failure(&self, message: impl Into<String>) {
        let message = message.into();
        self.sender
            .send_modify(|status| status.record_error(message.clone()));
    }
}

impl LiveObservation {
    pub(crate) fn from_test_frame(stream_id: u64, sequence: u64) -> Self {
        Self::from_test_stream(stream_id, sequence).0
    }

    pub(crate) fn from_test_stream(
        stream_id: u64,
        sequence: u64,
    ) -> (Self, LiveObservationTestPublisher) {
        let mut status = LiveObservationStatus::default();
        status.publish_frame(
            LiveObservationFrame::new(sequence, vec![0; 4], 1, 1, std::time::Instant::now()),
            Duration::ZERO,
            "test_capture",
        );
        let (sender, receiver) = watch::channel(status);
        let shutdown = LiveObservationShutdown::default();
        let worker_shutdown = shutdown.clone();
        let task = tokio::spawn(async move { worker_shutdown.cancelled().await });
        (
            Self {
                stream_id,
                fps: 1,
                max_dimension: 1,
                receiver,
                shutdown,
                task,
            },
            LiveObservationTestPublisher { sender },
        )
    }
}

#[rstest]
#[tokio::test]
async fn stop_requests_shutdown_and_waits_for_worker_acknowledgement() {
    let (sender, receiver) = watch::channel(LiveObservationStatus::default());
    let shutdown = LiveObservationShutdown::default();
    let worker_shutdown = shutdown.clone();
    let acknowledged = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_acknowledged = Arc::clone(&acknowledged);
    let task = tokio::spawn(async move {
        worker_shutdown.cancelled().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        worker_acknowledged.store(true, std::sync::atomic::Ordering::Release);
        drop(sender);
    });
    let observation = LiveObservation {
        stream_id: 1,
        fps: 1,
        max_dimension: 1,
        receiver,
        shutdown,
        task,
    };

    let state = observation.stop().await;

    assert!(acknowledged.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(state["active"], false);
}

#[cfg(windows)]
#[rstest]
fn shutdown_wakes_a_blocking_capture_waiter() {
    let shutdown = LiveObservationShutdown::default();
    let worker_shutdown = shutdown.clone();
    let (acknowledged, acknowledgement) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        acknowledged
            .send(worker_shutdown.wait_timeout(Duration::from_secs(5)))
            .expect("acknowledge shutdown");
    });

    std::thread::sleep(Duration::from_millis(10));
    shutdown.request();

    assert!(
        acknowledgement
            .recv_timeout(Duration::from_millis(250))
            .expect("blocking capture waiter should wake promptly")
    );
    worker.join().expect("join blocking capture waiter");
}

#[rstest]
#[tokio::test]
async fn test_stream_publisher_advances_the_latest_sequence() {
    let (observation, publisher) = LiveObservation::from_test_stream(3, 7);

    publisher.publish_frame(8, "test_resume");

    assert_eq!(
        observation.latest_fence(),
        Some(LiveObservationFence::new(3, 8))
    );
}

#[rstest]
#[tokio::test]
async fn static_source_wait_is_not_reported_as_frame_processing_cost() {
    let (observation, publisher) = LiveObservation::from_test_stream(3, 7);

    publisher.publish_measured_frame(
        8,
        "persistent_wgc",
        Duration::from_millis(908),
        FrameCaptureMeasurement::measured(
            Duration::from_millis(900),
            Duration::from_millis(8),
            Duration::from_millis(5),
            Duration::from_millis(2),
            CompositorTiming::unavailable("frame_timestamp_unavailable"),
        ),
    );

    let state = observation.state();
    assert_eq!(state["last_capture_duration_ms"], 908);
    assert_eq!(
        state["capture_duration_semantics"],
        "capture_cycle_wall_time_including_source_wait_readback_and_target_validation"
    );
    assert_eq!(state["last_source_wait_ms"], 900);
    assert_eq!(state["max_source_wait_ms"], 900);
    assert_eq!(state["last_readback_total_ms"], 8);
    assert_eq!(state["max_readback_total_ms"], 8);
    assert_eq!(
        state["max_compositor_to_publish_ms"],
        serde_json::Value::Null
    );
    assert_eq!(state["last_gpu_copy_map_ms"], 5);
    assert_eq!(state["last_cpu_copy_ms"], 2);
    assert_eq!(
        state["readback_total_semantics"],
        "frame_processing_after_source_arrival"
    );
}

#[rstest]
#[tokio::test]
async fn missing_compositor_timestamp_is_explicitly_unavailable() {
    let (observation, publisher) = LiveObservation::from_test_stream(3, 7);
    publisher.publish_measured_frame(
        8,
        "persistent_wgc",
        Duration::from_millis(8),
        FrameCaptureMeasurement::measured(
            Duration::ZERO,
            Duration::from_millis(8),
            Duration::from_millis(5),
            Duration::from_millis(2),
            CompositorTiming::unavailable("frame_timestamp_unavailable"),
        ),
    );

    assert_eq!(
        observation.state()["last_compositor_timing"],
        serde_json::json!({
            "status": "unavailable",
            "reason": "frame_timestamp_unavailable",
        })
    );
}

#[rstest]
#[tokio::test]
async fn failed_capture_does_not_fabricate_or_replace_frame_measurements() {
    let (observation, publisher) = LiveObservation::from_test_stream(3, 7);
    publisher.publish_measured_frame(
        8,
        "persistent_wgc",
        Duration::from_millis(12),
        FrameCaptureMeasurement::measured(
            Duration::from_millis(3),
            Duration::from_millis(8),
            Duration::from_millis(5),
            Duration::from_millis(2),
            CompositorTiming::Available {
                system_relative_time_100ns: 100,
                compositor_to_publish: Duration::from_millis(4),
            },
        ),
    );
    publisher.record_retry_failure("synthetic capture failure");

    let state = observation.state();
    assert_eq!(state["capture_failures"], 1);
    assert_eq!(state["last_error"], "synthetic capture failure");
    assert_eq!(state["last_source_wait_ms"], 3);
    assert_eq!(state["last_readback_total_ms"], 8);
    assert_eq!(state["max_compositor_to_publish_ms"], 4);
    assert_eq!(
        state["last_compositor_timing"],
        serde_json::json!({
            "status": "available",
            "system_relative_time_100ns": 100,
            "compositor_to_publish_ms": 4,
        })
    );
}

#[rstest]
fn compositor_timing_is_typed_before_the_first_frame() {
    assert_eq!(
        LiveObservationStatus::default().as_json(true, 8)["last_compositor_timing"],
        serde_json::json!({
            "status": "unavailable",
            "reason": "no_frame_observed",
        })
    );
}

#[rstest]
#[tokio::test]
async fn source_effective_fps_is_a_compatible_published_frame_cadence_alias() {
    let (observation, publisher) = LiveObservation::from_test_stream(3, 7);
    publisher.publish_frame(8, "test_source_frame");

    let state = observation.state();
    assert_eq!(state["source_effective_fps"], state["recent_effective_fps"]);
    assert_eq!(
        state["source_effective_fps_semantics"],
        "published_source_frame_cadence_not_requested_fps"
    );
}

#[rstest]
fn showcase_projection_shares_the_live_frame_buffer() {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(9, vec![7; 16], 2, 2, std::time::Instant::now()),
        Duration::ZERO,
        "test_capture",
    );
    let source = status.latest().expect("source frame").shared_bgra();

    let projected = project_showcase_status(&status);
    let projected = projected.latest().expect("projected frame").shared_bgra();

    assert!(Arc::ptr_eq(&source, &projected));
}
