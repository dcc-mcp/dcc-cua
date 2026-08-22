//! Small, bounded, cross-process image handoff.
//!
//! The header is owned by this project and versioned independently from the
//! host control protocol.

use std::time::{SystemTime, UNIX_EPOCH};

use ipckit::SharedMemory;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const HEADER_SIZE: usize = 48;
const HEADER_MAGIC: u64 = 0x4355_4100_5348_4D01;
const SEGMENT_PREFIX: &str = "cua_";
const TTL_SECS: u64 = 60;

#[derive(Debug, Error)]
pub enum SharedImageError {
    #[error("shared image is empty")]
    Empty,
    #[error("shared image is larger than {MAX_IMAGE_BYTES} bytes")]
    TooLarge,
    #[error("shared image descriptor is invalid: {0}")]
    Invalid(String),
    #[error("shared image has expired")]
    Expired,
    #[error("shared memory failed: {0}")]
    Ipc(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedImageDescriptor {
    pub name: String,
    pub id: String,
    pub length: usize,
    pub mime_type: String,
}

/// Owns the named region until its response handoff is replaced or the session ends.
pub struct SharedImage {
    _memory: SharedMemory,
    descriptor: SharedImageDescriptor,
}

/// Opens a Host-owned shared image for the duration of one response.
pub struct SharedImageReader {
    memory: SharedMemory,
    descriptor: SharedImageDescriptor,
}

impl SharedImage {
    pub fn from_bytes(
        bytes: &[u8],
        mime_type: impl Into<String>,
    ) -> Result<Self, SharedImageError> {
        if bytes.is_empty() {
            return Err(SharedImageError::Empty);
        }
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(SharedImageError::TooLarge);
        }

        let id = Uuid::new_v4().simple().to_string()[..16].to_owned();
        let name = format!("{SEGMENT_PREFIX}{id}");
        let mut memory = SharedMemory::create(&name, HEADER_SIZE + bytes.len())
            .map_err(|error| SharedImageError::Ipc(error.to_string()))?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut header = [0_u8; HEADER_SIZE];
        write_u64(&mut header[0..8], HEADER_MAGIC);
        write_u64(&mut header[8..16], bytes.len() as u64);
        write_u64(&mut header[16..24], bytes.len() as u64);
        write_u64(&mut header[24..32], now);
        write_u64(&mut header[32..40], TTL_SECS);
        memory
            .write(0, &header)
            .and_then(|_| memory.write(HEADER_SIZE, bytes))
            .map_err(|error| SharedImageError::Ipc(error.to_string()))?;

        Ok(Self {
            _memory: memory,
            descriptor: SharedImageDescriptor {
                name,
                id,
                length: bytes.len(),
                mime_type: mime_type.into(),
            },
        })
    }

    #[must_use]
    pub fn descriptor(&self) -> &SharedImageDescriptor {
        &self.descriptor
    }
}

impl SharedImageReader {
    pub fn open(descriptor: SharedImageDescriptor) -> Result<Self, SharedImageError> {
        if descriptor.name.is_empty() || descriptor.length == 0 {
            return Err(SharedImageError::Invalid(
                "name and length are required".into(),
            ));
        }
        if descriptor.length > MAX_IMAGE_BYTES {
            return Err(SharedImageError::TooLarge);
        }
        let memory = SharedMemory::open(&descriptor.name)
            .map_err(|error| SharedImageError::Ipc(error.to_string()))?;
        if memory.size() < HEADER_SIZE + descriptor.length {
            return Err(SharedImageError::Invalid(
                "shared memory region is smaller than its descriptor".into(),
            ));
        }
        let header = memory
            .read(0, HEADER_SIZE)
            .map_err(|error| SharedImageError::Ipc(error.to_string()))?;
        if read_u64(&header[0..8]) != HEADER_MAGIC
            || read_u64(&header[8..16]) != descriptor.length as u64
            || read_u64(&header[16..24]) != descriptor.length as u64
        {
            return Err(SharedImageError::Invalid(
                "shared memory header does not match its descriptor".into(),
            ));
        }
        let created_at = read_u64(&header[24..32]);
        let ttl = read_u64(&header[32..40]);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if created_at > now || now - created_at > ttl {
            return Err(SharedImageError::Expired);
        }
        Ok(Self { memory, descriptor })
    }

    #[must_use]
    pub fn descriptor(&self) -> &SharedImageDescriptor {
        &self.descriptor
    }

    pub fn read(&self) -> Result<Vec<u8>, SharedImageError> {
        self.memory
            .read(HEADER_SIZE, self.descriptor.length)
            .map_err(|error| SharedImageError::Ipc(error.to_string()))
    }
}

fn write_u64(target: &mut [u8], value: u64) {
    target.copy_from_slice(&value.to_ne_bytes());
}

fn read_u64(source: &[u8]) -> u64 {
    u64::from_ne_bytes(
        source
            .try_into()
            .expect("shared image header field is 8 bytes"),
    )
}

#[cfg(test)]
mod tests;
