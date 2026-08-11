use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use mp4::{AvcConfig, FourCC, Mp4Config, Mp4Sample, Mp4Writer};
use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, UsageType,
};
use openh264::formats::{BgraSliceU8, YUVBuffer};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::live_observation::{LiveObservationFrame, LiveObservationStatus};
use crate::{ComputerUseError, ComputerUseErrorCode, ComputerUseResult};

const MAX_WIDTH: u32 = 1600;
const MAX_HEIGHT: u32 = 900;
const SEGMENT_DURATION_MS: u64 = 5 * 60 * 1000;

struct PendingSample {
    start_time: u64,
    is_sync: bool,
    bytes: Vec<u8>,
}

struct FirstShowcaseFrame {
    frame: Arc<LiveObservationFrame>,
    resumed_acknowledgement: Option<oneshot::Sender<ComputerUseResult<()>>>,
}

enum ShowcaseProducerEvent {
    Frame(Arc<LiveObservationFrame>),
    /// A first frame after a pause. The encoder acknowledges only after the
    /// forced-IDR segment and its partial file are ready.
    ResumedFrame(
        Arc<LiveObservationFrame>,
        oneshot::Sender<ComputerUseResult<()>>,
    ),
    /// A producer-observed pause boundary. The acknowledgement keeps the
    /// public paused state behind durable segment finalization.
    Paused(oneshot::Sender<ComputerUseResult<()>>),
}

struct ShowcaseProducerStop {
    acknowledged: oneshot::Sender<()>,
}

struct ShowcaseProducer {
    frames: watch::Receiver<LiveObservationStatus>,
    sender: mpsc::Sender<ShowcaseProducerEvent>,
    last_forwarded_sequence: Option<u64>,
    paused: bool,
    pause_reason: Arc<Mutex<Option<Value>>>,
    terminal_reason: Arc<Mutex<Option<Value>>>,
}

impl ShowcaseProducer {
    async fn run(mut self, mut stop_requested: oneshot::Receiver<ShowcaseProducerStop>) {
        if !self.initialize().await {
            return;
        }
        loop {
            tokio::select! {
                biased;
                stop = &mut stop_requested => {
                    if let Ok(stop) = stop {
                        self.flush_for_stop().await;
                        let _ = stop.acknowledged.send(());
                    }
                    return;
                }
                changed = self.frames.changed() => {
                    if changed.is_err() {
                        self.flush_for_stop().await;
                        return;
                    }
                    if !self.synchronize_changed_status().await {
                        return;
                    }
                }
            }
        }
    }

    async fn initialize(&mut self) -> bool {
        let status = self.frames.borrow_and_update().clone();
        self.apply_status(&status, false).await
    }

    async fn synchronize_changed_status(&mut self) -> bool {
        let status = self.frames.borrow_and_update().clone();
        self.apply_status(&status, false).await
    }

    async fn flush_for_stop(&mut self) {
        let status = self.frames.borrow_and_update().clone();
        self.apply_status(&status, true).await;
    }

    async fn apply_status(&mut self, status: &LiveObservationStatus, guaranteed: bool) -> bool {
        let source_pause_reason = status.pause_reason();
        if let Some(reason) = source_pause_reason {
            if self.last_forwarded_sequence.is_none() && status.latest().is_none() {
                // No recorder handle exists until the encoder has a first
                // frame. Closing the producer makes startup fail with a typed
                // error instead of waiting forever on a paused source.
                return false;
            }
            if !self.paused && !self.enter_pause(status).await {
                return false;
            }
            self.project_applied_pause(Some(reason));
        } else if self.paused {
            match self.resume(status).await {
                GuaranteedFrameSend::Forwarded => self.project_applied_pause(None),
                GuaranteedFrameSend::NoNewFrame => {
                    self.latch_terminal_reason(status);
                    return true;
                }
                GuaranteedFrameSend::ReceiverClosed => return false,
            }
        } else {
            let sent = if guaranteed {
                send_latest_guaranteed(status, &self.sender, &mut self.last_forwarded_sequence)
                    .await
            } else {
                send_latest(status, &self.sender, &mut self.last_forwarded_sequence);
                GuaranteedFrameSend::NoNewFrame
            };
            if sent == GuaranteedFrameSend::ReceiverClosed {
                return false;
            }
            self.project_applied_pause(None);
        }
        self.latch_terminal_reason(status);
        true
    }

    async fn enter_pause(&mut self, status: &LiveObservationStatus) -> bool {
        if send_latest_guaranteed(status, &self.sender, &mut self.last_forwarded_sequence).await
            == GuaranteedFrameSend::ReceiverClosed
            || !send_pause_boundary(&self.sender).await
        {
            return false;
        }
        self.paused = true;
        true
    }

    async fn resume(&mut self, status: &LiveObservationStatus) -> GuaranteedFrameSend {
        let sent =
            send_resume_frame_guaranteed(status, &self.sender, &mut self.last_forwarded_sequence)
                .await;
        if sent == GuaranteedFrameSend::Forwarded {
            self.paused = false;
        }
        sent
    }

    fn project_applied_pause(&self, reason: Option<Value>) {
        *lock_unpoisoned(&self.pause_reason) = reason;
    }

    fn latch_terminal_reason(&self, status: &LiveObservationStatus) {
        latch_terminal_reason(status, &self.terminal_reason);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuaranteedFrameSend {
    Forwarded,
    NoNewFrame,
    ReceiverClosed,
}

#[derive(Clone, Copy)]
struct VideoFormat {
    width: u32,
    height: u32,
    fps: u32,
}

struct SegmentStart<'path> {
    path: &'path Path,
    index: u32,
    start_ms: u64,
    format: VideoFormat,
    nominal_duration: u32,
}

struct ActiveSegment {
    index: u32,
    path: PathBuf,
    partial_path: PathBuf,
    start_ms: u64,
    frame_count: u64,
    final_duration: u32,
    writer: Mp4Writer<File>,
    pending: PendingSample,
}

#[derive(Clone, serde::Serialize)]
struct ShowcaseSegment {
    index: u32,
    path: PathBuf,
    start_ms: u64,
    duration_ms: u64,
    frames: u64,
    width: u32,
    height: u32,
    fps: u32,
    finalized: bool,
}

#[derive(Clone, Default)]
struct ShowcaseProgress {
    segments: Vec<ShowcaseSegment>,
    current_partial: Option<PathBuf>,
}

pub(crate) struct ShowcaseRecorder {
    path: PathBuf,
    stop_producer: Option<oneshot::Sender<ShowcaseProducerStop>>,
    producer: JoinHandle<()>,
    encoder: JoinHandle<ComputerUseResult<Value>>,
    pause_reason: Arc<Mutex<Option<Value>>>,
    terminal_reason: Arc<Mutex<Option<Value>>>,
    outcome: Arc<Mutex<Option<Value>>>,
    progress: Arc<Mutex<ShowcaseProgress>>,
}

impl ShowcaseRecorder {
    pub(crate) async fn start(
        frames: watch::Receiver<LiveObservationStatus>,
        output_dir: &str,
        fps: u32,
    ) -> ComputerUseResult<Self> {
        {
            let initial_status = frames.borrow();
            if initial_status.latest().is_none() && initial_status.pause_reason().is_some() {
                return Err(capture_error(
                    "live observation paused before its first frame; showcase was not started",
                ));
            }
        }
        let path = Path::new(output_dir).join("showcase.mp4");
        let (frame_sender, frame_receiver) = mpsc::channel(2);
        let (ready_sender, ready_receiver) = oneshot::channel();
        let (stop_producer, stop_requested) = oneshot::channel::<ShowcaseProducerStop>();
        let pause_reason = Arc::new(Mutex::new(None));
        let terminal_reason = Arc::new(Mutex::new(None));
        let outcome = Arc::new(Mutex::new(None));
        let progress = Arc::new(Mutex::new(ShowcaseProgress::default()));
        let encoder_path = path.clone();
        let encoder_terminal_reason = Arc::clone(&terminal_reason);
        let encoder_outcome = Arc::clone(&outcome);
        let encoder_progress = Arc::clone(&progress);
        let encoder = tokio::task::spawn_blocking(move || {
            let result = encode_frames_with_progress(
                frame_receiver,
                &encoder_path,
                fps,
                ready_sender,
                &encoder_progress,
            );
            let terminal_reason = lock_unpoisoned(&encoder_terminal_reason).clone();
            let progress = lock_unpoisoned(&encoder_progress).clone();
            let state = match &result {
                Ok(state) => attach_terminal_reason(state.clone(), terminal_reason),
                Err(error) => json!({
                    "active": false,
                    "backend": "embedded-openh264",
                    "path": encoder_path.to_string_lossy(),
                    "manifest_path": manifest_output_path(&encoder_path).to_string_lossy(),
                    "finalized": false,
                    "segments": progress.segments,
                    "current_partial": progress.current_partial,
                    "error": {
                        "code": error.code,
                        "message": error.message,
                    },
                    "terminal_reason": terminal_reason,
                }),
            };
            *lock_unpoisoned(&encoder_outcome) = Some(state.clone());
            result.map(|_| state)
        });
        let producer_pause_reason = Arc::clone(&pause_reason);
        let producer_terminal_reason = Arc::clone(&terminal_reason);
        let producer = tokio::spawn(
            ShowcaseProducer {
                frames,
                sender: frame_sender,
                last_forwarded_sequence: None,
                paused: false,
                pause_reason: producer_pause_reason,
                terminal_reason: producer_terminal_reason,
            }
            .run(stop_requested),
        );
        match ready_receiver.await {
            Ok(Ok(())) => Ok(Self {
                path,
                stop_producer: Some(stop_producer),
                producer,
                encoder,
                pause_reason,
                terminal_reason,
                outcome,
                progress,
            }),
            Ok(Err(error)) => {
                producer.abort();
                Err(error)
            }
            Err(_) => {
                producer.abort();
                Err(ComputerUseError::new(
                    ComputerUseErrorCode::CaptureFailed,
                    "showcase encoder stopped before its first frame",
                ))
            }
        }
    }

    pub(crate) fn state(&self) -> Value {
        if let Some(outcome) = lock_unpoisoned(&self.outcome).clone() {
            return outcome;
        }
        let progress = lock_unpoisoned(&self.progress).clone();
        let manifest_path = manifest_output_path(&self.path);
        if let Some(terminal_reason) = lock_unpoisoned(&self.terminal_reason).clone() {
            return json!({
                "active": false,
                "backend": "embedded-openh264",
                "path": self.path.to_string_lossy(),
                "manifest_path": manifest_path.to_string_lossy(),
                "finalized": false,
                "segments": progress.segments,
                "current_partial": progress.current_partial,
                "paused": false,
                "pause_reason": Value::Null,
                "terminal_reason": terminal_reason,
            });
        }
        let active = !self.encoder.is_finished();
        let pause_reason = lock_unpoisoned(&self.pause_reason).clone();
        let paused = active && pause_reason.is_some();
        json!({
            "active": active,
            "backend": "embedded-openh264",
            "path": self.path.to_string_lossy(),
            "manifest_path": manifest_path.to_string_lossy(),
            "segments": progress.segments,
            "current_partial": progress.current_partial,
            "paused": paused,
            "pause_reason": pause_reason,
            "terminal_reason": Value::Null,
        })
    }

    pub(crate) async fn stop(mut self) -> ComputerUseResult<Value> {
        let producer_acknowledgement = self.stop_producer.take().and_then(|stop_producer| {
            let (acknowledged, acknowledgement) = oneshot::channel();
            stop_producer
                .send(ShowcaseProducerStop { acknowledged })
                .ok()
                .map(|_| acknowledgement)
        });
        if let Some(acknowledgement) = producer_acknowledgement {
            // A naturally closed live-observation source may win the select
            // after the request was sent. Joining producer and encoder below
            // is still the authoritative, fully-drained stop acknowledgement.
            let _ = acknowledgement.await;
        }
        let producer_result = (&mut self.producer).await.map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                format!("showcase producer task failed: {error}"),
            )
        });
        let encoder_result = (&mut self.encoder).await.map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                format!("showcase encoder task failed: {error}"),
            )
        });
        producer_result?;
        encoder_result?
    }
}

fn attach_terminal_reason(mut state: Value, terminal_reason: Option<Value>) -> Value {
    if let Some(terminal_reason) = terminal_reason {
        state
            .as_object_mut()
            .expect("showcase encoder state is an object")
            .insert("terminal_reason".into(), terminal_reason);
    }
    state
}

fn latch_terminal_reason(status: &LiveObservationStatus, terminal_reason: &Mutex<Option<Value>>) {
    let Some(reason) = status.terminal_reason() else {
        return;
    };
    let mut terminal_reason = lock_unpoisoned(terminal_reason);
    if terminal_reason.is_none() {
        *terminal_reason = Some(reason);
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Drop for ShowcaseRecorder {
    fn drop(&mut self) {
        let Some(stop_producer) = self.stop_producer.take() else {
            return;
        };
        let (acknowledged, _acknowledgement) = oneshot::channel();
        let _ = stop_producer.send(ShowcaseProducerStop { acknowledged });
    }
}

fn send_latest(
    status: &LiveObservationStatus,
    sender: &mpsc::Sender<ShowcaseProducerEvent>,
    last_forwarded_sequence: &mut Option<u64>,
) -> bool {
    let Some(frame) = status.latest() else {
        return false;
    };
    if last_forwarded_sequence.is_some_and(|sequence| frame.sequence() <= sequence) {
        return false;
    }
    if sender
        .try_send(ShowcaseProducerEvent::Frame(Arc::clone(&frame)))
        .is_err()
    {
        return false;
    }
    *last_forwarded_sequence = Some(frame.sequence());
    true
}

async fn send_latest_guaranteed(
    status: &LiveObservationStatus,
    sender: &mpsc::Sender<ShowcaseProducerEvent>,
    last_forwarded_sequence: &mut Option<u64>,
) -> GuaranteedFrameSend {
    let Some(frame) = status.latest() else {
        return GuaranteedFrameSend::NoNewFrame;
    };
    if last_forwarded_sequence.is_some_and(|sequence| frame.sequence() <= sequence) {
        return GuaranteedFrameSend::NoNewFrame;
    }
    let sequence = frame.sequence();
    if sender
        .send(ShowcaseProducerEvent::Frame(frame))
        .await
        .is_err()
    {
        return GuaranteedFrameSend::ReceiverClosed;
    }
    *last_forwarded_sequence = Some(sequence);
    GuaranteedFrameSend::Forwarded
}

async fn send_resume_frame_guaranteed(
    status: &LiveObservationStatus,
    sender: &mpsc::Sender<ShowcaseProducerEvent>,
    last_forwarded_sequence: &mut Option<u64>,
) -> GuaranteedFrameSend {
    let Some(frame) = status.latest() else {
        return GuaranteedFrameSend::NoNewFrame;
    };
    if last_forwarded_sequence.is_some_and(|sequence| frame.sequence() <= sequence) {
        return GuaranteedFrameSend::NoNewFrame;
    }
    let sequence = frame.sequence();
    let (acknowledged, acknowledgement) = oneshot::channel();
    if sender
        .send(ShowcaseProducerEvent::ResumedFrame(frame, acknowledged))
        .await
        .is_err()
    {
        return GuaranteedFrameSend::ReceiverClosed;
    }
    if !matches!(acknowledgement.await, Ok(Ok(()))) {
        return GuaranteedFrameSend::ReceiverClosed;
    }
    *last_forwarded_sequence = Some(sequence);
    GuaranteedFrameSend::Forwarded
}

async fn send_pause_boundary(sender: &mpsc::Sender<ShowcaseProducerEvent>) -> bool {
    let (acknowledged, acknowledgement) = oneshot::channel();
    if sender
        .send(ShowcaseProducerEvent::Paused(acknowledged))
        .await
        .is_err()
    {
        return false;
    }
    matches!(acknowledgement.await, Ok(Ok(())))
}

fn encode_frames_with_progress(
    mut frames: mpsc::Receiver<ShowcaseProducerEvent>,
    path: &Path,
    fps: u32,
    ready: oneshot::Sender<ComputerUseResult<()>>,
    progress: &Mutex<ShowcaseProgress>,
) -> ComputerUseResult<Value> {
    let FirstShowcaseFrame {
        frame: first,
        resumed_acknowledgement: first_frame_acknowledgement,
    } = receive_first_frame(&mut frames)?;
    let (source_width, source_height) = first.dimensions();
    let (width, height) = fit_dimensions(source_width, source_height);
    let video_format = VideoFormat { width, height, fps };
    let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(8_000_000))
        .max_frame_rate(FrameRate::from_hz(fps as f32))
        .usage_type(UsageType::ScreenContentRealTime)
        .intra_frame_period(IntraFramePeriod::from_num_frames(fps.saturating_mul(2)))
        .adaptive_quantization(false)
        .background_detection(false)
        .skip_frames(true);
    let mut encoder =
        Encoder::with_api_config(OpenH264API::from_source(), config).map_err(capture_error)?;
    let first_bgra = resize_bgra(first.bgra(), source_width, source_height, width, height);
    let first_stream = encoder
        .encode(&YUVBuffer::from_rgb_source(BgraSliceU8::new(
            &first_bgra,
            (width as usize, height as usize),
        )))
        .map_err(capture_error)?;
    let nominal_duration = (1000 / fps).max(1);
    let mut active_segment = Some({
        let mut current_progress = lock_unpoisoned(progress);
        begin_segment(
            &first_stream,
            path,
            0,
            0,
            video_format,
            nominal_duration,
            &mut current_progress,
        )?
    });
    if let Some(acknowledged) = first_frame_acknowledgement {
        let _ = acknowledged.send(Ok(()));
    }
    let _ = ready.send(Ok(()));

    let mut frame_count = 1_u64;
    let mut next_segment_index = 1_u32;
    let mut capture_anchor = first.captured_at();
    let mut media_anchor_ms = 0_u64;
    let mut media_duration_ms = 0_u64;
    while let Some(event) = frames.blocking_recv() {
        match event {
            ShowcaseProducerEvent::Paused(acknowledged) => {
                // Only an explicit producer pause removes wall-clock time.
                // Real long frame intervals without this event retain their
                // existing timestamps and five-minute roll behavior.
                let result = {
                    let mut current_progress = lock_unpoisoned(progress);
                    active_segment
                        .take()
                        .map_or(Ok(media_duration_ms), |segment| {
                            let tail_duration = pause_tail_duration(&segment, nominal_duration);
                            finalize_active_segment(
                                segment,
                                tail_duration,
                                video_format,
                                &mut current_progress,
                            )
                        })
                };
                match result {
                    Ok(duration_ms) => {
                        media_duration_ms = duration_ms;
                        let _ = acknowledged.send(Ok(()));
                    }
                    Err(error) => {
                        let _ = acknowledged.send(Err(error.clone()));
                        return Err(error);
                    }
                }
            }
            ShowcaseProducerEvent::ResumedFrame(frame, acknowledged) => {
                let result = (|| {
                    if active_segment.is_some() {
                        return Err(capture_error(
                            "showcase received a resume frame before finalizing its paused segment",
                        ));
                    }
                    capture_anchor = frame.captured_at();
                    media_anchor_ms = media_duration_ms;
                    active_segment = Some(begin_resumed_segment(
                        &mut encoder,
                        &frame,
                        SegmentStart {
                            path,
                            index: next_segment_index,
                            start_ms: media_duration_ms,
                            format: video_format,
                            nominal_duration,
                        },
                        progress,
                    )?);
                    next_segment_index = next_segment_index.saturating_add(1);
                    frame_count = frame_count.saturating_add(1);
                    Ok(())
                })();
                match result {
                    Ok(()) => {
                        let _ = acknowledged.send(Ok(()));
                    }
                    Err(error) => {
                        let _ = acknowledged.send(Err(error.clone()));
                        return Err(error);
                    }
                }
            }
            ShowcaseProducerEvent::Frame(frame) => {
                if active_segment.is_none() {
                    return Err(capture_error(
                        "showcase received an unacknowledged frame while paused",
                    ));
                }
                let (frame_width, frame_height) = frame.dimensions();
                let bgra = resize_bgra(frame.bgra(), frame_width, frame_height, width, height);
                let elapsed_ms = media_anchor_ms.saturating_add(
                    u64::try_from(
                        frame
                            .captured_at()
                            .saturating_duration_since(capture_anchor)
                            .as_millis(),
                    )
                    .unwrap_or(u64::MAX),
                );
                let segment_start_ms = active_segment
                    .as_ref()
                    .expect("active segment checked above")
                    .start_ms;
                if elapsed_ms.saturating_sub(segment_start_ms) >= SEGMENT_DURATION_MS {
                    let segment = active_segment.take().expect("active segment checked above");
                    let tail_duration = u32::try_from(
                        elapsed_ms
                            .saturating_sub(segment.start_ms)
                            .saturating_sub(segment.pending.start_time),
                    )
                    .unwrap_or(u32::MAX)
                    .max(1);
                    let mut current_progress = lock_unpoisoned(progress);
                    media_duration_ms = finalize_active_segment(
                        segment,
                        tail_duration,
                        video_format,
                        &mut current_progress,
                    )?;
                    encoder.force_intra_frame();
                    let stream = encode_bgra(&mut encoder, &bgra, width, height)?;
                    active_segment = Some(begin_segment(
                        &stream,
                        path,
                        next_segment_index,
                        elapsed_ms,
                        video_format,
                        nominal_duration,
                        &mut current_progress,
                    )?);
                    drop(current_progress);
                    next_segment_index = next_segment_index.saturating_add(1);
                    frame_count = frame_count.saturating_add(1);
                    continue;
                }

                let stream = encode_bgra(&mut encoder, &bgra, width, height)?;
                let segment = active_segment
                    .as_mut()
                    .expect("active segment checked above");
                let Some(mut current) =
                    pending_sample(&stream, elapsed_ms.saturating_sub(segment.start_ms))?
                else {
                    continue;
                };
                current.start_time = current
                    .start_time
                    .max(segment.pending.start_time.saturating_add(1));
                let sample_duration =
                    u32::try_from(current.start_time - segment.pending.start_time)
                        .unwrap_or(u32::MAX)
                        .max(1);
                let pending = std::mem::replace(&mut segment.pending, current);
                write_sample(&mut segment.writer, pending, sample_duration)?;
                segment.final_duration = sample_duration;
                segment.frame_count = segment.frame_count.saturating_add(1);
                frame_count = frame_count.saturating_add(1);
            }
        }
    }
    if let Some(segment) = active_segment {
        let tail_duration = segment.final_duration;
        let mut current_progress = lock_unpoisoned(progress);
        media_duration_ms =
            finalize_active_segment(segment, tail_duration, video_format, &mut current_progress)?;
    }
    let current_progress = lock_unpoisoned(progress);
    let segments = current_progress.segments.clone();
    drop(current_progress);
    let manifest_path = manifest_output_path(path);
    let state = json!({
        "active": false,
        "backend": "embedded-openh264",
        "path": path.to_string_lossy(),
        "manifest_path": manifest_path.to_string_lossy(),
        "width": width,
        "height": height,
        "fps": fps,
        "frames": frame_count,
        "duration_ms": media_duration_ms,
        "finalized": true,
        "segments": segments,
        "current_partial": Value::Null,
    });
    write_manifest(&manifest_path, &state)?;
    Ok(state)
}

fn receive_first_frame(
    frames: &mut mpsc::Receiver<ShowcaseProducerEvent>,
) -> ComputerUseResult<FirstShowcaseFrame> {
    while let Some(event) = frames.blocking_recv() {
        match event {
            ShowcaseProducerEvent::Frame(frame) => {
                return Ok(FirstShowcaseFrame {
                    frame,
                    resumed_acknowledgement: None,
                });
            }
            ShowcaseProducerEvent::ResumedFrame(frame, acknowledged) => {
                return Ok(FirstShowcaseFrame {
                    frame,
                    resumed_acknowledgement: Some(acknowledged),
                });
            }
            ShowcaseProducerEvent::Paused(acknowledged) => {
                let _ = acknowledged.send(Ok(()));
            }
        }
    }
    Err(capture_error("no showcase frame available"))
}

fn encode_bgra<'encoder>(
    encoder: &'encoder mut Encoder,
    bgra: &[u8],
    width: u32,
    height: u32,
) -> ComputerUseResult<openh264::encoder::EncodedBitStream<'encoder>> {
    encoder
        .encode(&YUVBuffer::from_rgb_source(BgraSliceU8::new(
            bgra,
            (width as usize, height as usize),
        )))
        .map_err(capture_error)
}

fn begin_segment(
    stream: &openh264::encoder::EncodedBitStream<'_>,
    path: &Path,
    index: u32,
    start_ms: u64,
    format: VideoFormat,
    nominal_duration: u32,
    progress: &mut ShowcaseProgress,
) -> ComputerUseResult<ActiveSegment> {
    let segment_path = segment_output_path(path, index);
    let partial_path = partial_segment_path(&segment_path);
    let (sps, pps) = parameter_sets(stream)?;
    let pending = pending_sample(stream, 0)?
        .ok_or_else(|| capture_error("OpenH264 omitted a segment's first video sample"))?;
    ensure_final_path_available(&segment_path)?;
    let writer = start_segment_writer(&partial_path, format.width, format.height, sps, pps)?;
    progress.current_partial = Some(partial_path.clone());
    Ok(ActiveSegment {
        index,
        path: segment_path,
        partial_path,
        start_ms,
        frame_count: 1,
        final_duration: nominal_duration,
        writer,
        pending,
    })
}

fn begin_resumed_segment(
    encoder: &mut Encoder,
    frame: &LiveObservationFrame,
    start: SegmentStart<'_>,
    progress: &Mutex<ShowcaseProgress>,
) -> ComputerUseResult<ActiveSegment> {
    let (source_width, source_height) = frame.dimensions();
    let bgra = resize_bgra(
        frame.bgra(),
        source_width,
        source_height,
        start.format.width,
        start.format.height,
    );
    encoder.force_intra_frame();
    let stream = encode_bgra(encoder, &bgra, start.format.width, start.format.height)?;
    let mut progress = lock_unpoisoned(progress);
    begin_segment(
        &stream,
        start.path,
        start.index,
        start.start_ms,
        start.format,
        start.nominal_duration,
        &mut progress,
    )
}

fn finalize_active_segment(
    mut segment: ActiveSegment,
    tail_duration: u32,
    format: VideoFormat,
    progress: &mut ShowcaseProgress,
) -> ComputerUseResult<u64> {
    let segment_duration_ms = segment
        .pending
        .start_time
        .saturating_add(u64::from(tail_duration));
    write_sample(&mut segment.writer, segment.pending, tail_duration)?;
    finalize_segment(segment.writer, &segment.partial_path, &segment.path)?;
    let end_ms = segment.start_ms.saturating_add(segment_duration_ms);
    progress.current_partial = None;
    progress.segments.push(segment_state(
        segment.index,
        &segment.path,
        segment.start_ms,
        segment_duration_ms,
        segment.frame_count,
        format,
    ));
    Ok(end_ms)
}

fn pause_tail_duration(segment: &ActiveSegment, nominal_duration: u32) -> u32 {
    // A nominal final sample must not push a pause-finalized segment beyond
    // the normal five-minute boundary.
    let remaining_ms = SEGMENT_DURATION_MS
        .saturating_sub(segment.pending.start_time)
        .max(1);
    nominal_duration.min(u32::try_from(remaining_ms).unwrap_or(u32::MAX))
}

fn start_segment_writer(
    path: &Path,
    width: u32,
    height: u32,
    sps: Vec<u8>,
    pps: Vec<u8>,
) -> ComputerUseResult<Mp4Writer<File>> {
    create_partial_file(path, |file| {
        let mut writer = Mp4Writer::write_start(
            file,
            &Mp4Config {
                major_brand: fourcc("isom")?,
                minor_version: 512,
                compatible_brands: vec![fourcc("isom")?, fourcc("iso2")?, fourcc("avc1")?],
                timescale: 1000,
            },
        )
        .map_err(capture_error)?;
        writer
            .add_track(
                &AvcConfig {
                    width: width as u16,
                    height: height as u16,
                    seq_param_set: sps,
                    pic_param_set: pps,
                }
                .into(),
            )
            .map_err(capture_error)?;
        Ok(writer)
    })
}

fn create_partial_file<T>(
    path: &Path,
    initialize: impl FnOnce(File) -> ComputerUseResult<T>,
) -> ComputerUseResult<T> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(capture_error)?;
    }
    let file = File::options()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(capture_error)?;
    match initialize(file) {
        Ok(initialized) => Ok(initialized),
        Err(error) => match std::fs::remove_file(path) {
            Ok(()) => Err(error),
            Err(cleanup_error) if cleanup_error.kind() == std::io::ErrorKind::NotFound => {
                Err(error)
            }
            Err(cleanup_error) => Err(capture_error(format!(
                "{}; failed to remove incomplete partial {}: {cleanup_error}",
                error.message,
                path.display()
            ))),
        },
    }
}

fn finalize_segment(
    mut writer: Mp4Writer<File>,
    partial_path: &Path,
    final_path: &Path,
) -> ComputerUseResult<()> {
    writer.write_end().map_err(capture_error)?;
    let file = writer.into_writer();
    file.sync_all().map_err(capture_error)?;
    drop(file);
    std::fs::rename(partial_path, final_path).map_err(capture_error)
}

fn ensure_final_path_available(path: &Path) -> ComputerUseResult<()> {
    if path.exists() {
        return Err(capture_error(format!(
            "showcase segment already exists: {}",
            path.display()
        )));
    }
    Ok(())
}

fn segment_output_path(path: &Path, index: u32) -> PathBuf {
    if index == 0 {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("showcase");
    path.with_file_name(format!("{stem}-{:04}.mp4", index + 1))
}

fn partial_segment_path(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("showcase");
    path.with_file_name(format!("{stem}.partial.mp4"))
}

fn manifest_output_path(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("showcase");
    path.with_file_name(format!("{stem}.manifest.json"))
}

fn write_manifest(path: &Path, manifest: &Value) -> ComputerUseResult<()> {
    ensure_final_path_available(path)?;
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("showcase.manifest");
    let partial_path = path.with_file_name(format!("{stem}.partial.json"));
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(&partial_path)
        .map_err(capture_error)?;
    serde_json::to_writer_pretty(&mut file, manifest).map_err(capture_error)?;
    file.write_all(b"\n").map_err(capture_error)?;
    file.sync_all().map_err(capture_error)?;
    drop(file);
    std::fs::rename(partial_path, path).map_err(capture_error)
}

fn segment_state(
    index: u32,
    path: &Path,
    start_ms: u64,
    duration_ms: u64,
    frames: u64,
    format: VideoFormat,
) -> ShowcaseSegment {
    ShowcaseSegment {
        index,
        path: path.to_path_buf(),
        start_ms,
        duration_ms,
        frames,
        width: format.width,
        height: format.height,
        fps: format.fps,
        finalized: true,
    }
}

fn pending_sample(
    stream: &openh264::encoder::EncodedBitStream<'_>,
    start_time: u64,
) -> ComputerUseResult<Option<PendingSample>> {
    let bytes = avcc_sample(stream)?;
    Ok((!bytes.is_empty()).then(|| PendingSample {
        start_time,
        is_sync: matches!(stream.frame_type(), FrameType::IDR | FrameType::I),
        bytes,
    }))
}

fn write_sample(
    writer: &mut Mp4Writer<File>,
    sample: PendingSample,
    duration: u32,
) -> ComputerUseResult<()> {
    writer
        .write_sample(
            1,
            &Mp4Sample {
                start_time: sample.start_time,
                duration,
                rendering_offset: 0,
                is_sync: sample.is_sync,
                bytes: sample.bytes.into(),
            },
        )
        .map_err(capture_error)
}

fn parameter_sets(
    stream: &openh264::encoder::EncodedBitStream<'_>,
) -> ComputerUseResult<(Vec<u8>, Vec<u8>)> {
    let mut sps = None;
    let mut pps = None;
    visit_nals(stream, |nal| match nal.first().map(|byte| byte & 0x1f) {
        Some(7) => sps = Some(nal.to_vec()),
        Some(8) => pps = Some(nal.to_vec()),
        _ => {}
    });
    Ok((
        sps.ok_or_else(|| capture_error("OpenH264 omitted SPS"))?,
        pps.ok_or_else(|| capture_error("OpenH264 omitted PPS"))?,
    ))
}

fn avcc_sample(stream: &openh264::encoder::EncodedBitStream<'_>) -> ComputerUseResult<Vec<u8>> {
    let mut sample = Vec::new();
    let mut overflow = false;
    visit_nals(stream, |nal| {
        if !matches!(nal.first().map(|byte| byte & 0x1f), Some(7 | 8)) {
            if let Ok(length) = u32::try_from(nal.len()) {
                sample.extend_from_slice(&length.to_be_bytes());
                sample.extend_from_slice(nal);
            } else {
                overflow = true;
            }
        }
    });
    if overflow {
        return Err(capture_error("H.264 NAL exceeds the MP4 sample limit"));
    }
    Ok(sample)
}

fn visit_nals(stream: &openh264::encoder::EncodedBitStream<'_>, mut visitor: impl FnMut(&[u8])) {
    for layer_index in 0..stream.num_layers() {
        let Some(layer) = stream.layer(layer_index) else {
            continue;
        };
        for nal_index in 0..layer.nal_count() {
            if let Some(nal) = layer.nal_unit(nal_index).and_then(strip_start_code) {
                visitor(nal);
            }
        }
    }
}

fn strip_start_code(nal: &[u8]) -> Option<&[u8]> {
    if nal.starts_with(&[0, 0, 0, 1]) {
        Some(&nal[4..])
    } else if nal.starts_with(&[0, 0, 1]) {
        Some(&nal[3..])
    } else {
        None
    }
}

fn fit_dimensions(width: u32, height: u32) -> (u32, u32) {
    fit_dimensions_with_bounds(width, height, MAX_WIDTH, MAX_HEIGHT)
}

pub(crate) fn fit_dimensions_with_bounds(
    width: u32,
    height: u32,
    max_width: u32,
    max_height: u32,
) -> (u32, u32) {
    let scale = (max_width as f64 / f64::from(width))
        .min(max_height as f64 / f64::from(height))
        .min(1.0);
    let width = ((f64::from(width) * scale) as u32).max(2) & !1;
    let height = ((f64::from(height) * scale) as u32).max(2) & !1;
    (width, height)
}

pub(crate) fn resize_bgra(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    width: u32,
    height: u32,
) -> Vec<u8> {
    if (source_width, source_height) == (width, height) {
        return source.to_vec();
    }
    let mut output = vec![0; width as usize * height as usize * 4];
    for y in 0..height {
        let source_y = y.saturating_mul(source_height) / height;
        for x in 0..width {
            let source_x = x.saturating_mul(source_width) / width;
            let source_index = (source_y as usize * source_width as usize + source_x as usize) * 4;
            let output_index = (y as usize * width as usize + x as usize) * 4;
            output[output_index..output_index + 4]
                .copy_from_slice(&source[source_index..source_index + 4]);
        }
    }
    output
}

fn fourcc(value: &str) -> ComputerUseResult<FourCC> {
    value.parse().map_err(capture_error)
}

fn capture_error(error: impl std::fmt::Display) -> ComputerUseError {
    ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
}

#[cfg(test)]
mod tests;
