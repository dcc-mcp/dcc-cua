use rstest::rstest;

use super::*;

#[rstest]
fn native_wgc_geometry_failure_is_distinct_from_backend_unavailability() {
    for reason in [
        WgcGeometryError::FrameMetadataUnavailable,
        WgcGeometryError::InvalidFrameSize,
        WgcGeometryError::FrameSizeChanged,
        WgcGeometryError::RawShapeMismatch,
    ] {
        assert_eq!(geometry_error(reason).geometry_failure(), Some(reason));
    }
    assert_eq!(
        capture_error("backend", "unavailable").geometry_failure(),
        None
    );
    assert_eq!(
        invalid_target("backend frame unavailable").geometry_failure(),
        None
    );
}

#[rstest]
fn actual_item_size_is_bounded_before_pool_allocation_or_recreation() {
    assert_eq!(
        actual_size(SizeInt32 {
            Width: 646,
            Height: 495
        })
        .unwrap(),
        [646, 495]
    );
    for (width, height) in [(0, 1), (-1, 1), (1, 0), (i32::MAX, i32::MAX), (8193, 8192)] {
        assert_eq!(
            actual_size(SizeInt32 {
                Width: width,
                Height: height
            })
            .unwrap_err()
            .geometry_failure(),
            Some(WgcGeometryError::InvalidFrameSize)
        );
    }
}
