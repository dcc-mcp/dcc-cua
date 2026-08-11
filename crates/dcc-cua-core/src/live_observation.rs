#[cfg(any(not(windows), test))]
use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dcc_cua_indicator::{interrupt_generation, interrupt_generation_changed};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::task::JoinHandle;

#[cfg(windows)]
use crate::window_target::windows_window_process_id;
use crate::{
    ComputerUseDriver, ComputerUseError, ComputerUseErrorCode,
    ComputerUseLiveObservationStartRequest, ComputerUseResult,
};

#[cfg(test)]
mod tests;

const FIRST_FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const PAUSE_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
static LIVE_OBSERVATION_STREAM_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LiveObservationFence {
    stream_id: u64,
    sequence: u64,
}

impl LiveObservationFence {
    pub(crate) const fn new(stream_id: u64, sequence: u64) -> Self {
        Self {
            stream_id,
            sequence,
        }
    }

    pub(crate) const fn sequence_for(self, stream_id: u64) -> Option<u64> {
        if self.stream_id == stream_id {
            Some(self.sequence)
        } else {
            None
        }
    }

    pub(crate) const fn stream_id(self) -> u64 {
        self.stream_id
    }
}

#[derive(Debug)]
pub(crate) struct LiveObservationFrame {
    sequence: u64,
    bgra: Vec<u8>,
    width: u32,
    height: u32,
    captured_at_ms: u128,
    captured_at: Instant,
}

impl LiveObservationFrame {
    pub(crate) fn new(
        sequence: u64,
        bgra: Vec<u8>,
        width: u32,
        height: u32,
        captured_at: Instant,
    ) -> Self {
        Self {
            sequence,
            bgra,
            width,
            height,
            captured_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis()),
            captured_at,
        }
    }

    pub(crate) fn sequence(&self) -> u64 {
        self.sequence
    }

    pub(crate) fn bgra(&self) -> &[u8] {
        &self.bgra
    }

    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub(crate) fn captured_at_ms(&self) -> u128 {
        self.captured_at_ms
    }

    pub(crate) fn captured_at(&self) -> Instant {
        self.captured_at
    }

    pub(crate) fn age_ms(&self) -> u128 {
        self.captured_at.elapsed().as_millis()
    }
}

#[derive(Clone, Debug)]
pub(crate) enum CompositorTiming {
    #[cfg(any(windows, test))]
    Available {
        system_relative_time_100ns: i64,
        compositor_to_publish: Duration,
    },
    Unavailable {
        reason: &'static str,
    },
}

impl CompositorTiming {
    pub(crate) const fn unavailable(reason: &'static str) -> Self {
        Self::Unavailable { reason }
    }

    fn as_json(&self) -> Value {
        match self {
            #[cfg(any(windows, test))]
            Self::Available {
                system_relative_time_100ns,
                compositor_to_publish,
            } => json!({
                "status": "available",
                "system_relative_time_100ns": system_relative_time_100ns,
                "compositor_to_publish_ms": duration_ms(*compositor_to_publish),
            }),
            Self::Unavailable { reason } => json!({
                "status": "unavailable",
                "reason": reason,
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FrameCaptureMeasurement {
    source_wait: Option<Duration>,
    readback_total: Option<Duration>,
    gpu_copy_map: Option<Duration>,
    cpu_copy: Option<Duration>,
    compositor: CompositorTiming,
}

impl FrameCaptureMeasurement {
    #[cfg(any(windows, test))]
    pub(crate) const fn measured(
        source_wait: Duration,
        readback_total: Duration,
        gpu_copy_map: Duration,
        cpu_copy: Duration,
        compositor: CompositorTiming,
    ) -> Self {
        Self {
            source_wait: Some(source_wait),
            readback_total: Some(readback_total),
            gpu_copy_map: Some(gpu_copy_map),
            cpu_copy: Some(cpu_copy),
            compositor,
        }
    }

    const fn unavailable(reason: &'static str) -> Self {
        Self {
            source_wait: None,
            readback_total: None,
            gpu_copy_map: None,
            cpu_copy: None,
            compositor: CompositorTiming::unavailable(reason),
        }
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn update_optional_max(maximum: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *maximum = Some(maximum.map_or(value, |current| current.max(value)));
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LiveObservationStatus {
    latest: Option<Arc<LiveObservationFrame>>,
    frames_captured: u64,
    frames_replaced: u64,
    capture_failures: u64,
    first_captured_at: Option<Instant>,
    previous_captured_at: Option<Instant>,
    recent_frame_interval_ms: Option<f64>,
    /// Compatibility metric for the whole capture cycle. It includes source
    /// frame wait, backend readback, and exact-target validation; it is not a
    /// frame-processing latency metric.
    last_capture_duration_ms: Option<u64>,
    max_capture_duration_ms: u64,
    last_source_wait_ms: Option<u64>,
    max_source_wait_ms: Option<u64>,
    last_readback_total_ms: Option<u64>,
    max_readback_total_ms: Option<u64>,
    last_gpu_copy_map_ms: Option<u64>,
    max_gpu_copy_map_ms: Option<u64>,
    last_cpu_copy_ms: Option<u64>,
    max_cpu_copy_ms: Option<u64>,
    last_compositor_timing: Option<CompositorTiming>,
    max_compositor_to_publish_ms: Option<u64>,
    capture_mode: Option<&'static str>,
    last_error: Option<String>,
    pause_reason: Option<LiveObservationReason>,
    terminal_reason: Option<LiveObservationReason>,
}

#[derive(Clone, Debug)]
struct LiveObservationReason {
    code: ComputerUseErrorCode,
    message: String,
    timestamp_ms: u128,
    last_sequence: Option<u64>,
}

impl LiveObservationReason {
    fn from_error(error: &ComputerUseError, last_sequence: Option<u64>) -> Self {
        Self {
            code: error.code,
            message: error.message.clone(),
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis()),
            last_sequence,
        }
    }

    fn as_json(&self) -> Value {
        json!({
            "code": self.code,
            "message": self.message,
            "timestamp_ms": self.timestamp_ms,
            "last_sequence": self.last_sequence,
        })
    }
}

impl LiveObservationStatus {
    #[cfg(any(not(windows), test))]
    pub(crate) fn publish_frame(
        &mut self,
        frame: LiveObservationFrame,
        capture_duration: Duration,
        capture_mode: &'static str,
    ) {
        self.publish_measured_frame(
            frame,
            capture_duration,
            capture_mode,
            FrameCaptureMeasurement::unavailable("capture_mode_does_not_report_split_timing"),
        );
    }

    pub(crate) fn publish_measured_frame(
        &mut self,
        frame: LiveObservationFrame,
        capture_duration: Duration,
        capture_mode: &'static str,
        measurement: FrameCaptureMeasurement,
    ) {
        self.first_captured_at.get_or_insert(frame.captured_at);
        if let Some(previous) = self.previous_captured_at {
            let interval_ms = frame
                .captured_at
                .saturating_duration_since(previous)
                .as_secs_f64()
                * 1000.0;
            if interval_ms > 0.0 {
                self.recent_frame_interval_ms = Some(
                    self.recent_frame_interval_ms
                        .map_or(interval_ms, |current| current * 0.8 + interval_ms * 0.2),
                );
            }
        }
        self.previous_captured_at = Some(frame.captured_at);
        let capture_duration_ms = duration_ms(capture_duration);
        self.last_capture_duration_ms = Some(capture_duration_ms);
        self.max_capture_duration_ms = self.max_capture_duration_ms.max(capture_duration_ms);
        self.last_source_wait_ms = measurement.source_wait.map(duration_ms);
        self.last_readback_total_ms = measurement.readback_total.map(duration_ms);
        self.last_gpu_copy_map_ms = measurement.gpu_copy_map.map(duration_ms);
        self.last_cpu_copy_ms = measurement.cpu_copy.map(duration_ms);
        update_optional_max(&mut self.max_source_wait_ms, self.last_source_wait_ms);
        update_optional_max(&mut self.max_readback_total_ms, self.last_readback_total_ms);
        update_optional_max(&mut self.max_gpu_copy_map_ms, self.last_gpu_copy_map_ms);
        update_optional_max(&mut self.max_cpu_copy_ms, self.last_cpu_copy_ms);
        let compositor_to_publish_ms = match &measurement.compositor {
            #[cfg(any(windows, test))]
            CompositorTiming::Available {
                compositor_to_publish,
                ..
            } => Some(duration_ms(*compositor_to_publish)),
            CompositorTiming::Unavailable { .. } => None,
        };
        update_optional_max(
            &mut self.max_compositor_to_publish_ms,
            compositor_to_publish_ms,
        );
        self.last_compositor_timing = Some(measurement.compositor);
        self.capture_mode = Some(capture_mode);
        if self.latest.is_some() {
            self.frames_replaced = self.frames_replaced.saturating_add(1);
        }
        self.latest = Some(Arc::new(frame));
        self.frames_captured = self.frames_captured.saturating_add(1);
        self.last_error = None;
        self.pause_reason = None;
    }

    fn record_error(&mut self, message: impl Into<String>) {
        self.capture_failures = self.capture_failures.saturating_add(1);
        self.last_error = Some(message.into());
    }

    pub(crate) fn record_terminal_error(&mut self, error: &ComputerUseError) {
        if pause_capture_error(error) {
            self.record_paused_error(error);
            return;
        }
        self.record_error(error.message.clone());
        self.pause_reason = None;
        self.terminal_reason = Some(LiveObservationReason::from_error(
            error,
            self.latest.as_ref().map(|frame| frame.sequence()),
        ));
    }

    pub(crate) fn record_paused_error(&mut self, error: &ComputerUseError) {
        if self.terminal_reason.is_some() {
            return;
        }
        self.record_error(error.message.clone());
        self.pause_reason = Some(LiveObservationReason::from_error(
            error,
            self.latest.as_ref().map(|frame| frame.sequence()),
        ));
    }

    pub(crate) fn pause_reason(&self) -> Option<Value> {
        self.pause_reason
            .as_ref()
            .map(LiveObservationReason::as_json)
    }

    pub(crate) fn terminal_reason(&self) -> Option<Value> {
        self.terminal_reason
            .as_ref()
            .map(LiveObservationReason::as_json)
    }

    fn pause_error(&self) -> Option<ComputerUseError> {
        self.pause_reason
            .as_ref()
            .map(|reason| ComputerUseError::new(reason.code, reason.message.clone()))
    }

    fn terminal_error(&self) -> Option<ComputerUseError> {
        self.terminal_reason
            .as_ref()
            .map(|reason| ComputerUseError::new(reason.code, reason.message.clone()))
    }

    pub(crate) fn latest(&self) -> Option<Arc<LiveObservationFrame>> {
        self.latest.clone()
    }

    pub(crate) fn as_json(&self, active: bool, fps: u32) -> Value {
        let effective_fps = self.latest.as_ref().and_then(|latest| {
            let frame_intervals = self.frames_captured.saturating_sub(1);
            let seconds = latest
                .captured_at
                .saturating_duration_since(self.first_captured_at?)
                .as_secs_f64();
            (frame_intervals > 0 && seconds > 0.0).then(|| frame_intervals as f64 / seconds)
        });
        let recent_effective_fps = self
            .recent_frame_interval_ms
            .filter(|interval_ms| *interval_ms > 0.0)
            .map(|interval_ms| 1000.0 / interval_ms);
        let last_compositor_timing = self.last_compositor_timing.as_ref().map_or_else(
            || CompositorTiming::unavailable("no_frame_observed").as_json(),
            CompositorTiming::as_json,
        );
        json!({
            "active": active,
            "target_fps": fps,
            "effective_fps": effective_fps,
            "recent_effective_fps": recent_effective_fps,
            "source_effective_fps": recent_effective_fps,
            "source_effective_fps_semantics": "published_source_frame_cadence_not_requested_fps",
            "last_capture_duration_ms": self.last_capture_duration_ms,
            "max_capture_duration_ms": self.max_capture_duration_ms,
            "capture_duration_semantics": "capture_cycle_wall_time_including_source_wait_readback_and_target_validation",
            "last_source_wait_ms": self.last_source_wait_ms,
            "max_source_wait_ms": self.max_source_wait_ms,
            "last_readback_total_ms": self.last_readback_total_ms,
            "max_readback_total_ms": self.max_readback_total_ms,
            "readback_total_semantics": "frame_processing_after_source_arrival",
            "last_gpu_copy_map_ms": self.last_gpu_copy_map_ms,
            "max_gpu_copy_map_ms": self.max_gpu_copy_map_ms,
            "last_cpu_copy_ms": self.last_cpu_copy_ms,
            "max_cpu_copy_ms": self.max_cpu_copy_ms,
            "last_compositor_timing": last_compositor_timing,
            "max_compositor_to_publish_ms": self.max_compositor_to_publish_ms,
            "capture_mode": self.capture_mode.unwrap_or("initializing"),
            "latest_sequence": self.latest.as_ref().map(|frame| frame.sequence()),
            "latest_frame_age_ms": self.latest.as_ref().map(|frame| frame.age_ms()),
            "frames_captured": self.frames_captured,
            "frames_replaced": self.frames_replaced,
            "capture_failures": self.capture_failures,
            "last_error": self.last_error,
            "paused": self.pause_reason.is_some(),
            "pause_reason": self.pause_reason(),
            "terminal_reason": self.terminal_reason(),
        })
    }
}

pub(crate) async fn wait_for_latest_frame(
    receiver: &mut watch::Receiver<LiveObservationStatus>,
    after_sequence: Option<u64>,
    wait: std::time::Duration,
) -> ComputerUseResult<Arc<LiveObservationFrame>> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let status = receiver.borrow_and_update().clone();
        if let Some(error) = status.terminal_error() {
            return Err(error);
        }
        if let Some(error) = status.pause_error() {
            return Err(error);
        }
        if let Some(frame) = status
            .latest()
            .filter(|frame| after_sequence.is_none_or(|sequence| frame.sequence() > sequence))
        {
            return Ok(frame);
        }
        match tokio::time::timeout_at(deadline, receiver.changed()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::CaptureFailed,
                    "live observation stopped before a frame was available",
                ));
            }
            Err(_) => {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::CaptureFailed,
                    status.last_error.unwrap_or_else(|| {
                        "live observation did not produce a fresh frame before the timeout".into()
                    }),
                ));
            }
        }
    }
}

/// Require a live frame newer than every evidence invalidation boundary.
/// Transition fences are independent from post-action fences because input or
/// target suspension preserves the producer while invalidating consumers.
pub(crate) fn observation_sequence_fence(
    stream_id: u64,
    decision: Option<LiveObservationFence>,
    action_completion: Option<LiveObservationFence>,
    transition: Option<LiveObservationFence>,
) -> Option<u64> {
    decision
        .and_then(|fence| fence.sequence_for(stream_id))
        .max(action_completion.and_then(|fence| fence.sequence_for(stream_id)))
        .max(transition.and_then(|fence| fence.sequence_for(stream_id)))
}

pub(crate) struct LiveObservation {
    stream_id: u64,
    fps: u32,
    max_dimension: u32,
    receiver: watch::Receiver<LiveObservationStatus>,
    task: JoinHandle<()>,
}

impl LiveObservation {
    pub(crate) async fn start(
        driver: ComputerUseDriver,
        session_id: String,
        process_id: u32,
        window_handle: u64,
        request: &ComputerUseLiveObservationStartRequest,
    ) -> ComputerUseResult<Self> {
        request.validate()?;
        let stream_id = LIVE_OBSERVATION_STREAM_COUNTER.fetch_add(1, Ordering::Relaxed);
        #[cfg(not(windows))]
        {
            let fps = request.fps;
            let (sender, receiver) = watch::channel(LiveObservationStatus::default());
            let task = tokio::spawn(run_portable_capture_loop(
                driver,
                session_id,
                process_id,
                window_handle,
                fps,
                interrupt_generation(),
                sender,
            ));
            let mut observation = Self {
                stream_id,
                fps,
                max_dimension: request.max_dimension,
                receiver,
                task,
            };
            wait_for_latest_frame(&mut observation.receiver, None, FIRST_FRAME_TIMEOUT).await?;
            Ok(observation)
        }
        #[cfg(windows)]
        {
            let _ = (driver, session_id);
            crate::interactive_desktop::require_exact_window_observation_available()?;
            ensure_window_owner(process_id, window_handle)?;
            let fps = request.fps;
            let (sender, receiver) = watch::channel(LiveObservationStatus::default());
            let task = tokio::spawn(run_capture_loop(
                process_id,
                window_handle,
                fps,
                interrupt_generation(),
                sender,
            ));
            let mut observation = Self {
                stream_id,
                fps,
                max_dimension: request.max_dimension,
                receiver,
                task,
            };
            wait_for_latest_frame(&mut observation.receiver, None, FIRST_FRAME_TIMEOUT).await?;
            Ok(observation)
        }
    }

    pub(crate) async fn latest_after(
        &mut self,
        sequence: Option<u64>,
    ) -> ComputerUseResult<Arc<LiveObservationFrame>> {
        wait_for_latest_frame(&mut self.receiver, sequence, FIRST_FRAME_TIMEOUT).await
    }

    pub(crate) fn latest_fence(&self) -> Option<LiveObservationFence> {
        self.receiver
            .borrow()
            .latest()
            .map(|frame| LiveObservationFence::new(self.stream_id, frame.sequence()))
    }

    pub(crate) const fn stream_id(&self) -> u64 {
        self.stream_id
    }

    pub(crate) fn state(&self) -> Value {
        let mut state = self
            .receiver
            .borrow()
            .as_json(!self.task.is_finished(), self.fps);
        state["stream_id"] = json!(self.stream_id);
        state
    }

    pub(crate) fn is_active(&self) -> bool {
        !self.task.is_finished()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<LiveObservationStatus> {
        self.receiver.clone()
    }

    pub(crate) const fn fps(&self) -> u32 {
        self.fps
    }

    pub(crate) const fn max_dimension(&self) -> u32 {
        self.max_dimension
    }

    pub(crate) async fn stop(mut self) -> Value {
        self.task.abort();
        let _ = (&mut self.task).await;
        self.receiver.borrow().as_json(false, self.fps)
    }
}

#[cfg(not(windows))]
async fn run_portable_capture_loop(
    driver: ComputerUseDriver,
    session_id: String,
    process_id: u32,
    window_handle: u64,
    fps: u32,
    started_interrupt_generation: u64,
    sender: watch::Sender<LiveObservationStatus>,
) {
    let interval = std::time::Duration::from_secs_f64(1.0 / f64::from(fps));
    let mut sequence = 0_u64;
    loop {
        if sender.is_closed()
            || interrupt_generation_changed(started_interrupt_generation, interrupt_generation())
        {
            return;
        }
        let capture_started = Instant::now();
        match driver
            .capture_exact_window_png(process_id, window_handle, &session_id)
            .await
            .and_then(|png| decode_png_to_bgra(&png))
        {
            Ok((bgra, width, height)) => {
                sequence = sequence.saturating_add(1);
                sender.send_modify(|status| {
                    status.publish_frame(
                        LiveObservationFrame::new(sequence, bgra, width, height, Instant::now()),
                        capture_started.elapsed(),
                        "driver_png_decode",
                    );
                });
            }
            Err(error) => match capture_failure_disposition(error) {
                CaptureFailureDisposition::Terminal(error) => {
                    sender.send_modify(|status| status.record_terminal_error(&error));
                    return;
                }
                CaptureFailureDisposition::Pause(error) => {
                    sender.send_modify(|status| status.record_paused_error(&error));
                    tokio::time::sleep(PAUSE_RETRY_INTERVAL).await;
                    continue;
                }
                CaptureFailureDisposition::Retry(error) => {
                    sender.send_modify(|status| status.record_error(error.message));
                }
            },
        }
        tokio::time::sleep(interval.saturating_sub(capture_started.elapsed())).await;
    }
}

pub(crate) fn terminal_capture_error(error: &ComputerUseError) -> bool {
    matches!(
        error.code,
        ComputerUseErrorCode::MissingWindow | ComputerUseErrorCode::InvalidTarget
    )
}

fn pause_capture_error(error: &ComputerUseError) -> bool {
    error.code == ComputerUseErrorCode::InteractiveDesktopUnavailable
}

#[derive(Debug)]
pub(crate) enum CaptureFailureDisposition {
    Retry(ComputerUseError),
    Pause(ComputerUseError),
    Terminal(ComputerUseError),
}

fn capture_failure_disposition(error: ComputerUseError) -> CaptureFailureDisposition {
    if pause_capture_error(&error) {
        CaptureFailureDisposition::Pause(error)
    } else if terminal_capture_error(&error) {
        CaptureFailureDisposition::Terminal(error)
    } else {
        CaptureFailureDisposition::Retry(error)
    }
}

#[cfg(any(windows, test))]
pub(crate) fn live_capture_failure_disposition(
    capture_error: ComputerUseError,
    observation_availability: ComputerUseResult<()>,
) -> CaptureFailureDisposition {
    if let Err(desktop_error) = observation_availability {
        return capture_failure_disposition(desktop_error);
    }
    capture_failure_disposition(capture_error)
}

#[cfg(any(not(windows), test))]
pub(crate) fn decode_png_to_bgra(data: &[u8]) -> ComputerUseResult<(Vec<u8>, u32, u32)> {
    let mut decoder = png::Decoder::new(Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(capture_error)?;
    let mut decoded = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut decoded).map_err(capture_error)?;
    let pixels = &decoded[..info.buffer_size()];
    let pixel_count = usize::try_from(info.width)
        .ok()
        .and_then(|width| {
            usize::try_from(info.height)
                .ok()
                .map(|height| width * height)
        })
        .ok_or_else(|| capture_error("PNG dimensions overflow"))?;
    let mut bgra = Vec::with_capacity(pixel_count * 4);
    match info.color_type {
        png::ColorType::Rgba => pixels.chunks_exact(4).for_each(|pixel| {
            bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }),
        png::ColorType::Rgb => pixels.chunks_exact(3).for_each(|pixel| {
            bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        }),
        png::ColorType::Grayscale => pixels.iter().for_each(|value| {
            bgra.extend_from_slice(&[*value, *value, *value, 255]);
        }),
        png::ColorType::GrayscaleAlpha => pixels.chunks_exact(2).for_each(|pixel| {
            bgra.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
        }),
        png::ColorType::Indexed => {
            return Err(capture_error("indexed PNG was not expanded by the decoder"));
        }
    }
    if bgra.len() != pixel_count * 4 {
        return Err(capture_error("PNG pixel buffer has an invalid length"));
    }
    Ok((bgra, info.width, info.height))
}

#[cfg(any(not(windows), test))]
fn capture_error(error: impl std::fmt::Display) -> ComputerUseError {
    ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
}

impl Drop for LiveObservation {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(windows)]
async fn run_capture_loop(
    process_id: u32,
    window_handle: u64,
    fps: u32,
    started_interrupt_generation: u64,
    sender: watch::Sender<LiveObservationStatus>,
) {
    let join_error_sender = sender.clone();
    if let Err(error) = tokio::task::spawn_blocking(move || {
        run_windows_capture_loop(
            process_id,
            window_handle,
            fps,
            started_interrupt_generation,
            sender,
        );
    })
    .await
    {
        let error = ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            format!("persistent WGC worker failed: {error}"),
        );
        join_error_sender.send_modify(|status| {
            status.record_terminal_error(&error);
        });
    }
}

#[cfg(windows)]
enum WindowsLiveCapture {
    Persistent(dcc_cua_platform_windows::PersistentWgcCapture),
    OneShot,
}

#[cfg(windows)]
struct WindowsCapturedFrame {
    bgra: Vec<u8>,
    width: u32,
    height: u32,
    capture_mode: &'static str,
    measurement: Option<dcc_cua_platform_windows::WgcFrameMeasurement>,
}

#[cfg(windows)]
impl From<dcc_cua_platform_windows::WgcPublishedFrameMeasurement> for FrameCaptureMeasurement {
    fn from(measurement: dcc_cua_platform_windows::WgcPublishedFrameMeasurement) -> Self {
        let compositor = match measurement.compositor {
            dcc_cua_platform_windows::WgcCompositorTiming::Available {
                system_relative_time_100ns,
                compositor_to_publish,
            } => CompositorTiming::Available {
                system_relative_time_100ns,
                compositor_to_publish,
            },
            dcc_cua_platform_windows::WgcCompositorTiming::Unavailable { reason } => {
                CompositorTiming::unavailable(reason.as_str())
            }
        };
        Self::measured(
            measurement.source_wait,
            measurement.readback_total,
            measurement.gpu_copy_map,
            measurement.cpu_copy,
            compositor,
        )
    }
}

#[cfg(windows)]
impl WindowsLiveCapture {
    fn new(window_handle: u64) -> Self {
        dcc_cua_platform_windows::PersistentWgcCapture::new(window_handle)
            .map_or(Self::OneShot, Self::Persistent)
    }

    fn next_frame(&mut self, window_handle: u64) -> ComputerUseResult<WindowsCapturedFrame> {
        match self {
            Self::Persistent(capture) => match capture.next_measured_frame(FIRST_FRAME_TIMEOUT) {
                Ok(frame) => Ok(WindowsCapturedFrame {
                    bgra: frame.bgra,
                    width: frame.width,
                    height: frame.height,
                    capture_mode: "persistent_wgc",
                    measurement: Some(frame.measurement),
                }),
                Err(persistent_error) => platform_windows::wgc::screenshot_window_via_wgc(
                    window_handle,
                )
                .map(|(bgra, width, height)| WindowsCapturedFrame {
                    bgra,
                    width,
                    height,
                    capture_mode: "one_shot_wgc_recovery",
                    measurement: None,
                })
                .map_err(|fallback_error| {
                    ComputerUseError::new(
                        ComputerUseErrorCode::CaptureFailed,
                        format!(
                            "{persistent_error}; one-shot WGC recovery failed: {fallback_error}"
                        ),
                    )
                }),
            },
            Self::OneShot => platform_windows::wgc::screenshot_window_via_wgc(window_handle)
                .map(|(bgra, width, height)| WindowsCapturedFrame {
                    bgra,
                    width,
                    height,
                    capture_mode: "one_shot_wgc_fallback",
                    measurement: None,
                })
                .map_err(|error| {
                    ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
                }),
        }
    }
}

#[cfg(windows)]
fn run_windows_capture_loop(
    process_id: u32,
    window_handle: u64,
    fps: u32,
    started_interrupt_generation: u64,
    sender: watch::Sender<LiveObservationStatus>,
) {
    let interval = std::time::Duration::from_secs_f64(1.0 / f64::from(fps));
    let mut sequence = 0_u64;
    let mut capture = WindowsLiveCapture::new(window_handle);
    loop {
        if sender.is_closed()
            || interrupt_generation_changed(started_interrupt_generation, interrupt_generation())
        {
            return;
        }
        let capture_started = Instant::now();
        match ensure_window_owner(process_id, window_handle)
            .and_then(|()| capture.next_frame(window_handle))
            .and_then(|frame| {
                ensure_window_owner(process_id, window_handle)?;
                Ok(frame)
            }) {
            Ok(frame) => {
                if interrupt_generation_changed(
                    started_interrupt_generation,
                    interrupt_generation(),
                ) {
                    return;
                }
                sequence = sequence.saturating_add(1);
                let measurement = frame.measurement.map_or_else(
                    || {
                        FrameCaptureMeasurement::unavailable(
                            "capture_mode_does_not_report_split_timing",
                        )
                    },
                    |measurement| FrameCaptureMeasurement::from(measurement.at_publish()),
                );
                sender.send_modify(|status| {
                    status.publish_measured_frame(
                        LiveObservationFrame::new(
                            sequence,
                            frame.bgra,
                            frame.width,
                            frame.height,
                            Instant::now(),
                        ),
                        capture_started.elapsed(),
                        frame.capture_mode,
                        measurement,
                    );
                });
            }
            Err(error) => {
                match live_capture_failure_disposition(
                    error,
                    crate::interactive_desktop::require_exact_window_observation_available(),
                ) {
                    CaptureFailureDisposition::Terminal(error) => {
                        sender.send_modify(|status| status.record_terminal_error(&error));
                        return;
                    }
                    CaptureFailureDisposition::Pause(error) => {
                        sender.send_modify(|status| status.record_paused_error(&error));
                        std::thread::sleep(PAUSE_RETRY_INTERVAL);
                        continue;
                    }
                    CaptureFailureDisposition::Retry(error) => {
                        sender.send_modify(|status| status.record_error(error.message));
                    }
                }
            }
        }
        std::thread::sleep(interval.saturating_sub(capture_started.elapsed()));
    }
}

#[cfg(windows)]
fn ensure_window_owner(process_id: u32, window_handle: u64) -> ComputerUseResult<()> {
    if windows_window_process_id(window_handle)? != process_id {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTarget,
            "live observation target process identity changed",
        ));
    }
    Ok(())
}
