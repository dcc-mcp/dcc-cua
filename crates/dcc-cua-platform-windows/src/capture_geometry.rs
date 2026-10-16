//! Pure physical geometry admission shared by snapshots and native frame producers.
//! This proves geometry only; exact HWND/instance/desktop/route authorization is separate.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub(crate) const MAX_WGC_FRAME_PIXELS: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeWindowGeometry {
    pub win32_bounds: [i32; 4],
    pub dwm_bounds: Option<[i32; 4]>,
    pub dpi: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WgcFrameGeometry {
    pub item_size_before: [u32; 2],
    pub item_size_after: [u32; 2],
    pub pool_size: [u32; 2],
    pub content_size: [u32; 2],
    pub texture_size: [u32; 2],
    pub row_pitch_bytes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WgcSourceOrigin {
    Win32Window,
    DwmExtendedFrame,
    IdenticalNativeBounds,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedWgcGeometry {
    pub source_rect: [i32; 4],
    pub origin: WgcSourceOrigin,
    pub frame: WgcFrameGeometry,
    pub bgra_byte_len: usize,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum WgcGeometryError {
    #[error("actual WGC frame geometry metadata is unavailable")]
    FrameMetadataUnavailable,
    #[error("native Win32/DWM bounds or DPI changed after capture")]
    NativeGeometryChanged,
    #[error("exact WGC physical DWM bounds proof is missing")]
    MissingDwmBounds,
    #[error("exact WGC physical native bounds or DPI are invalid")]
    InvalidNativeGeometry,
    #[error("WGC content dimensions are invalid or exceed the capture limit")]
    InvalidFrameSize,
    #[error("actual WGC item, pool, content or texture dimensions differ")]
    FrameSizeChanged,
    #[error("actual WGC row pitch or complete BGRA byte length is invalid")]
    RawShapeMismatch,
    #[error("WGC dimensions match native rectangles with different physical origins")]
    AmbiguousOrigin,
    #[error("WGC dimensions do not match a complete native physical rectangle")]
    NoNativeRectangleMatch,
}

pub fn validate_wgc_frame_geometry(
    frame: WgcFrameGeometry,
    bgra_byte_len: usize,
) -> Result<(), WgcGeometryError> {
    let [width, height] = frame.content_size;
    let count = usize::try_from(width)
        .ok()
        .and_then(|width| usize::try_from(height).ok()?.checked_mul(width))
        .filter(|count| *count > 0 && *count <= MAX_WGC_FRAME_PIXELS)
        .ok_or(WgcGeometryError::InvalidFrameSize)?;
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(WgcGeometryError::InvalidFrameSize);
    }
    if [
        frame.item_size_before,
        frame.item_size_after,
        frame.pool_size,
        frame.texture_size,
    ]
    .iter()
    .any(|size| *size != frame.content_size)
    {
        return Err(WgcGeometryError::FrameSizeChanged);
    }
    let row_bytes = width
        .checked_mul(4)
        .ok_or(WgcGeometryError::RawShapeMismatch)?;
    if frame.row_pitch_bytes < row_bytes || count.checked_mul(4) != Some(bgra_byte_len) {
        return Err(WgcGeometryError::RawShapeMismatch);
    }
    Ok(())
}

fn valid_rectangle(rect: [i32; 4]) -> bool {
    rect[2] > 0
        && rect[3] > 0
        && rect[0].checked_add(rect[2]).is_some()
        && rect[1].checked_add(rect[3]).is_some()
}

/// Validate independently acquired physical bounds. GetWindowRect may include
/// invisible borders and be DPI-virtualized; DWM extended bounds are separate:
/// https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowrect
pub fn validate_native_window_geometry(
    before: NativeWindowGeometry,
    after: NativeWindowGeometry,
) -> Result<(), WgcGeometryError> {
    if before != after {
        return Err(WgcGeometryError::NativeGeometryChanged);
    }
    let dwm = after.dwm_bounds.ok_or(WgcGeometryError::MissingDwmBounds)?;
    if after.dpi == 0 || !valid_rectangle(after.win32_bounds) || !valid_rectangle(dwm) {
        return Err(WgcGeometryError::InvalidNativeGeometry);
    }
    Ok(())
}

/// Resolve one unique native physical origin without scaling, padding or cropping.
/// All WGC sizes must be actual API values, never derived from requested bounds.
pub fn resolve_exact_wgc_geometry(
    before: NativeWindowGeometry,
    after: NativeWindowGeometry,
    frame: WgcFrameGeometry,
    bgra_byte_len: usize,
) -> Result<ResolvedWgcGeometry, WgcGeometryError> {
    validate_native_window_geometry(before, after)?;
    let dwm = after.dwm_bounds.ok_or(WgcGeometryError::MissingDwmBounds)?;
    let win32 = after.win32_bounds;
    validate_wgc_frame_geometry(frame, bgra_byte_len)?;
    let matches = |bounds: [i32; 4]| [bounds[2] as u32, bounds[3] as u32] == frame.content_size;
    let (source_rect, origin) = match (matches(win32), matches(dwm)) {
        (true, true) if win32 == dwm => (win32, WgcSourceOrigin::IdenticalNativeBounds),
        (true, true) => return Err(WgcGeometryError::AmbiguousOrigin),
        (true, false) => (win32, WgcSourceOrigin::Win32Window),
        (false, true) => (dwm, WgcSourceOrigin::DwmExtendedFrame),
        (false, false) => return Err(WgcGeometryError::NoNativeRectangleMatch),
    };
    Ok(ResolvedWgcGeometry {
        source_rect,
        origin,
        frame,
        bgra_byte_len,
    })
}

#[cfg(test)]
mod tests;
