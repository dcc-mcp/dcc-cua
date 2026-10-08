use rstest::rstest;

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

#[rstest]
fn actual_fixture_dimensions_select_exact_dwm_origin() {
    let geometry = resolve_exact_wgc_geometry(native(), native(), frame(), 646 * 495 * 4).unwrap();
    assert_eq!(geometry.source_rect, [73, 80, 646, 495]);
    assert_eq!(geometry.origin, WgcSourceOrigin::DwmExtendedFrame);
}

#[rstest]
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

#[rstest]
fn equal_sizes_with_different_origins_are_ambiguous() {
    let mut value = native();
    value.win32_bounds = [60, 80, 646, 495];
    assert_eq!(
        resolve_exact_wgc_geometry(value, value, frame(), 646 * 495 * 4),
        Err(WgcGeometryError::AmbiguousOrigin)
    );
}

#[rstest]
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

#[rstest]
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

#[rstest]
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
