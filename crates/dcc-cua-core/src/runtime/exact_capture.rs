use super::*;

#[cfg(windows)]
use std::sync::atomic::AtomicU64;

pub(super) struct ExactWindowCapture {
    #[cfg(windows)]
    pub(super) diagnostics: Option<crate::capture_diagnostics::PixelBufferDiagnostics>,
    #[cfg(windows)]
    pub(super) wgc_geometry: Option<dcc_cua_platform_windows::ResolvedWgcGeometry>,
    pub(super) data: Vec<u8>,
    pub(super) backend: &'static str,
    pub(super) fallback: &'static str,
    #[cfg(windows)]
    pub(super) mode: ExactWindowPixelCaptureMode,
    #[cfg(windows)]
    pub(super) generation: u64,
    #[cfg(windows)]
    pub(super) dpi: u32,
    #[cfg(windows)]
    pub(super) bounds: [i32; 4],
    #[cfg(windows)]
    pub(super) source_rect: [i32; 4],
    #[cfg(windows)]
    pub(super) native_evidence: dcc_cua_platform_windows::ExactWindowPixelEvidence,
}

#[cfg(windows)]
static EXACT_WINDOW_CAPTURE_GENERATION: AtomicU64 = AtomicU64::new(1);

#[cfg(windows)]
pub(super) fn exact_capture_diagnostic(
    process_id: u32,
    window_id: u64,
    stage: ComputerUseCaptureStage,
    reason: ComputerUseCaptureReason,
) -> ComputerUseCaptureDiagnostic {
    ComputerUseCaptureDiagnostic {
        stage,
        reason,
        target_process_id: process_id,
        target_window_handle: window_id,
        target_bounds: None,
        blocker_process_id: None,
        blocker_window_handle: None,
        blocker_bounds: None,
        cloaked: None,
        os_error: None,
        root_bounds_failure: None,
    }
}

#[cfg(windows)]
pub(crate) fn map_visible_capture_error(
    code: ComputerUseErrorCode,
    stage: ComputerUseCaptureStage,
    process_id: u32,
    window_id: u64,
    error: dcc_cua_platform_windows::VisibleWindowCaptureError,
) -> ComputerUseError {
    let message = error.to_string();
    map_visible_capture_diagnostic(
        code,
        stage,
        process_id,
        window_id,
        *error.diagnostic,
        message,
    )
}

#[cfg(windows)]
pub(super) fn map_visible_capture_diagnostic(
    code: ComputerUseErrorCode,
    stage: ComputerUseCaptureStage,
    process_id: u32,
    window_id: u64,
    native: dcc_cua_platform_windows::VisibleWindowCaptureDiagnostic,
    message: impl Into<String>,
) -> ComputerUseError {
    use ComputerUseCaptureReason as Public;
    use dcc_cua_platform_windows::VisibleWindowCaptureReason as Native;
    let reason = match native.reason {
        Native::NativeReadFailed => Public::NativeReadFailed,
        Native::TargetUnavailable => Public::TargetUnavailable,
        Native::TargetNotVisible => Public::TargetNotVisible,
        Native::TargetMinimized => Public::TargetMinimized,
        Native::TargetBoundsInvalid => Public::TargetBoundsInvalid,
        Native::TargetOutsideDesktop => Public::TargetOutsideDesktop,
        Native::RootCloakingUnavailable => Public::RootCloakingUnavailable,
        Native::RootBoundsUnavailable => Public::RootBoundsUnavailable,
        Native::RootBoundsInvalid => Public::RootBoundsInvalid,
        Native::RootEnumerationIncomplete => Public::RootEnumerationIncomplete,
        Native::TargetNotReached => Public::TargetNotReached,
        Native::TargetBoundsChanged => Public::TargetBoundsChanged,
        Native::RootOverlap => Public::RootOverlap,
    };
    let mut capture = exact_capture_diagnostic(process_id, window_id, stage, reason);
    capture.target_bounds = native.target_bounds;
    capture.blocker_process_id = native.blocker_process_id;
    capture.blocker_window_handle = native.blocker_window_handle;
    capture.blocker_bounds = native.blocker_bounds;
    capture.cloaked = native.cloaked;
    capture.os_error = native.os_error;
    capture.root_bounds_failure = native
        .root_bounds_failure
        .map(super::map_root_bounds_failure);
    ComputerUseError::new(code, message).with_details(ComputerUseErrorDetails {
        capture: Some(capture),
        phase: Some(ComputerUseErrorPhase::EvidenceDispatch),
        ..Default::default()
    })
}

#[cfg(windows)]
pub(crate) fn map_root_bounds_failure(
    failure: dcc_cua_platform_windows::RootBoundsFailureDiagnostic,
) -> crate::ComputerUseRootBoundsFailureDiagnostic {
    use crate::{ComputerUseRootBoundsFailureDiagnostic, ComputerUseRootBoundsRole};
    use dcc_cua_platform_windows::RootBoundsRole;
    ComputerUseRootBoundsFailureDiagnostic {
        root_role: match failure.root_role {
            RootBoundsRole::TargetRoot => ComputerUseRootBoundsRole::TargetRoot,
            RootBoundsRole::AboveTargetRoot => ComputerUseRootBoundsRole::AboveTargetRoot,
        },
        proof_target_root_window_handle: failure.proof_target_root_window_handle,
        dwm_raw_rect_edges: failure.dwm_raw_rect_edges,
        dwm_classification: map_root_bounds_class(failure.dwm_classification),
        visible: failure.visible,
        cloaked: failure.cloaked,
        win32_read_after_dwm_rejection: failure.win32_read_after_dwm_rejection,
        win32_raw_rect_edges: failure.win32_raw_rect_edges,
        win32_classification: failure.win32_classification.map(map_root_bounds_class),
        win32_os_error: failure.win32_os_error,
        zero_area_status_mismatch: failure.zero_area_status_mismatch,
    }
}

#[cfg(windows)]
pub(super) fn map_root_bounds_class(
    native: dcc_cua_platform_windows::RootBoundsClass,
) -> crate::ComputerUseRootBoundsClass {
    use crate::ComputerUseRootBoundsClass as Public;
    use dcc_cua_platform_windows::RootBoundsClass as Native;
    match native {
        Native::Positive => Public::Positive,
        Native::ZeroArea => Public::ZeroArea,
        Native::Inverted => Public::Inverted,
        Native::Overflow => Public::Overflow,
    }
}

#[cfg(windows)]
pub(crate) fn map_capture_identity_error(
    process_id: u32,
    window_id: u64,
    error: dcc_cua_platform_windows::ExactWindowCaptureIdentityError,
) -> ComputerUseError {
    ComputerUseError::new(ComputerUseErrorCode::InvalidTarget, error.to_string()).with_details(
        ComputerUseErrorDetails {
            capture: Some(exact_capture_diagnostic(
                process_id,
                window_id,
                ComputerUseCaptureStage::CaptureIdentity,
                ComputerUseCaptureReason::CaptureIdentityUnavailable,
            )),
            phase: Some(ComputerUseErrorPhase::EvidenceDispatch),
            ..Default::default()
        },
    )
}

#[cfg(windows)]
pub(crate) struct VerifiedVisibleBgraFrame {
    pub capture: dcc_cua_platform_windows::VisibleWindowCapture,
    pub evidence: dcc_cua_platform_windows::ExactWindowPixelEvidence,
    pub generation: u64,
}

#[cfg(windows)]
pub(crate) fn next_exact_capture_generation() -> u64 {
    EXACT_WINDOW_CAPTURE_GENERATION.fetch_add(1, Ordering::Relaxed)
}

#[cfg(windows)]
pub(crate) fn live_native_evidence(
    process_id: u32,
    window_id: u64,
) -> ComputerUseResult<dcc_cua_platform_windows::ExactWindowPixelEvidence> {
    let evidence = dcc_cua_platform_windows::exact_window_pixel_evidence(process_id, window_id)
        .map_err(|error| {
            map_visible_capture_error(
                ComputerUseErrorCode::CaptureFailed,
                ComputerUseCaptureStage::NativeEvidence,
                process_id,
                window_id,
                error,
            )
        })?;
    // Native proof performs a potentially slow occlusion traversal after its
    // geometry reads. Re-read actual metadata before admitting those pixels.
    let final_state = dcc_cua_platform_windows::exact_window_native_state(process_id, window_id)
        .map_err(|error| {
            ComputerUseError::new(ComputerUseErrorCode::InvalidTarget, error.to_string())
        })?;
    validate_live_final_native_state(&evidence, &final_state)?;
    Ok(evidence)
}

#[cfg(windows)]
pub(super) fn validate_live_final_native_state(
    evidence: &dcc_cua_platform_windows::ExactWindowPixelEvidence,
    state: &dcc_cua_platform_windows::ExactWindowNativeState,
) -> ComputerUseResult<()> {
    if evidence.process_id != state.process_id
        || evidence.window_handle != state.window_handle
        || evidence.instance != state.instance
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTarget,
            "the exact live target instance changed after native proof",
        ));
    }
    if state.minimized {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetMinimized,
            "the live target was minimized after native proof",
        ));
    }
    if !state.visible {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetUnavailable,
            "the live target was hidden after native proof",
        ));
    }
    if state.bounds != Some(evidence.bounds)
        || state.visible_bounds != Some(evidence.visible_bounds)
        || state.dpi == 0
        || state.dpi != evidence.dpi
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "the exact live Win32/DWM geometry or DPI changed after native proof",
        ));
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn validate_live_native_evidence(
    before: &dcc_cua_platform_windows::ExactWindowPixelEvidence,
    after: &dcc_cua_platform_windows::ExactWindowPixelEvidence,
    visible: bool,
) -> ComputerUseResult<()> {
    validate_native_exact_window_pixel_evidence(
        before,
        after,
        if visible {
            ExactWindowPixelCaptureMode::VisibleDesktopCrop
        } else {
            ExactWindowPixelCaptureMode::WindowContent
        },
    )
}

#[cfg(windows)]
pub(crate) fn validate_exact_bgra_dimensions(
    length: usize,
    width: u32,
    height: u32,
    source_rect: [i32; 4],
) -> ComputerUseResult<()> {
    let expected = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .and_then(|bytes| usize::try_from(bytes).ok());
    if width == 0
        || height == 0
        || expected != Some(length)
        || i32::try_from(width).ok() != Some(source_rect[2])
        || i32::try_from(height).ok() != Some(source_rect[3])
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "the verified native BGRA buffer does not match its physical source rectangle",
        ));
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn validate_live_frame_provenance(
    proof: &dcc_cua_showcase::NativeFrameProvenance,
    target: &WindowTarget,
    final_evidence: &dcc_cua_platform_windows::ExactWindowPixelEvidence,
) -> ComputerUseResult<()> {
    let captured = dcc_cua_platform_windows::ExactWindowPixelEvidence {
        process_id: proof.process_id,
        window_handle: proof.window_handle,
        bounds: proof.native_window_bounds,
        visible_bounds: proof.native_visible_bounds,
        dpi: proof.window_dpi,
        visible: true,
        minimized: false,
        unobscured: true,
        visibility_failure: None,
        instance: dcc_cua_platform_windows::ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: proof.native_instance.process_creation_time_100ns,
            window_thread_id: proof.native_instance.window_thread_id,
            window_class_hash: proof.native_instance.window_class_hash,
            owner_window_handle: proof.native_instance.owner_window_handle,
        },
    };
    if proof.process_id != target.pid
        || proof.window_handle != target.window_id
        || proof.native_window_bounds != target.bounds
        || proof.capture_generation == 0
        || proof.window_dpi == 0
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "the live frame proof does not match the final exact target",
        ));
    }
    validate_live_source_geometry(proof)?;
    validate_live_native_evidence(
        &captured,
        final_evidence,
        proof.source == dcc_cua_showcase::NativeFrameSource::VerifiedVisible,
    )
}

#[cfg(windows)]
pub(crate) fn native_wgc_frame_geometry(
    geometry: dcc_cua_platform_windows::ResolvedWgcGeometry,
) -> dcc_cua_showcase::NativeWgcFrameGeometry {
    let frame = geometry.frame;
    dcc_cua_showcase::NativeWgcFrameGeometry {
        item_size_before: frame.item_size_before,
        item_size_after: frame.item_size_after,
        pool_size: frame.pool_size,
        content_size: frame.content_size,
        texture_size: frame.texture_size,
        row_pitch_bytes: frame.row_pitch_bytes,
        bgra_byte_len: geometry.bgra_byte_len,
    }
}

#[cfg(windows)]
pub(super) fn validate_live_source_geometry(
    proof: &dcc_cua_showcase::NativeFrameProvenance,
) -> ComputerUseResult<()> {
    use dcc_cua_showcase::NativeFrameSource;
    let invalid = || {
        ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "the live native frame lacks its actual physical source geometry proof",
        )
    };
    match proof.source {
        NativeFrameSource::VerifiedVisible => {
            if proof.wgc_geometry.is_some() || proof.source_rect != proof.native_visible_bounds {
                return Err(invalid());
            }
        }
        NativeFrameSource::Wgc => {
            let measured = proof.wgc_geometry.ok_or_else(invalid)?;
            let native = dcc_cua_platform_windows::NativeWindowGeometry {
                win32_bounds: proof.native_window_bounds,
                dwm_bounds: Some(proof.native_visible_bounds),
                dpi: proof.window_dpi,
            };
            let resolved = dcc_cua_platform_windows::resolve_exact_wgc_geometry(
                native,
                native,
                dcc_cua_platform_windows::WgcFrameGeometry {
                    item_size_before: measured.item_size_before,
                    item_size_after: measured.item_size_after,
                    pool_size: measured.pool_size,
                    content_size: measured.content_size,
                    texture_size: measured.texture_size,
                    row_pitch_bytes: measured.row_pitch_bytes,
                },
                measured.bgra_byte_len,
            )
            .map_err(|_| invalid())?;
            if proof.source_rect != resolved.source_rect {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn validate_native_live_publication(
    proof: &dcc_cua_showcase::NativeFrameProvenance,
    target: &WindowTarget,
) -> ComputerUseResult<()> {
    let final_evidence = live_native_evidence(target.pid, target.window_id)?;
    validate_live_frame_provenance(proof, target, &final_evidence)?;
    if proof.source == dcc_cua_showcase::NativeFrameSource::Wgc
        && dcc_cua_platform_windows::exact_window_capture_route(target.pid, target.window_id)
            .map_err(|error| map_capture_identity_error(target.pid, target.window_id, error))?
            != dcc_cua_platform_windows::ExactWindowCaptureRoute::Wgc
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTarget,
            "live WGC source became ambiguous before screenshot publication",
        ));
    }
    Ok(())
}

/// Runs synchronously inside the existing snapshot/live capture worker. No PNG round trip.
#[cfg(windows)]
pub(crate) fn capture_verified_visible_bgra(
    process_id: u32,
    window_id: u64,
    before: dcc_cua_platform_windows::ExactWindowPixelEvidence,
) -> ComputerUseResult<VerifiedVisibleBgraFrame> {
    interactive_desktop::require_exact_window_observation_available()?;
    if before.process_id != process_id || before.window_handle != window_id {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTarget,
            "the native capture fence belongs to a different exact target",
        ));
    }
    validate_live_native_evidence(&before, &before, true)?;
    dcc_cua_platform_windows::exact_window_capture_route(process_id, window_id)
        .map_err(|error| map_capture_identity_error(process_id, window_id, error))?;
    let capture = dcc_cua_platform_windows::capture_visible_window(process_id, window_id).map_err(
        |error| {
            map_visible_capture_error(
                ComputerUseErrorCode::CaptureFailed,
                ComputerUseCaptureStage::VisibleDesktopProof,
                process_id,
                window_id,
                error,
            )
        },
    )?;
    let after = live_native_evidence(process_id, window_id)?;
    validate_live_native_evidence(&before, &after, true)?;
    dcc_cua_platform_windows::exact_window_capture_route(process_id, window_id)
        .map_err(|error| map_capture_identity_error(process_id, window_id, error))?;
    if capture.bounds != after.visible_bounds {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "the verified visible crop moved during native capture",
        ));
    }
    validate_exact_bgra_dimensions(
        capture.bgra.len(),
        capture.width,
        capture.height,
        capture.bounds,
    )?;
    interactive_desktop::require_exact_window_observation_available()?;
    Ok(VerifiedVisibleBgraFrame {
        capture,
        evidence: after,
        generation: next_exact_capture_generation(),
    })
}

#[cfg(windows)]
pub(super) async fn capture_exact_window(
    process_id: u32,
    window_id: u64,
) -> ComputerUseResult<ExactWindowCapture> {
    capture_exact_window_with_diagnostics(process_id, window_id, false).await
}

#[cfg(windows)]
pub(super) async fn capture_exact_window_with_diagnostics(
    process_id: u32,
    window_id: u64,
    diagnostics_enabled: bool,
) -> ComputerUseResult<ExactWindowCapture> {
    tokio::task::spawn_blocking(move || {
        let capture_started = std::time::Instant::now();
        let generation = EXACT_WINDOW_CAPTURE_GENERATION.fetch_add(1, Ordering::Relaxed);
        let before = dcc_cua_platform_windows::exact_window_pixel_evidence(
            process_id,
            window_id,
        )
        .map_err(|error| {
            map_visible_capture_error(ComputerUseErrorCode::InvalidTarget,
                ComputerUseCaptureStage::NativeEvidence, process_id, window_id, error)
        })?;
        validate_native_exact_window_pixel_evidence(
            &before,
            &before,
            ExactWindowPixelCaptureMode::WindowContent,
        )?;
        let route = dcc_cua_platform_windows::exact_window_capture_route(process_id, window_id)
            .map_err(|error| {
                map_capture_identity_error(process_id, window_id, error)
            })?;
        if route == dcc_cua_platform_windows::ExactWindowCaptureRoute::VerifiedVisible {
            let visible = capture_verified_visible_bgra(process_id, window_id, before)?;
            let capture_to_raw_elapsed = capture_started.elapsed();
            let encoded = crate::capture_diagnostics::encode_exact_frame(
                &visible.capture.bgra, visible.capture.width, visible.capture.height,
                diagnostics_enabled, capture_to_raw_elapsed,
            )?;
            return Ok(ExactWindowCapture {
                data: encoded.data,
                diagnostics: encoded.diagnostics,
                wgc_geometry: None,
                backend: "dcc-cua-visible-exact-window",
                fallback: "same_executable_multi_window_exact_visible_proof",
                mode: ExactWindowPixelCaptureMode::VisibleDesktopCrop,
                generation: visible.generation,
                dpi: visible.evidence.dpi,
                bounds: visible.evidence.bounds,
                source_rect: visible.capture.bounds,
                native_evidence: visible.evidence,
            });
        }
        let wgc_error =
            match dcc_cua_platform_windows::PersistentWgcCapture::new(process_id, window_id) {
            Ok(mut capture) => match capture.next_measured_frame(Duration::from_secs(5)) {
                Ok(frame) => {
                    let capture_to_raw_elapsed = capture_started.elapsed();
                    if dcc_cua_platform_windows::exact_window_capture_route(
                        process_id,
                        window_id,
                    )
                    .map_err(|error| {
                        map_capture_identity_error(process_id, window_id, error)
                    })? != dcc_cua_platform_windows::ExactWindowCaptureRoute::Wgc
                    {
                        return Err(ComputerUseError::new(
                            ComputerUseErrorCode::InvalidTarget,
                            "another window from the target executable appeared during WGC capture; pixels were discarded",
                        ));
                    }
                    let after = dcc_cua_platform_windows::exact_window_pixel_evidence(
                        process_id,
                        window_id,
                    )
                    .map_err(|error| {
                        map_visible_capture_error(ComputerUseErrorCode::InvalidTarget,
                            ComputerUseCaptureStage::NativeEvidence, process_id, window_id, error)
                    })?;
                    validate_native_exact_window_pixel_evidence(
                        &before,
                        &after,
                        ExactWindowPixelCaptureMode::WindowContent,
                    )?;
                    let geometry = dcc_cua_platform_windows::resolve_exact_wgc_geometry(
                        dcc_cua_platform_windows::NativeWindowGeometry {
                            win32_bounds: before.bounds, dwm_bounds: Some(before.visible_bounds), dpi: before.dpi,
                        },
                        dcc_cua_platform_windows::NativeWindowGeometry {
                            win32_bounds: after.bounds, dwm_bounds: Some(after.visible_bounds), dpi: after.dpi,
                        },
                        frame.geometry, frame.bgra.len(),
                    ).map_err(|error| ComputerUseError::new(ComputerUseErrorCode::StaleObservation, error.to_string()))?;
                    if frame.geometry.content_size != [frame.width, frame.height] {
                        return Err(ComputerUseError::new(ComputerUseErrorCode::StaleObservation,
                            "the WGC raw frame dimensions differ from its actual content proof"));
                    }
                    let encoded = crate::capture_diagnostics::encode_exact_frame(
                        &frame.bgra, frame.width, frame.height, diagnostics_enabled, capture_to_raw_elapsed,
                    )?;
                    return Ok(ExactWindowCapture {
                        data: encoded.data,
                        diagnostics: encoded.diagnostics,
                        wgc_geometry: Some(geometry),
                        backend: "dcc-cua-wgc-exact-window",
                        fallback: "exact_window_wgc",
                        mode: ExactWindowPixelCaptureMode::WindowContent,
                        generation,
                        dpi: after.dpi,
                        bounds: after.bounds,
                        source_rect: geometry.source_rect,
                        native_evidence: after,
                    });
                }
                Err(error) => {
                    if let Some(reason) = error.geometry_failure() {
                        return Err(ComputerUseError::new(ComputerUseErrorCode::StaleObservation, reason.to_string()));
                    }
                    error.to_string()
                },
            },
            Err(error) => {
                if let Some(reason) = error.geometry_failure() {
                    return Err(ComputerUseError::new(ComputerUseErrorCode::StaleObservation, reason.to_string()));
                }
                error.to_string()
            },
        };
        let visible = capture_verified_visible_bgra(process_id, window_id, before)
            .map_err(|mut error| {
                error.message = format!("exact WGC capture failed ({wgc_error}); {}", error.message);
                error
            })?;
        let capture_to_raw_elapsed = capture_started.elapsed();
        let encoded = crate::capture_diagnostics::encode_exact_frame(
            &visible.capture.bgra, visible.capture.width, visible.capture.height,
            diagnostics_enabled, capture_to_raw_elapsed,
        )?;
        Ok(ExactWindowCapture {
            data: encoded.data,
            diagnostics: encoded.diagnostics,
            wgc_geometry: None,
            backend: "dcc-cua-visible-exact-window",
            fallback: "verified_same_process_visible_window_crop",
            mode: ExactWindowPixelCaptureMode::VisibleDesktopCrop,
            generation: visible.generation,
            dpi: visible.evidence.dpi,
            bounds: visible.evidence.bounds,
            source_rect: visible.capture.bounds,
            native_evidence: visible.evidence,
        })
    })
    .await
    .map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            format!("exact window capture task failed: {error}"),
        )
    })?
    .map_err(|mut error| {
        let details = error.details.get_or_insert_default();
        if details.capture.is_none() {
            details.capture = Some(exact_capture_diagnostic(process_id, window_id,
                ComputerUseCaptureStage::PublicationValidation,
                ComputerUseCaptureReason::PixelEvidenceChanged));
        }
        error
    })
}

#[cfg(not(windows))]
pub(super) async fn capture_exact_window(
    _process_id: u32,
    _window_id: u64,
) -> ComputerUseResult<ExactWindowCapture> {
    Err(ComputerUseError::new(
        ComputerUseErrorCode::BackendUnavailable,
        "exact native window capture is unavailable on this platform",
    ))
}
