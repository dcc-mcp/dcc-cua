use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use mp4::{AvcConfig, FourCC, Mp4Config, Mp4Sample, Mp4Writer};
use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, UsageType,
};
use openh264::formats::{BgraSliceU8, YUVBuffer};
use serde_json::{Value, json};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

use crate::live_observation::{LiveObservationFrame, LiveObservationStatus};
use crate::{ComputerUseError, ComputerUseErrorCode, ComputerUseResult};

const MAX_WIDTH: u32 = 1600;
const MAX_HEIGHT: u32 = 900;

struct PendingSample {
    start_time: u64,
    is_sync: bool,
    bytes: Vec<u8>,
}

pub(crate) struct ShowcaseRecorder {
    path: PathBuf,
    producer: JoinHandle<()>,
    encoder: JoinHandle<ComputerUseResult<Value>>,
}

impl ShowcaseRecorder {
    pub(crate) async fn start(
        mut frames: watch::Receiver<LiveObservationStatus>,
        output_dir: &str,
        fps: u32,
    ) -> ComputerUseResult<Self> {
        let path = Path::new(output_dir).join("showcase.mp4");
        let (frame_sender, frame_receiver) = sync_channel(2);
        let (ready_sender, ready_receiver) = oneshot::channel();
        let encoder_path = path.clone();
        let encoder = tokio::task::spawn_blocking(move || {
            encode_frames(frame_receiver, &encoder_path, fps, ready_sender)
        });
        let producer = tokio::spawn(async move {
            send_latest(&frames, &frame_sender);
            while frames.changed().await.is_ok() {
                send_latest(&frames, &frame_sender);
            }
        });
        match ready_receiver.await {
            Ok(Ok(())) => Ok(Self {
                path,
                producer,
                encoder,
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
        json!({
            "active": !self.encoder.is_finished(),
            "backend": "embedded-openh264",
            "path": self.path.to_string_lossy(),
        })
    }

    pub(crate) async fn stop(mut self) -> ComputerUseResult<Value> {
        self.producer.abort();
        let _ = (&mut self.producer).await;
        (&mut self.encoder).await.map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                format!("showcase encoder task failed: {error}"),
            )
        })?
    }
}

impl Drop for ShowcaseRecorder {
    fn drop(&mut self) {
        self.producer.abort();
    }
}

fn send_latest(
    frames: &watch::Receiver<LiveObservationStatus>,
    sender: &SyncSender<std::sync::Arc<LiveObservationFrame>>,
) {
    if let Some(frame) = frames.borrow().latest() {
        let _ = sender.try_send(frame);
    }
}

fn encode_frames(
    frames: Receiver<std::sync::Arc<LiveObservationFrame>>,
    path: &Path,
    fps: u32,
    ready: oneshot::Sender<ComputerUseResult<()>>,
) -> ComputerUseResult<Value> {
    let first = frames
        .recv()
        .map_err(|_| capture_error("no showcase frame available"))?;
    let (source_width, source_height) = first.dimensions();
    let (width, height) = fit_dimensions(source_width, source_height);
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
    let (sps, pps) = parameter_sets(&first_stream)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(capture_error)?;
    }
    let file = File::create(path).map_err(capture_error)?;
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
    let mut pending = pending_sample(&first_stream, 0)?
        .ok_or_else(|| capture_error("OpenH264 omitted the first video sample"))?;
    let _ = ready.send(Ok(()));

    let mut frame_count = 1_u64;
    let first_captured_at = first.captured_at();
    let nominal_duration = (1000 / fps).max(1);
    let mut final_duration = nominal_duration;
    for frame in frames {
        let (frame_width, frame_height) = frame.dimensions();
        let bgra = resize_bgra(frame.bgra(), frame_width, frame_height, width, height);
        let stream = encoder
            .encode(&YUVBuffer::from_rgb_source(BgraSliceU8::new(
                &bgra,
                (width as usize, height as usize),
            )))
            .map_err(capture_error)?;
        let elapsed_ms = u64::try_from(
            frame
                .captured_at()
                .saturating_duration_since(first_captured_at)
                .as_millis(),
        )
        .unwrap_or(u64::MAX);
        let Some(mut current) = pending_sample(&stream, elapsed_ms)? else {
            continue;
        };
        current.start_time = current.start_time.max(pending.start_time.saturating_add(1));
        final_duration = u32::try_from(current.start_time - pending.start_time)
            .unwrap_or(u32::MAX)
            .max(1);
        write_sample(&mut writer, pending, final_duration)?;
        pending = current;
        frame_count = frame_count.saturating_add(1);
    }
    let duration_ms = pending.start_time.saturating_add(u64::from(final_duration));
    write_sample(&mut writer, pending, final_duration)?;
    writer.write_end().map_err(capture_error)?;
    Ok(json!({
        "active": false,
        "backend": "embedded-openh264",
        "path": path.to_string_lossy(),
        "width": width,
        "height": height,
        "fps": fps,
        "frames": frame_count,
        "duration_ms": duration_ms,
        "finalized": true,
    }))
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
