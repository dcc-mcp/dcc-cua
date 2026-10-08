use serde::{Deserialize, Serialize};

/// Immutable source evidence travels with pixels, independently of video size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FrameCaptureProvenance {
    Portable,
    NativeExactWindow(NativeFrameProvenance),
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
}
