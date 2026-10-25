//! Inversion port: the platform owns capture; the existing recorder owns muxing.
use crate::{
    RecordingAuthorization, audio_track::AacTrackBuffer, process_audio::AudioLifecycleError,
};
use dcc_cua_protocol::recording_audio::ProcessAudioGrant;
use std::{sync::Arc, time::Instant};

pub trait RecorderAudioOwner: Send {
    /// Stop/reap the source before returning its bounded PCM buffer.
    fn finish(&mut self) -> Result<AacTrackBuffer, AudioLifecycleError>;
    fn evidence(&self) -> serde_json::Value;
}

pub trait RecorderAudioFactory: Send {
    fn grant(&self) -> &ProcessAudioGrant;
    fn start(
        self: Box<Self>,
        epoch: Instant,
        authorization: Arc<RecordingAuthorization>,
        ended: Box<dyn Fn() + Send>,
    ) -> Result<Box<dyn RecorderAudioOwner>, AudioLifecycleError>;
}

#[cfg(test)]
mod tests;
