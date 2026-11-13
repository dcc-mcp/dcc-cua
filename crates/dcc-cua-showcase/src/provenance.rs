use serde::{Deserialize, Serialize};

/// Immutable source evidence travels with pixels, independently of video size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FrameCaptureProvenance {
    Portable,
    NativeExactWindow(Box<NativeFrameProvenance>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeFrameSource {
    Wgc,
    VerifiedVisible,
}

impl NativeFrameSource {
    #[must_use]
    pub const fn backend(self) -> &'static str {
        match self {
            Self::Wgc => "dcc-cua-wgc-exact-window",
            Self::VerifiedVisible => "dcc-cua-visible-exact-window",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeFrameInstance {
    pub process_creation_time_100ns: u64,
    pub window_thread_id: u32,
    pub window_class_hash: u64,
    pub owner_window_handle: u64,
}

/// Actual WGC measurements carried as portable data, without native APIs or
/// an independent geometry policy in the media encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeWgcFrameGeometry {
    pub item_size_before: [u32; 2],
    pub item_size_after: [u32; 2],
    pub pool_size: [u32; 2],
    pub content_size: [u32; 2],
    pub texture_size: [u32; 2],
    pub row_pitch_bytes: u32,
    pub bgra_byte_len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeFrameProvenance {
    pub source: NativeFrameSource,
    pub process_id: u32,
    pub window_handle: u64,
    pub native_instance: NativeFrameInstance,
    pub native_window_bounds: [i32; 4],
    pub native_visible_bounds: [i32; 4],
    pub source_rect: [i32; 4],
    pub window_dpi: u32,
    pub capture_generation: u64,
    pub stream_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wgc_geometry: Option<NativeWgcFrameGeometry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wgc_timing: Option<NativeWgcFrameTiming>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_preparation: Option<PreparedCaptureFrameProvenance>,
}

/// Raw source and publication clocks remain distinct in every sample sidecar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeWgcFrameTiming {
    pub compositor_time_100ns: i64,
    pub published_qpc_100ns: i64,
    pub published_at_unix_ms: u128,
    /// Full calibration bracket; midpoint uncertainty is at most half this.
    pub calibration_bracket_ns: u64,
    pub pipeline: NativeCapturePipelineTiming,
}

impl NativeWgcFrameTiming {
    /// Freshness uses the earliest possible capture, not the media midpoint.
    #[must_use]
    pub fn capture_uncertainty(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(self.calibration_bracket_ns.div_ceil(2).saturating_add(100))
    }
}

/// Disjoint wall-clock phases, not CPU costs. Source capture includes its wait
/// and readback. Clock mapping and status serialization are outside these phases.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeCapturePipelineTiming {
    pub recording_precheck_us: u64,
    pub exclusion_acquire_us: u64,
    pub source_capture_us: u64,
    pub source_validation_us: u64,
    pub recording_admission_us: u64,
    pub publication_validation_us: u64,
}

/// Actual passive preparation binding carried with each recorded frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedCaptureFrameProvenance {
    pub preparation_id: [u8; 16],
    pub actual_foreground: bool,
    /// Windows GetTickCount64 monotonic uptime milliseconds, never UTC.
    pub captured_at_ms: u64,
}
