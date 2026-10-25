//! Bounded private pipes for one owned audio worker, not a public capture API.
use super::{ProcessAudioGrant, ProcessAudioTarget};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{self, Read, Write};

pub const AUDIO_WORKER_PROTOCOL: u32 = 1;
pub const MAX_AUDIO_WORKER_MESSAGE: usize = 64 * 1024;
pub const MAX_AUDIO_WORKER_SAMPLES: usize = 4800;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioWorkerInitialization {
    pub protocol: u32,
    pub grant: ProcessAudioGrant,
    pub deadline_qpc_100ns: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AudioWorkerCommand {
    Initialize { request: AudioWorkerInitialization },
    Start {},
    Stop {},
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AudioWorkerEvent {
    Prepared {
        target: ProcessAudioTarget,
        worker_process_id: u32,
        worker_creation_time_100ns: u64,
        windows_build: u32,
    },
    Started {},
    Packet {
        qpc_100ns: u64,
        device_position: u64,
        flags: u32,
        samples: Vec<i16>,
    },
    Stopped {},
    Failed {
        code: AudioWorkerFailure,
        #[serde(default)]
        diagnostic: Option<AudioWorkerFailureDiagnostic>,
    },
}

/// Metadata only: no PCM, device selection, window title or executable path.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AudioWorkerFailureDiagnostic {
    pub stage: Option<AudioWorkerFailureStage>,
    pub hresult: Option<i32>,
    pub packet: Option<AudioWorkerPacketMetadata>,
    pub stream_stop_succeeded: Option<bool>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioWorkerFailureStage {
    GetNextPacketSize,
    GetBuffer,
    PacketValidation,
    ReleaseBuffer,
    StopStream,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AudioWorkerPacketMetadata {
    pub frames: u32,
    pub flags: u32,
    pub qpc_100ns: u64,
    pub device_position: u64,
    pub expected_device_position: Option<u64>,
    pub null_data: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioWorkerFailure {
    InvalidGrant,
    UnsupportedPlatform,
    IdentityUnavailable,
    IdentityChanged,
    ActivationFailed,
    StreamFailed,
    ContinuityLost,
    BudgetExpired,
    ProtocolFailed,
}

pub fn write_message<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    let data = serde_json::to_vec(value).map_err(io::Error::other)?;
    if data.is_empty() || data.len() > MAX_AUDIO_WORKER_MESSAGE {
        return Err(io::Error::other("audio worker frame exceeds ceiling"));
    }
    writer.write_all(&(data.len() as u32).to_le_bytes())?;
    writer.write_all(&data)?;
    writer.flush()
}

pub fn read_message<R: Read, T: DeserializeOwned>(reader: &mut R) -> io::Result<Option<T>> {
    let mut size = [0u8; 4];
    let first = reader.read(&mut size[..1])?;
    if first == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut size[1..])?;
    let size = u32::from_le_bytes(size) as usize;
    if size == 0 || size > MAX_AUDIO_WORKER_MESSAGE {
        return Err(io::Error::other("audio worker frame exceeds ceiling"));
    }
    let mut data = vec![0u8; size];
    reader.read_exact(&mut data)?;
    serde_json::from_slice(&data)
        .map(Some)
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests;
