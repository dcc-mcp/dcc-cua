#[cfg(any(not(windows), test))]
use std::io::Cursor;
use std::sync::Arc;
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

const FIRST_FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

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

#[derive(Clone, Debug, Default)]
pub(crate) struct LiveObservationStatus {
    latest: Option<Arc<LiveObservationFrame>>,
    frames_captured: u64,
    frames_replaced: u64,
    capture_failures: u64,
    first_captured_at: Option<Instant>,
    previous_captured_at: Option<Instant>,
    recent_frame_interval_ms: Option<f64>,
    last_capture_duration_ms: Option<u64>,
    max_capture_duration_ms: u64,
    capture_mode: Option<&'static str>,
    last_error: Option<String>,
}

impl LiveObservationStatus {
    pub(crate) fn publish_frame(
        &mut self,
        frame: LiveObservationFrame,
        capture_duration: Duration,
        capture_mode: &'static str,
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
        let capture_duration_ms = u64::try_from(capture_duration.as_millis()).unwrap_or(u64::MAX);
        self.last_capture_duration_ms = Some(capture_duration_ms);
        self.max_capture_duration_ms = self.max_capture_duration_ms.max(capture_duration_ms);
        self.capture_mode = Some(capture_mode);
        if self.latest.is_some() {
            self.frames_replaced = self.frames_replaced.saturating_add(1);
        }
        self.latest = Some(Arc::new(frame));
        self.frames_captured = self.frames_captured.saturating_add(1);
        self.last_error = None;
    }

    fn record_error(&mut self, message: impl Into<String>) {
        self.capture_failures = self.capture_failures.saturating_add(1);
        self.last_error = Some(message.into());
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
        json!({
            "active": active,
            "target_fps": fps,
            "effective_fps": effective_fps,
            "recent_effective_fps": recent_effective_fps,
            "last_capture_duration_ms": self.last_capture_duration_ms,
            "max_capture_duration_ms": self.max_capture_duration_ms,
            "capture_mode": self.capture_mode.unwrap_or("initializing"),
            "latest_sequence": self.latest.as_ref().map(|frame| frame.sequence()),
            "latest_frame_age_ms": self.latest.as_ref().map(|frame| frame.age_ms()),
            "frames_captured": self.frames_captured,
            "frames_replaced": self.frames_replaced,
            "capture_failures": self.capture_failures,
            "last_error": self.last_error,
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

pub(crate) struct LiveObservation {
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
            crate::interactive_desktop::require_available()?;
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

    pub(crate) fn state(&self) -> Value {
        self.receiver
            .borrow()
            .as_json(!self.task.is_finished(), self.fps)
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
            Err(error) => sender.send_modify(|status| status.record_error(error.message)),
        }
        tokio::time::sleep(interval.saturating_sub(capture_started.elapsed())).await;
    }
}

pub(crate) fn terminal_capture_error(error: &ComputerUseError) -> bool {
    matches!(
        error.code,
        ComputerUseErrorCode::MissingWindow
            | ComputerUseErrorCode::InvalidTarget
            | ComputerUseErrorCode::InteractiveDesktopUnavailable
    )
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
        join_error_sender.send_modify(|status| {
            status.record_error(format!("persistent WGC worker failed: {error}"));
        });
    }
}

#[cfg(windows)]
enum WindowsLiveCapture {
    Persistent(dcc_cua_platform_windows::PersistentWgcCapture),
    OneShot,
}

#[cfg(windows)]
impl WindowsLiveCapture {
    fn new(window_handle: u64) -> Self {
        dcc_cua_platform_windows::PersistentWgcCapture::new(window_handle)
            .map_or(Self::OneShot, Self::Persistent)
    }

    fn next_frame(
        &mut self,
        window_handle: u64,
    ) -> ComputerUseResult<(Vec<u8>, u32, u32, &'static str)> {
        match self {
            Self::Persistent(capture) => match capture.next_frame(FIRST_FRAME_TIMEOUT) {
                Ok((bgra, width, height)) => Ok((bgra, width, height, "persistent_wgc")),
                Err(persistent_error) => platform_windows::wgc::screenshot_window_via_wgc(
                    window_handle,
                )
                .map(|(bgra, width, height)| (bgra, width, height, "one_shot_wgc_recovery"))
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
                .map(|(bgra, width, height)| (bgra, width, height, "one_shot_wgc_fallback"))
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
            Ok((bgra, width, height, capture_mode)) => {
                if interrupt_generation_changed(
                    started_interrupt_generation,
                    interrupt_generation(),
                ) {
                    return;
                }
                sequence = sequence.saturating_add(1);
                sender.send_modify(|status| {
                    status.publish_frame(
                        LiveObservationFrame::new(sequence, bgra, width, height, Instant::now()),
                        capture_started.elapsed(),
                        capture_mode,
                    );
                });
            }
            Err(error) => {
                if let Err(desktop_error) = crate::interactive_desktop::require_available() {
                    sender.send_modify(|status| status.record_error(desktop_error.message));
                    return;
                }
                let terminal = terminal_capture_error(&error);
                sender.send_modify(|status| status.record_error(error.message));
                if terminal {
                    return;
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
