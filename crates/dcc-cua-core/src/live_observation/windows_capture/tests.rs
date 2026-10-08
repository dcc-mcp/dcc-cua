use rstest::rstest;

use super::*;

fn evidence() -> ExactWindowPixelEvidence {
    ExactWindowPixelEvidence {
        process_id: 42,
        window_handle: 500,
        bounds: [-100, 20, 100, 90],
        visible_bounds: [-98, 21, 96, 86],
        dpi: 144,
        visible: true,
        minimized: false,
        unobscured: true,
        instance: ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: 1000,
            window_thread_id: 8,
            window_class_hash: 90,
            owner_window_handle: 0,
        },
    }
}

#[rstest]
fn verified_frame_admission_requires_every_native_fence_component() {
    let before = evidence();
    for mutation in 0..12 {
        let mut after = before;
        match mutation {
            0 => after.process_id += 1,
            1 => after.window_handle += 1,
            2 => after.instance.process_creation_time_100ns += 1,
            3 => after.instance.window_thread_id += 1,
            4 => after.instance.window_class_hash += 1,
            5 => after.instance.owner_window_handle += 1,
            6 => after.bounds[0] += 1,
            7 => after.visible_bounds[2] -= 1,
            8 => after.dpi += 1,
            9 => after.visible = false,
            10 => after.minimized = true,
            11 => after.unobscured = false,
            _ => unreachable!(),
        }
        assert!(
            validate_live_native_evidence(&before, &after, true).is_err(),
            "mutation {mutation}"
        );
    }
    assert!(validate_live_native_evidence(&before, &before, true).is_ok());
    let mut occluded_before = before;
    occluded_before.unobscured = false;
    assert!(validate_live_native_evidence(&occluded_before, &before, true).is_err());
}

#[rstest]
fn native_instance_reuse_and_wgc_route_change_are_terminal() {
    let pinned = evidence().instance;
    for mutation in 0..4 {
        let mut replacement = pinned;
        match mutation {
            0 => replacement.process_creation_time_100ns += 1,
            1 => replacement.window_thread_id += 1,
            2 => replacement.window_class_hash += 1,
            3 => replacement.owner_window_handle += 1,
            _ => unreachable!(),
        }
        assert_eq!(
            validate_instance(pinned, replacement).unwrap_err().code,
            ComputerUseErrorCode::InvalidTarget
        );
    }
    assert!(require_wgc_route(ExactWindowCaptureRoute::Wgc).is_ok());
    assert_eq!(
        require_wgc_route(ExactWindowCaptureRoute::VerifiedVisible)
            .unwrap_err()
            .code,
        ComputerUseErrorCode::InvalidTarget
    );
}

fn measured_wgc_frame() -> WgcFrameGeometry {
    WgcFrameGeometry {
        item_size_before: [96, 86],
        item_size_after: [96, 86],
        pool_size: [96, 86],
        content_size: [96, 86],
        texture_size: [96, 86],
        row_pitch_bytes: 400,
    }
}

#[rstest]
fn live_wgc_uses_shared_actual_dwm_geometry_and_carries_measurements() {
    let native = evidence();
    let resolved =
        resolve_live_wgc_geometry(native, native, measured_wgc_frame(), 96 * 86 * 4, 96, 86)
            .unwrap();
    assert_eq!(resolved.source_rect, native.visible_bounds);
    assert_ne!(resolved.source_rect, native.bounds);
    let target = WindowsLiveTarget {
        process_id: native.process_id,
        window_handle: native.window_handle,
        stream_id: 7,
        instance: native.instance,
        route: ExactWindowCaptureRoute::Wgc,
    };
    let FrameCaptureProvenance::NativeExactWindow(proof) =
        target.provenance(native, WindowsFrameGeometry::Wgc(resolved), 9)
    else {
        panic!("native WGC provenance")
    };
    assert_eq!(proof.source_rect, native.visible_bounds);
    assert_eq!(proof.native_window_bounds, native.bounds);
    let measured = proof.wgc_geometry.unwrap();
    assert_eq!(measured.content_size, [96, 86]);
    assert_eq!(measured.row_pitch_bytes, 400);
    assert_eq!(measured.bgra_byte_len, 96 * 86 * 4);
}

#[rstest]
fn live_wgc_shared_geometry_rejects_drift_ambiguity_and_unproved_raw_shape() {
    let before = evidence();
    for mutation in 0..8 {
        let mut after = before;
        let mut frame = measured_wgc_frame();
        let mut length = 96 * 86 * 4;
        let mut width = 96;
        match mutation {
            0 => after.bounds[0] += 1,
            1 => after.visible_bounds[0] += 1,
            2 => after.dpi += 1,
            3 => frame.item_size_after[0] += 1,
            4 => frame.texture_size[1] += 1,
            5 => frame.row_pitch_bytes = 380,
            6 => length -= 1,
            7 => width += 1,
            _ => unreachable!(),
        }
        assert!(
            resolve_live_wgc_geometry(before, after, frame, length, width, 86).is_err(),
            "mutation {mutation}"
        );
    }
    let mut ambiguous = before;
    ambiguous.bounds[2] = 96;
    ambiguous.bounds[3] = 86;
    assert!(
        resolve_live_wgc_geometry(
            ambiguous,
            ambiguous,
            measured_wgc_frame(),
            96 * 86 * 4,
            96,
            86
        )
        .is_err()
    );
}

#[rstest]
fn raw_frame_admission_rejects_length_crop_and_dimension_mismatch() {
    assert!(validate_exact_bgra_dimensions(96 * 86 * 4, 96, 86, [-98, 21, 96, 86]).is_ok());
    for (length, width, height, rect) in [
        (3, 1, 1, [0, 0, 1, 1]),
        (5, 1, 1, [0, 0, 1, 1]),
        (4, 1, 1, [0, 0, 2, 1]),
        (0, 0, 0, [0, 0, 0, 0]),
        (4, u32::MAX, u32::MAX, [0, 0, 1, 1]),
    ] {
        assert!(validate_exact_bgra_dimensions(length, width, height, rect).is_err());
    }
}

#[rstest]
fn visible_provenance_retains_crop_and_no_wgc_timestamps() {
    let evidence = evidence();
    let target = WindowsLiveTarget {
        process_id: evidence.process_id,
        window_handle: evidence.window_handle,
        stream_id: 7,
        instance: evidence.instance,
        route: ExactWindowCaptureRoute::VerifiedVisible,
    };
    let FrameCaptureProvenance::NativeExactWindow(proof) =
        target.provenance(evidence, WindowsFrameGeometry::VerifiedVisible, 9)
    else {
        panic!("native proof");
    };
    assert_eq!(proof.source_rect, evidence.visible_bounds);
    assert!(proof.wgc_geometry.is_none());
    assert_ne!(proof.source_rect, proof.native_window_bounds);
    assert_eq!(proof.stream_id, 7);
    assert_eq!(
        proof.native_instance.window_thread_id,
        evidence.instance.window_thread_id
    );
    let timing =
        FrameCaptureMeasurement::unavailable("verified_visible_has_no_compositor_or_split_timing");
    assert!(timing.source_wait.is_none() && timing.gpu_copy_map.is_none());
    assert_eq!(timing.compositor.as_json()["status"], "unavailable");
}
