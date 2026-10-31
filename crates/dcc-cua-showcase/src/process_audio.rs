//! Bounded process-audio admission using the existing recorder's revocation fence.
//! Ports do not start a screen recorder or choose a device/PID on a caller's behalf.
use std::sync::Arc;
use std::time::Instant;

use dcc_cua_protocol::recording_audio::{
    ProcessAudioFailure, ProcessAudioGrant, ProcessAudioTarget, RecordingAudioMode,
};

use crate::RecordingAuthorization;

pub const PROCESS_AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const PROCESS_AUDIO_CHANNELS: usize = 2;
pub const MAX_PROCESS_AUDIO_PACKET_SAMPLES: usize = 4_800;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AudioLifecycleError {
    #[error(transparent)]
    Policy(#[from] ProcessAudioFailure),
    #[error("process audio permission is missing, expired or revoked")]
    Authorization,
    #[error("process audio target or effective source changed")]
    Identity,
    #[error("process audio packet format, timestamp or size is invalid")]
    Packet,
    #[error("process audio duration or output ceiling exhausted")]
    Budget,
    #[error("process audio source failed or lost continuity")]
    Source,
    #[error("owned process audio cleanup has not been acknowledged")]
    CleanupUnknown,
    #[error("process audio writer failed")]
    Writer,
}

/// Describes an activated source before streaming. Other sources are refusals,
/// never alternative capture requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectiveAudioSource {
    ProcessTree(ProcessAudioTarget),
    Other,
}

pub trait ProcessAudioSource: Send {
    /// Activate/verify only; this must not start delivering samples.
    fn prepare(
        &mut self,
        target: ProcessAudioTarget,
    ) -> Result<EffectiveAudioSource, AudioLifecycleError>;
    fn start(&mut self) -> Result<(), AudioLifecycleError>;
    /// Must release prepared resources even if streaming never began.
    fn stop(&mut self) -> Result<(), AudioLifecycleError>;
}

/// Implemented by the existing recorder's audio-track/mux owner. The remaining
/// budget must be enforced before output writes, including container overhead.
pub trait ProcessAudioMux: Send {
    fn write_pcm(
        &mut self,
        pts_us: u64,
        samples: &[i16],
        remaining_output_bytes: u64,
    ) -> Result<u64, AudioLifecycleError>;
    /// Finalize the shared recording after capture has stopped; no capture retry.
    fn finalize(&mut self) -> Result<(), AudioLifecycleError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioStopReceipt {
    pub source_stopped: bool,
    pub mux_finalized: bool,
    pub cleanup_pending: bool,
    pub terminal_error: Option<AudioLifecycleError>,
    pub packets_accepted: u64,
    pub output_bytes: u64,
}

#[derive(Debug, thiserror::Error)]
#[error("process audio startup failed: {error}")]
pub struct AudioStartFailure {
    pub error: AudioLifecycleError,
    pub cleanup: AudioStopReceipt,
}

pub struct ProcessAudioSession<S: ProcessAudioSource, M: ProcessAudioMux> {
    source: S,
    mux: M,
    grant: ProcessAudioGrant,
    authorization: Arc<RecordingAuthorization>,
    clock_us: Arc<dyn Fn() -> u64 + Send + Sync>,
    last_clock_us: u64,
    last_packet_end_us: Option<u64>,
    output_bytes: u64,
    packets: u64,
    terminal: Option<AudioStopReceipt>,
}

impl<S: ProcessAudioSource, M: ProcessAudioMux> ProcessAudioSession<S, M> {
    pub fn start(
        source: S,
        mux: M,
        grant: ProcessAudioGrant,
        observed: ProcessAudioTarget,
        windows_build: Option<u32>,
        authorization: Arc<RecordingAuthorization>,
        media_epoch: Instant,
    ) -> Result<Self, AudioStartFailure> {
        Self::start_with_clock(
            source,
            mux,
            grant,
            observed,
            windows_build,
            authorization,
            Arc::new(move || {
                Instant::now()
                    .checked_duration_since(media_epoch)
                    .and_then(|elapsed| u64::try_from(elapsed.as_micros()).ok())
                    .unwrap_or(u64::MAX)
            }),
        )
    }

    fn start_with_clock(
        mut source: S,
        mut mux: M,
        grant: ProcessAudioGrant,
        observed: ProcessAudioTarget,
        windows_build: Option<u32>,
        authorization: Arc<RecordingAuthorization>,
        clock_us: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Result<Self, AudioStartFailure> {
        let validated = ProcessAudioGrant::validate_start(
            RecordingAudioMode::TargetProcessTree,
            Some(&grant),
            observed,
            windows_build,
        )
        .map(|_| ())
        .map_err(AudioLifecycleError::Policy);
        let prepared = validated.and_then(|()| {
            let admission = authorization
                .admit()
                .map_err(|_| AudioLifecycleError::Authorization)?;
            if clock_us() >= grant.max_duration_ms * 1000 {
                return Err(AudioLifecycleError::Budget);
            }
            if source.prepare(grant.target)? != EffectiveAudioSource::ProcessTree(grant.target) {
                return Err(AudioLifecycleError::Identity);
            }
            admission
                .validate_current()
                .map_err(|_| AudioLifecycleError::Authorization)?;
            if clock_us() >= grant.max_duration_ms * 1000 {
                return Err(AudioLifecycleError::Budget);
            }
            source.start()
        });
        if let Err(error) = prepared {
            authorization.revoke();
            let source_stopped = source.stop().is_ok();
            let mux_finalized = mux.finalize().is_ok();
            return Err(AudioStartFailure {
                error,
                cleanup: AudioStopReceipt {
                    source_stopped,
                    mux_finalized,
                    cleanup_pending: !source_stopped || !mux_finalized,
                    terminal_error: Some(error),
                    packets_accepted: 0,
                    output_bytes: 0,
                },
            });
        }
        Ok(Self {
            source,
            mux,
            grant,
            authorization,
            clock_us,
            last_clock_us: 0,
            last_packet_end_us: None,
            output_bytes: 0,
            packets: 0,
            terminal: None,
        })
    }

    /// The capture adapter supplies a fresh exact identity. Packet timestamps
    /// use the same monotonic origin as video; a gap is retained, never squeezed.
    pub fn ingest(
        &mut self,
        observed: ProcessAudioTarget,
        pts_us: u64,
        samples: &[i16],
    ) -> Result<(), AudioLifecycleError> {
        if self.terminal.is_some() {
            return Err(AudioLifecycleError::Source);
        }
        let result = self.admit_packet(observed, pts_us, samples);
        if let Err(error) = result {
            self.finish(Some(error));
        }
        result
    }

    fn admit_packet(
        &mut self,
        observed: ProcessAudioTarget,
        pts_us: u64,
        samples: &[i16],
    ) -> Result<(), AudioLifecycleError> {
        let admission = self
            .authorization
            .admit()
            .map_err(|_| AudioLifecycleError::Authorization)?;
        if observed != self.grant.target {
            return Err(AudioLifecycleError::Identity);
        }
        if samples.is_empty()
            || samples.len() > MAX_PROCESS_AUDIO_PACKET_SAMPLES
            || !samples.len().is_multiple_of(PROCESS_AUDIO_CHANNELS)
        {
            return Err(AudioLifecycleError::Packet);
        }
        let now = (self.clock_us)();
        let duration_us = u64::try_from(samples.len() / PROCESS_AUDIO_CHANNELS)
            .map_err(|_| AudioLifecycleError::Packet)?
            * 1_000_000
            / u64::from(PROCESS_AUDIO_SAMPLE_RATE);
        let end = pts_us
            .checked_add(duration_us)
            .ok_or(AudioLifecycleError::Packet)?;
        if now < self.last_clock_us
            || self
                .last_packet_end_us
                .is_some_and(|last| pts_us.saturating_add(21) < last)
        {
            return Err(AudioLifecycleError::Packet);
        }
        let ceiling = self.grant.max_duration_ms * 1000;
        if now >= ceiling || end > ceiling {
            return Err(AudioLifecycleError::Budget);
        }
        let remaining = self
            .grant
            .max_output_bytes
            .checked_sub(self.output_bytes)
            .ok_or(AudioLifecycleError::Budget)?;
        let written = self.mux.write_pcm(pts_us, samples, remaining)?;
        if written > remaining {
            return Err(AudioLifecycleError::Budget);
        }
        self.output_bytes += written;
        self.packets += 1;
        self.last_clock_us = now;
        self.last_packet_end_us = Some(end);
        drop(admission);
        Ok(())
    }

    /// Device loss/discontinuity/unknown delivery ends both owners once.
    pub fn source_failed(&mut self) -> AudioStopReceipt {
        self.finish(Some(AudioLifecycleError::Source))
    }

    pub fn stop(&mut self) -> AudioStopReceipt {
        self.finish(None)
    }

    fn finish(&mut self, error: Option<AudioLifecycleError>) -> AudioStopReceipt {
        if let Some(receipt) = &self.terminal {
            return receipt.clone();
        }
        self.authorization.revoke();
        let source_stopped = self.source.stop().is_ok();
        let mux_finalized = self.mux.finalize().is_ok();
        let receipt = AudioStopReceipt {
            source_stopped,
            mux_finalized,
            cleanup_pending: !source_stopped || !mux_finalized,
            terminal_error: error.or(if !source_stopped {
                Some(AudioLifecycleError::Source)
            } else if !mux_finalized {
                Some(AudioLifecycleError::Writer)
            } else {
                None
            }),
            packets_accepted: self.packets,
            output_bytes: self.output_bytes,
        };
        self.terminal = Some(receipt.clone());
        receipt
    }
}

impl<S: ProcessAudioSource, M: ProcessAudioMux> Drop for ProcessAudioSession<S, M> {
    fn drop(&mut self) {
        self.finish(None);
    }
}

#[cfg(test)]
mod tests;
