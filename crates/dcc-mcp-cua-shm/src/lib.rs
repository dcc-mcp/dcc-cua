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

/// Owns the named region until the consumer has opened it or the session ends.
pub struct SharedImage {
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
            memory,
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

    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.memory.size() >= HEADER_SIZE + self.descriptor.length
    }
}

fn write_u64(target: &mut [u8], value: u64) {
    target.copy_from_slice(&value.to_ne_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_uses_cua_shared_memory_shape() {
        let image = SharedImage::from_bytes(b"png", "image/png").unwrap();
        let descriptor = image.descriptor();
        assert!(descriptor.name.starts_with("cua_"));
        assert_eq!(descriptor.length, 3);
        assert_eq!(descriptor.mime_type, "image/png");
        assert!(image.is_alive());

        let opened = ipckit::SharedMemory::open(&descriptor.name).unwrap();
        let header = opened.read(0, HEADER_SIZE).unwrap();
        assert_eq!(
            u64::from_ne_bytes(header[0..8].try_into().unwrap()),
            HEADER_MAGIC
        );
        assert_eq!(u64::from_ne_bytes(header[8..16].try_into().unwrap()), 3);
        assert_eq!(&opened.read(HEADER_SIZE, 3).unwrap(), b"png");
    }

    #[test]
    fn image_size_is_bounded() {
        assert!(matches!(
            SharedImage::from_bytes(&vec![0; MAX_IMAGE_BYTES + 1], "image/png"),
            Err(SharedImageError::TooLarge)
        ));
    }
}
