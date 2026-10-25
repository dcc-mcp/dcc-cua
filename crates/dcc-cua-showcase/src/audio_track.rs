//! AAC track construction for the existing MP4 writer; never opens a capture source.
use std::io::{self, Seek, SeekFrom, Write};

use dcc_cua_protocol::recording_audio::ProcessAudioGrant;
use mp4::{AacConfig, Mp4Sample, Mp4Writer, TrackConfig};
use rusty_aac::{AacEncoder, AacEncoderConfig};

use crate::process_audio::{
    AudioLifecycleError, MAX_PROCESS_AUDIO_PACKET_SAMPLES, PROCESS_AUDIO_CHANNELS,
    PROCESS_AUDIO_SAMPLE_RATE,
};

/// Bounds the complete existing recording, including video, audio and MP4 headers.
/// Seeking back to patch a header does not consume the same byte budget twice.
pub struct BoundedRecordingOutput<W> {
    inner: W,
    limit: u64,
    position: u64,
    length: u64,
}

impl<W: Write + Seek> BoundedRecordingOutput<W> {
    pub fn new(mut inner: W, limit: u64) -> io::Result<Self> {
        let position = inner.stream_position()?;
        let length = inner.seek(SeekFrom::End(0))?;
        inner.seek(SeekFrom::Start(position))?;
        if limit == 0 || length > limit || position > limit {
            return Err(io::Error::other("recording output ceiling invalid"));
        }
        Ok(Self {
            inner,
            limit,
            position,
            length,
        })
    }
    pub fn into_inner(self) -> W {
        self.inner
    }
    pub const fn length(&self) -> u64 {
        self.length
    }
}

impl<W: Write + Seek> Write for BoundedRecordingOutput<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let end = self
            .position
            .checked_add(bytes.len() as u64)
            .filter(|end| *end <= self.limit)
            .ok_or_else(|| io::Error::other("recording output ceiling exhausted"))?;
        let written = self.inner.write(bytes)?;
        self.position = end - (bytes.len() - written) as u64;
        self.length = self.length.max(self.position);
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write + Seek> Seek for BoundedRecordingOutput<W> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::Current(n) => i128::from(self.position) + i128::from(n),
            SeekFrom::End(n) => i128::from(self.length) + i128::from(n),
        };
        if position < 0 || position > i128::from(self.limit) {
            return Err(io::Error::other("recording seek exceeds output ceiling"));
        }
        self.position = self.inner.seek(SeekFrom::Start(position as u64))?;
        Ok(self.position)
    }
}

pub struct AacTrackBuffer {
    grant: ProcessAudioGrant,
    samples: Vec<i16>,
    nonzero_samples: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AacTrackReceipt {
    pub track_id: u32,
    pub input_frames: u64,
    pub encoded_samples: u64,
    pub encoded_bytes: u64,
    pub nonzero_input_samples: u64,
    /// AAC-LC filterbank priming; native marker alignment must measure this.
    pub codec_priming_samples: u32,
}

impl AacTrackBuffer {
    pub fn new(grant: ProcessAudioGrant) -> Result<Self, AudioLifecycleError> {
        grant.validate()?;
        Ok(Self {
            grant,
            samples: Vec::new(),
            nonzero_samples: 0,
        })
    }

    /// Copy only bounded admitted PCM. Gaps remain silence on the shared video
    /// clock; caller identity/revocation admission belongs to ProcessAudioSession.
    pub fn push(&mut self, pts_us: u64, samples: &[i16]) -> Result<(), AudioLifecycleError> {
        if samples.is_empty()
            || samples.len() > MAX_PROCESS_AUDIO_PACKET_SAMPLES
            || !samples.len().is_multiple_of(PROCESS_AUDIO_CHANNELS)
        {
            return Err(AudioLifecycleError::Packet);
        }
        let start = u128::from(pts_us) * u128::from(PROCESS_AUDIO_SAMPLE_RATE) / 1_000_000;
        let current = self.samples.len() / PROCESS_AUDIO_CHANNELS;
        let start = usize::try_from(start).map_err(|_| AudioLifecycleError::Budget)?;
        // Microsecond timestamp quantization may differ by one PCM frame.
        if start.saturating_add(1) < current {
            return Err(AudioLifecycleError::Packet);
        }
        let start = start.max(current);
        let end = start
            .checked_add(samples.len() / PROCESS_AUDIO_CHANNELS)
            .ok_or(AudioLifecycleError::Budget)?;
        let max_frames = self.grant.max_duration_ms * u64::from(PROCESS_AUDIO_SAMPLE_RATE) / 1000;
        if end as u128 > u128::from(max_frames) {
            return Err(AudioLifecycleError::Budget);
        }
        let length = end
            .checked_mul(PROCESS_AUDIO_CHANNELS)
            .ok_or(AudioLifecycleError::Budget)?;
        self.samples
            .try_reserve_exact(length - self.samples.len())
            .map_err(|_| AudioLifecycleError::Budget)?;
        self.samples.resize(start * PROCESS_AUDIO_CHANNELS, 0);
        self.samples.extend_from_slice(samples);
        self.nonzero_samples += samples.iter().filter(|sample| **sample != 0).count() as u64;
        Ok(())
    }

    /// The caller owns the existing single video track (ID 1). Add AAC as track
    /// 2 before that writer's ordinary write_end; this starts no second mux/file.
    pub fn append_after_single_video_track<W: Write + Seek>(
        self,
        writer: &mut Mp4Writer<BoundedRecordingOutput<W>>,
    ) -> Result<AacTrackReceipt, AudioLifecycleError> {
        if self.samples.is_empty() {
            return Err(AudioLifecycleError::Packet);
        }
        let input_frames = (self.samples.len() / PROCESS_AUDIO_CHANNELS) as u64;
        let mut encoder = AacEncoder::new(AacEncoderConfig::default());
        for chunk in self.samples.chunks(MAX_PROCESS_AUDIO_PACKET_SAMPLES) {
            let floats: Vec<f32> = chunk
                .iter()
                .map(|value| f32::from(*value) / 32768.0)
                .collect();
            encoder
                .push_pcm(
                    &floats,
                    PROCESS_AUDIO_CHANNELS as u16,
                    PROCESS_AUDIO_SAMPLE_RATE,
                )
                .map_err(|_| AudioLifecycleError::Writer)?;
        }
        drop(self.samples);
        encoder.finish();
        let mut config: TrackConfig = AacConfig {
            bitrate: 128_000,
            ..Default::default()
        }
        .into();
        config.timescale = PROCESS_AUDIO_SAMPLE_RATE;
        writer
            .add_track(&config)
            .map_err(|_| AudioLifecycleError::Writer)?;
        let mut encoded_bytes = 0_u64;
        let mut encoded_samples = 0_u64;
        loop {
            let packet = match encoder.next_packet() {
                Ok(packet) => packet,
                Err(rusty_aac::Error::Eof) => break,
                Err(_) => return Err(AudioLifecycleError::Writer),
            };
            encoded_bytes = encoded_bytes
                .checked_add(packet.data.len() as u64)
                .filter(|n| *n <= self.grant.max_output_bytes)
                .ok_or(AudioLifecycleError::Budget)?;
            let start_time = u64::try_from(packet.pts).map_err(|_| AudioLifecycleError::Packet)?;
            encoded_samples = start_time
                .checked_add(u64::from(packet.duration))
                .ok_or(AudioLifecycleError::Packet)?;
            writer
                .write_sample(
                    2,
                    &Mp4Sample {
                        start_time,
                        duration: packet.duration,
                        rendering_offset: 0,
                        is_sync: true,
                        bytes: packet.data.into(),
                    },
                )
                .map_err(|_| AudioLifecycleError::Writer)?;
        }
        Ok(AacTrackReceipt {
            track_id: 2,
            input_frames,
            encoded_samples,
            encoded_bytes,
            nonzero_input_samples: self.nonzero_samples,
            codec_priming_samples: 1024,
        })
    }
}

#[cfg(test)]
mod tests;
