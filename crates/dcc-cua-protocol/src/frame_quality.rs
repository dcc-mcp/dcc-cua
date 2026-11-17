//! Content-free quality assessment shared by native and future host capture ports.
//!
//! This vocabulary does not advertise a capture backend or grant capture authority.
//! A nonuniform frame is only informative, never proof of the intended UI content.
use serde::{Deserialize, Serialize};

use crate::MAX_BINARY_FRAME_BYTES;

/// Exact uniformity is the initial blank threshold. Near-uniform images are not
/// rejected heuristically: applications may intentionally display sparse content.
pub const BLANK_RGB_STDDEV_MILLI: u32 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    WindowContent,
    VisibleDesktopCrop,
    RedactedDesktopCrop,
    InProcessQt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameStatistics {
    pub width: u32,
    pub height: u32,
    pub pixel_count: u64,
    /// Per-channel values in R/G/B order, scaled by 1000.
    pub mean_rgb_milli: [u32; 3],
    pub stddev_rgb_milli: [u32; 3],
    /// Shannon entropy per color channel, scaled by 1000 (0..8000).
    pub entropy_rgb_millibits: [u32; 3],
    pub uniform_rgb: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FrameAssessment {
    Informative {
        attempted_mode: CaptureMode,
        statistics: FrameStatistics,
    },
    BlankCapture {
        attempted_mode: CaptureMode,
        statistics: FrameStatistics,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FrameDataError {
    #[error("capture frame dimensions must be nonzero")]
    Empty,
    #[error("capture frame exceeds the binary transport ceiling")]
    TooLarge,
    #[error("capture frame must contain exactly width * height * 4 BGRA bytes")]
    InvalidLength,
}

/// Assess a complete, tightly packed BGRA frame without retaining its pixels.
/// Padding must be removed by the owning backend. The fourth byte is ignored:
/// BI_RGB high bytes are not necessarily alpha, and cannot make blank RGB valid.
pub fn assess_bgra(
    attempted_mode: CaptureMode,
    bgra: &[u8],
    width: u32,
    height: u32,
) -> Result<FrameAssessment, FrameDataError> {
    if width == 0 || height == 0 {
        return Err(FrameDataError::Empty);
    }
    let pixel_count = u64::from(width) * u64::from(height);
    let expected = pixel_count
        .checked_mul(4)
        .filter(|bytes| *bytes <= MAX_BINARY_FRAME_BYTES as u64)
        .ok_or(FrameDataError::TooLarge)?;
    if usize::try_from(expected).ok() != Some(bgra.len()) {
        return Err(FrameDataError::InvalidLength);
    }
    let mut histograms = [[0_u64; 256]; 3];
    for pixel in bgra.chunks_exact(4) {
        for (channel, value) in [pixel[2], pixel[1], pixel[0]].into_iter().enumerate() {
            histograms[channel][usize::from(value)] += 1;
        }
    }
    let mut mean_rgb_milli = [0; 3];
    let mut stddev_rgb_milli = [0; 3];
    let mut entropy_rgb_millibits = [0; 3];
    let mut uniform_rgb = true;
    for (channel, histogram) in histograms.iter().enumerate() {
        let mut sum = 0_u64;
        let mut square_sum = 0_u64;
        let mut occupied_bins = 0;
        let mut entropy = 0.0;
        for (value, &count) in histogram.iter().enumerate().filter(|(_, n)| **n != 0) {
            occupied_bins += 1;
            sum += value as u64 * count;
            square_sum += (value as u64).pow(2) * count;
            let probability = count as f64 / pixel_count as f64;
            entropy -= probability * probability.log2();
        }
        // Integer variance numerator avoids cancellation for almost-uniform frames.
        // The transport ceiling bounds every intermediate comfortably within u128.
        let variance_numerator =
            u128::from(pixel_count) * u128::from(square_sum) - u128::from(sum).pow(2);
        let stddev = (variance_numerator as f64).sqrt() / pixel_count as f64;
        mean_rgb_milli[channel] = ((sum * 1000 + pixel_count / 2) / pixel_count) as u32;
        stddev_rgb_milli[channel] = (stddev * 1000.0).round() as u32;
        entropy_rgb_millibits[channel] = (entropy * 1000.0).round() as u32;
        uniform_rgb &= occupied_bins == 1;
    }
    let statistics = FrameStatistics {
        width,
        height,
        pixel_count,
        mean_rgb_milli,
        stddev_rgb_milli,
        entropy_rgb_millibits,
        uniform_rgb,
    };
    // Use exact histogram occupancy rather than rounded statistics for the decision.
    // A sufficiently sparse nonuniform frame can round to zero standard deviation.
    Ok(if uniform_rgb {
        FrameAssessment::BlankCapture {
            attempted_mode,
            statistics,
        }
    } else {
        FrameAssessment::Informative {
            attempted_mode,
            statistics,
        }
    })
}

#[cfg(test)]
mod tests;
