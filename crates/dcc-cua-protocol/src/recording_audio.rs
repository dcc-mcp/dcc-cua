//! Independent playback-audio ceiling. This is not a capture backend or a video grant.
use serde::{Deserialize, Serialize};
pub mod worker;

pub const MIN_PROCESS_LOOPBACK_WINDOWS_BUILD: u32 = 20_348;
pub const MAX_PROCESS_AUDIO_DURATION_MS: u64 = 300_000;
pub const MAX_PROCESS_AUDIO_OUTPUT_BYTES: u64 = 256 * 1024 * 1024;

/// Callers can request a mode, never a PID, device, microphone or desktop mix.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingAudioMode {
    #[default]
    None,
    TargetProcessTree,
}

impl RecordingAudioMode {
    pub fn is_none(&self) -> bool {
        *self == Self::None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessAudioTarget {
    pub process_id: u32,
    pub process_creation_time_100ns: u64,
    pub window_handle: u64,
    pub window_thread_id: u32,
    pub window_class_hash: u64,
}

/// Issuer-owned immutable data, carried only inside the trusted task lease.
/// Deserializing this structure alone never establishes user authorization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessAudioGrant {
    pub target: ProcessAudioTarget,
    pub max_duration_ms: u64,
    pub max_output_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProcessAudioFailure {
    #[error("process audio requires an independent trusted task grant")]
    MissingGrant,
    #[error("process audio exact native target is invalid or changed")]
    InvalidTarget,
    #[error("process audio requires Windows build 20348 or later")]
    UnsupportedPlatform,
    #[error("process audio duration or output ceiling is invalid")]
    InvalidBudget,
    #[error("process audio duration or output ceiling was exhausted")]
    BudgetExhausted,
}

impl ProcessAudioGrant {
    pub fn validate(&self) -> Result<(), ProcessAudioFailure> {
        if self.target.process_id == 0
            || self.target.process_creation_time_100ns == 0
            || self.target.window_handle == 0
            || self.target.window_thread_id == 0
            || self.target.window_class_hash == 0
        {
            return Err(ProcessAudioFailure::InvalidTarget);
        }
        if self.max_duration_ms == 0
            || self.max_duration_ms > MAX_PROCESS_AUDIO_DURATION_MS
            || self.max_output_bytes == 0
            || self.max_output_bytes > MAX_PROCESS_AUDIO_OUTPUT_BYTES
        {
            return Err(ProcessAudioFailure::InvalidBudget);
        }
        Ok(())
    }

    /// The actual target is read from the authorized video window, not caller params.
    pub fn validate_start(
        mode: RecordingAudioMode,
        grant: Option<&Self>,
        observed_target: ProcessAudioTarget,
        windows_build: Option<u32>,
    ) -> Result<Option<&Self>, ProcessAudioFailure> {
        if mode == RecordingAudioMode::None {
            return Ok(None);
        }
        let grant = grant.ok_or(ProcessAudioFailure::MissingGrant)?;
        grant.validate()?;
        if grant.target != observed_target {
            return Err(ProcessAudioFailure::InvalidTarget);
        }
        if windows_build.is_none_or(|build| build < MIN_PROCESS_LOOPBACK_WINDOWS_BUILD) {
            return Err(ProcessAudioFailure::UnsupportedPlatform);
        }
        Ok(Some(grant))
    }
}

#[cfg(test)]
mod tests;
