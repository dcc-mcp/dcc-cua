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
mod tests {
    use super::*;

    fn native() -> NativeWindowGeometry {
        NativeWindowGeometry {
            win32_bounds: [60, 80, 672, 508],
            dwm_bounds: Some([73, 80, 646, 495]),
            dpi: 240,
        }
    }

    fn frame() -> WgcFrameGeometry {
        WgcFrameGeometry {
            item_size_before: [646, 495],
            item_size_after: [646, 495],
            pool_size: [646, 495],
            content_size: [646, 495],
            texture_size: [646, 495],
            row_pitch_bytes: 2688,
        }
    }

    #[test]
    fn actual_fixture_dimensions_select_exact_dwm_origin() {
        let geometry =
            resolve_exact_wgc_geometry(native(), native(), frame(), 646 * 495 * 4).unwrap();
        assert_eq!(geometry.source_rect, [73, 80, 646, 495]);
        assert_eq!(geometry.origin, WgcSourceOrigin::DwmExtendedFrame);
    }

    #[test]
    fn only_exact_win32_or_identical_rectangles_can_select_other_origins() {
        let mut shape = frame();
        shape.item_size_before = [672, 508];
        shape.item_size_after = [672, 508];
        shape.pool_size = [672, 508];
        shape.content_size = [672, 508];
        shape.texture_size = [672, 508];
        shape.row_pitch_bytes = 2688;
        assert_eq!(
            resolve_exact_wgc_geometry(native(), native(), shape, 672 * 508 * 4)
                .unwrap()
                .origin,
            WgcSourceOrigin::Win32Window
        );
        let mut same = native();
        same.dwm_bounds = Some(same.win32_bounds);
        assert_eq!(
            resolve_exact_wgc_geometry(same, same, shape, 672 * 508 * 4)
                .unwrap()
                .origin,
            WgcSourceOrigin::IdenticalNativeBounds
        );
    }

    #[test]
    fn equal_sizes_with_different_origins_are_ambiguous() {
        let mut value = native();
        value.win32_bounds = [60, 80, 646, 495];
        assert_eq!(
            resolve_exact_wgc_geometry(value, value, frame(), 646 * 495 * 4),
            Err(WgcGeometryError::AmbiguousOrigin)
        );
    }

    #[test]
    fn every_native_geometry_component_must_remain_stable() {
        for change in 0..9 {
            let before = native();
            let mut after = before;
            match change {
                0..=3 => after.win32_bounds[change] += 1,
                4..=7 => after.dwm_bounds.as_mut().unwrap()[change - 4] += 1,
                _ => after.dpi += 1,
            }
            assert_eq!(
                resolve_exact_wgc_geometry(before, after, frame(), 646 * 495 * 4),
                Err(WgcGeometryError::NativeGeometryChanged)
            );
        }
    }

    #[test]
    fn missing_invalid_or_unmatched_native_bounds_fail_closed() {
        let mut value = native();
        value.dwm_bounds = None;
        assert_eq!(
            resolve_exact_wgc_geometry(value, value, frame(), 646 * 495 * 4),
            Err(WgcGeometryError::MissingDwmBounds)
        );
        for change in 0..3 {
            let mut value = native();
            match change {
                0 => value.dpi = 0,
                1 => value.dwm_bounds = Some([0, 0, 0, 1]),
                _ => value.win32_bounds = [i32::MAX, 0, 10, 10],
            }
            assert_eq!(
                resolve_exact_wgc_geometry(value, value, frame(), 646 * 495 * 4),
                Err(WgcGeometryError::InvalidNativeGeometry)
            );
        }
        let mut value = native();
        value.dwm_bounds.as_mut().unwrap()[2] -= 1;
        assert_eq!(
            resolve_exact_wgc_geometry(value, value, frame(), 646 * 495 * 4),
            Err(WgcGeometryError::NoNativeRectangleMatch)
        );
    }

    #[test]
    fn every_actual_wgc_size_and_full_raw_shape_is_required() {
        for change in 0..6 {
            let mut value = frame();
            match change {
                0 => value.item_size_before[0] += 1,
                1 => value.item_size_after[1] += 1,
                2 => value.pool_size[0] += 1,
                3 => value.texture_size[1] += 1,
                4 => value.content_size[0] += 1,
                _ => value.row_pitch_bytes = 1,
            }
            assert!(validate_wgc_frame_geometry(value, 646 * 495 * 4).is_err());
        }
        for length in [0, 646 * 495 * 4 - 1, 646 * 495 * 4 + 1] {
            assert_eq!(
                validate_wgc_frame_geometry(frame(), length),
                Err(WgcGeometryError::RawShapeMismatch)
            );
        }
        let mut value = frame();
        value.content_size = [0, 0];
        assert_eq!(
            validate_wgc_frame_geometry(value, 0),
            Err(WgcGeometryError::InvalidFrameSize)
        );
    }
}
