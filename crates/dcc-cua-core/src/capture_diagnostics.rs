//! Opt-in summaries of the exact bytes entering the unchanged PNG encoder.
//!
//! The fourth BI_RGB byte is reported as a raw high byte, not interpreted as
//! premultiplied alpha. This module neither acquires pixels nor changes them.
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

use crate::contracts::*;
use crate::policy::encode_bgra_to_png;

#[derive(Debug, Serialize)]
pub(crate) struct PixelBufferDiagnostics {
    pub width: u32,
    pub height: u32,
    pub byte_length: usize,
    pub raw_bgra_sha256: String,
    pub canonical_rgba_sha256: String,
    /// Exactly 256 bins per channel, in B/G/R/raw-high-byte order.
    pub bgra_channel_histograms: [Vec<u64>; 4],
    pub capture_to_raw_elapsed_micros: u64,
    pub summary_elapsed_micros: u64,
    pub png_encode_elapsed_micros: u64,
}

pub(crate) struct EncodedPixelFrame {
    pub data: Vec<u8>,
    pub diagnostics: Option<PixelBufferDiagnostics>,
}

fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn summarize_bgra(
    bgra: &[u8],
    width: u32,
    height: u32,
    capture_to_raw_elapsed: Duration,
) -> ComputerUseResult<PixelBufferDiagnostics> {
    let expected = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .and_then(|bytes| usize::try_from(bytes).ok());
    if width == 0 || height == 0 || expected != Some(bgra.len()) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "capture diagnostics require a complete, nonempty BGRA frame",
        ));
    }
    let started = Instant::now();
    let mut rgba_hash = Sha256::new();
    let mut histograms = std::array::from_fn(|_| vec![0_u64; 256]);
    for pixel in bgra.as_chunks::<4>().0 {
        rgba_hash.update([pixel[2], pixel[1], pixel[0], pixel[3]]);
        for (channel, value) in pixel.iter().enumerate() {
            histograms[channel][usize::from(*value)] += 1;
        }
    }
    Ok(PixelBufferDiagnostics {
        width,
        height,
        byte_length: bgra.len(),
        raw_bgra_sha256: format!("{:x}", Sha256::digest(bgra)),
        canonical_rgba_sha256: format!("{:x}", rgba_hash.finalize()),
        bgra_channel_histograms: histograms,
        capture_to_raw_elapsed_micros: micros(capture_to_raw_elapsed),
        summary_elapsed_micros: micros(started.elapsed()),
        png_encode_elapsed_micros: 0,
    })
}

pub(crate) fn encode_exact_frame(
    bgra: &[u8],
    width: u32,
    height: u32,
    diagnostics_enabled: bool,
    capture_to_raw_elapsed: Duration,
) -> ComputerUseResult<EncodedPixelFrame> {
    let mut diagnostics = diagnostics_enabled
        .then(|| summarize_bgra(bgra, width, height, capture_to_raw_elapsed))
        .transpose()?;
    let encode_started = Instant::now();
    let data = encode_bgra_to_png(bgra, width, height)?;
    if let Some(diagnostics) = &mut diagnostics {
        diagnostics.png_encode_elapsed_micros = micros(encode_started.elapsed());
    }
    Ok(EncodedPixelFrame { data, diagnostics })
}

#[cfg(test)]
mod tests;
