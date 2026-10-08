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
    for pixel in bgra.chunks_exact(4) {
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
mod tests {
    use super::*;

    #[test]
    fn diagnostics_describe_raw_bytes_and_decoded_png_without_alpha_normalization() {
        let bgra = [3, 2, 1, 141, 83, 42, 7, 0, 17, 122, 138, 255];
        let encoded = encode_exact_frame(&bgra, 3, 1, true, Duration::from_micros(17)).unwrap();
        let diagnostics = encoded.diagnostics.unwrap();
        let decoder = png::Decoder::new(encoded.data.as_slice());
        let mut reader = decoder.read_info().unwrap();
        let mut rgba = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut rgba).unwrap();
        rgba.truncate(info.buffer_size());
        assert_eq!(rgba, [1, 2, 3, 141, 7, 42, 83, 0, 138, 122, 17, 255]);
        assert_eq!(
            diagnostics.raw_bgra_sha256,
            format!("{:x}", Sha256::digest(bgra))
        );
        assert_eq!(
            diagnostics.canonical_rgba_sha256,
            format!("{:x}", Sha256::digest(&rgba))
        );
        assert_ne!(
            diagnostics.raw_bgra_sha256,
            diagnostics.canonical_rgba_sha256
        );
        assert_eq!(diagnostics.capture_to_raw_elapsed_micros, 17);
        let serialized = serde_json::to_value(&diagnostics).unwrap();
        assert_eq!(serialized["capture_to_raw_elapsed_micros"], 17);
        assert!(serialized.get("raw_readback_elapsed_micros").is_none());
        for (channel, histogram) in diagnostics.bgra_channel_histograms.iter().enumerate() {
            assert_eq!(histogram.len(), 256);
            assert_eq!(histogram.iter().sum::<u64>(), 3);
            for pixel in bgra.chunks_exact(4) {
                assert_eq!(histogram[usize::from(pixel[channel])], 1);
            }
        }
    }

    #[test]
    fn opt_in_does_not_change_png_bytes_and_default_omits_summaries() {
        let bgra = [83, 42, 7, 255, 17, 122, 138, 255];
        let default = encode_exact_frame(&bgra, 2, 1, false, Duration::ZERO).unwrap();
        let diagnostic = encode_exact_frame(&bgra, 2, 1, true, Duration::ZERO).unwrap();
        assert!(default.diagnostics.is_none());
        assert_eq!(default.data, diagnostic.data);
    }

    #[test]
    fn incomplete_or_empty_frames_fail_without_a_diagnostic_summary() {
        for (data, width, height) in [
            (&[1_u8, 2, 3][..], 1, 1),
            (&[][..], 0, 1),
            (&[][..], u32::MAX, u32::MAX),
        ] {
            assert!(encode_exact_frame(data, width, height, true, Duration::ZERO).is_err());
        }
    }
}
