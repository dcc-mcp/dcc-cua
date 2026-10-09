use super::*;
use crate::runtime::{
    capture_verified_visible_bgra, live_native_evidence, map_capture_identity_error,
    native_wgc_frame_geometry, next_exact_capture_generation, validate_exact_bgra_dimensions,
    validate_live_native_evidence,
};
use dcc_cua_platform_windows::{
    ExactWindowCaptureRoute, ExactWindowPixelEvidence, ExactWindowPixelInstanceEvidence,
    NativeWindowGeometry, ResolvedWgcGeometry, WgcFrameGeometry, resolve_exact_wgc_geometry,
};
use dcc_cua_showcase::{NativeFrameInstance, NativeFrameProvenance, NativeFrameSource};

#[derive(Clone, Copy)]
pub(super) struct WindowsLiveTarget {
    process_id: u32,
    window_handle: u64,
    stream_id: u64,
    instance: ExactWindowPixelInstanceEvidence,
    route: ExactWindowCaptureRoute,
}

impl WindowsLiveTarget {
    pub(super) fn new(
        process_id: u32,
        window_handle: u64,
        stream_id: u64,
    ) -> ComputerUseResult<Self> {
        let state = dcc_cua_platform_windows::exact_window_native_state(process_id, window_handle)
            .map_err(|error| {
                ComputerUseError::new(ComputerUseErrorCode::InvalidTarget, error.to_string())
            })?;
        let route = dcc_cua_platform_windows::exact_window_capture_route(process_id, window_handle)
            .map_err(|error| map_capture_identity_error(process_id, window_handle, error))?;
        Ok(Self {
            process_id,
            window_handle,
            stream_id,
            instance: state.instance,
            route,
        })
    }

    fn evidence(self) -> ComputerUseResult<ExactWindowPixelEvidence> {
        let state = dcc_cua_platform_windows::exact_window_native_state(
            self.process_id,
            self.window_handle,
        )
        .map_err(|error| {
            ComputerUseError::new(ComputerUseErrorCode::InvalidTarget, error.to_string())
        })?;
        validate_instance(self.instance, state.instance)?;
        if state.minimized {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::TargetMinimized,
                "the live exact target is minimized",
            ));
        }
        if !state.visible {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::TargetUnavailable,
                "the live exact target is hidden",
            ));
        }
        let evidence = live_native_evidence(self.process_id, self.window_handle)?;
        validate_instance(self.instance, evidence.instance)?;
        Ok(evidence)
    }

    fn require_instance(self) -> ComputerUseResult<()> {
        let state = dcc_cua_platform_windows::exact_window_native_state(
            self.process_id,
            self.window_handle,
        )
        .map_err(|error| {
            ComputerUseError::new(ComputerUseErrorCode::InvalidTarget, error.to_string())
        })?;
        validate_instance(self.instance, state.instance)
    }

    fn require_wgc_route(self) -> ComputerUseResult<()> {
        require_wgc_route(
            dcc_cua_platform_windows::exact_window_capture_route(
                self.process_id,
                self.window_handle,
            )
            .map_err(|error| {
                map_capture_identity_error(self.process_id, self.window_handle, error)
            })?,
        )
    }

    fn provenance(
        self,
        evidence: ExactWindowPixelEvidence,
        geometry: WindowsFrameGeometry,
        generation: u64,
    ) -> FrameCaptureProvenance {
        let (source, source_rect, wgc_geometry) = match geometry {
            WindowsFrameGeometry::VerifiedVisible => (
                NativeFrameSource::VerifiedVisible,
                evidence.visible_bounds,
                None,
            ),
            WindowsFrameGeometry::Wgc(resolved) => (
                NativeFrameSource::Wgc,
                resolved.source_rect,
                Some(native_wgc_frame_geometry(resolved)),
            ),
        };
        FrameCaptureProvenance::NativeExactWindow(NativeFrameProvenance {
            source,
            process_id: self.process_id,
            window_handle: self.window_handle,
            native_instance: NativeFrameInstance {
                process_creation_time_100ns: evidence.instance.process_creation_time_100ns,
                window_thread_id: evidence.instance.window_thread_id,
                window_class_hash: evidence.instance.window_class_hash,
                owner_window_handle: evidence.instance.owner_window_handle,
            },
            native_window_bounds: evidence.bounds,
            native_visible_bounds: evidence.visible_bounds,
            source_rect,
            window_dpi: evidence.dpi,
            capture_generation: generation,
            stream_id: self.stream_id,
            wgc_geometry,
        })
    }
}

#[derive(Clone, Copy)]
enum WindowsFrameGeometry {
    VerifiedVisible,
    Wgc(ResolvedWgcGeometry),
}

fn resolve_live_wgc_geometry(
    before: ExactWindowPixelEvidence,
    after: ExactWindowPixelEvidence,
    frame: WgcFrameGeometry,
    bgra_len: usize,
    width: u32,
    height: u32,
) -> ComputerUseResult<ResolvedWgcGeometry> {
    let native = |evidence: ExactWindowPixelEvidence| NativeWindowGeometry {
        win32_bounds: evidence.bounds,
        dwm_bounds: Some(evidence.visible_bounds),
        dpi: evidence.dpi,
    };
    let resolved = resolve_exact_wgc_geometry(native(before), native(after), frame, bgra_len)
        .map_err(|error| {
            ComputerUseError::new(ComputerUseErrorCode::StaleObservation, error.to_string())
        })?;
    validate_exact_bgra_dimensions(bgra_len, width, height, resolved.source_rect)?;
    Ok(resolved)
}

fn map_live_wgc_error(error: dcc_cua_platform_windows::WgcCaptureError) -> ComputerUseError {
    ComputerUseError::new(
        if error.geometry_failure().is_some() {
            ComputerUseErrorCode::StaleObservation
        } else {
            ComputerUseErrorCode::CaptureFailed
        },
        error.to_string(),
    )
}

fn validate_instance(
    pinned: ExactWindowPixelInstanceEvidence,
    actual: ExactWindowPixelInstanceEvidence,
) -> ComputerUseResult<()> {
    if pinned != actual {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTarget,
            "the live exact process or HWND instance was replaced",
        ));
    }
    Ok(())
}

fn require_wgc_route(route: ExactWindowCaptureRoute) -> ComputerUseResult<()> {
    if route != ExactWindowCaptureRoute::Wgc {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTarget,
            "another same-executable root appeared during live WGC capture; restart requires a newly selected verified source",
        ));
    }
    Ok(())
}

enum WindowsLiveCapture {
    Persistent(dcc_cua_platform_windows::PersistentWgcCapture),
    Uninitialized,
    VerifiedVisible,
}

struct WindowsCapturedFrame {
    bgra: Vec<u8>,
    width: u32,
    height: u32,
    capture_mode: &'static str,
    provenance: FrameCaptureProvenance,
    measurement: Option<dcc_cua_platform_windows::WgcFrameMeasurement>,
}

#[cfg(windows)]
impl From<dcc_cua_platform_windows::WgcPublishedFrameMeasurement> for FrameCaptureMeasurement {
    fn from(measurement: dcc_cua_platform_windows::WgcPublishedFrameMeasurement) -> Self {
        let compositor = match measurement.compositor {
            dcc_cua_platform_windows::WgcCompositorTiming::Available {
                system_relative_time_100ns,
                compositor_to_publish,
            } => CompositorTiming::Available {
                system_relative_time_100ns,
                compositor_to_publish,
            },
            dcc_cua_platform_windows::WgcCompositorTiming::Unavailable { reason } => {
                CompositorTiming::unavailable(reason.as_str())
            }
        };
        Self::measured(
            measurement.source_wait,
            measurement.readback_total,
            measurement.gpu_copy_map,
            measurement.cpu_copy,
            compositor,
        )
    }
}

impl WindowsLiveCapture {
    fn new(target: WindowsLiveTarget) -> Self {
        if target.route == ExactWindowCaptureRoute::VerifiedVisible {
            return Self::VerifiedVisible;
        }
        // Initialize once through the same checked path as recovery. A typed
        // geometry error must reach the pause fence instead of being hidden by
        // an immediate second capture attempt.
        Self::Uninitialized
    }

    fn next_frame(&mut self, target: WindowsLiveTarget) -> ComputerUseResult<WindowsCapturedFrame> {
        crate::interactive_desktop::require_exact_window_observation_available()?;
        let before = target.evidence()?;
        if matches!(self, Self::VerifiedVisible) {
            let visible =
                capture_verified_visible_bgra(target.process_id, target.window_handle, before)?;
            let after = target.evidence()?;
            validate_live_native_evidence(&before, &after, true)?;
            validate_live_native_evidence(&visible.evidence, &after, true)?;
            return Ok(WindowsCapturedFrame {
                bgra: visible.capture.bgra,
                width: visible.capture.width,
                height: visible.capture.height,
                capture_mode: "verified_visible",
                measurement: None,
                provenance: target.provenance(
                    after,
                    WindowsFrameGeometry::VerifiedVisible,
                    visible.generation,
                ),
            });
        }
        target.require_wgc_route()?;
        validate_live_native_evidence(&before, &before, false)?;
        let (frame, capture_mode) = match self {
            Self::Persistent(capture) => match capture.next_measured_frame(FIRST_FRAME_TIMEOUT) {
                Ok(frame) => (frame, "persistent_wgc"),
                Err(error) if error.geometry_failure().is_some() => {
                    return Err(map_live_wgc_error(error));
                }
                Err(_) => self.reinitialize(target, "reinitialized_wgc_recovery")?,
            },
            Self::Uninitialized => self.reinitialize(target, "persistent_wgc")?,
            Self::VerifiedVisible => unreachable!("visible producer returned above"),
        };
        target.require_wgc_route()?;
        let after = target.evidence()?;
        validate_live_native_evidence(&before, &after, false)?;
        let geometry = resolve_live_wgc_geometry(
            before,
            after,
            frame.geometry,
            frame.bgra.len(),
            frame.width,
            frame.height,
        )?;
        crate::interactive_desktop::require_exact_window_observation_available()?;
        Ok(WindowsCapturedFrame {
            bgra: frame.bgra,
            width: frame.width,
            height: frame.height,
            capture_mode,
            provenance: target.provenance(
                after,
                WindowsFrameGeometry::Wgc(geometry),
                next_exact_capture_generation(),
            ),
            measurement: Some(frame.measurement),
        })
    }

    fn reinitialize(
        &mut self,
        target: WindowsLiveTarget,
        capture_mode: &'static str,
    ) -> ComputerUseResult<(dcc_cua_platform_windows::PersistentWgcFrame, &'static str)> {
        target.require_wgc_route()?;
        let mut capture = dcc_cua_platform_windows::PersistentWgcCapture::new(
            target.process_id,
            target.window_handle,
        )
        .map_err(map_live_wgc_error)?;
        let frame = capture
            .next_measured_frame(FIRST_FRAME_TIMEOUT)
            .map_err(map_live_wgc_error)?;
        *self = Self::Persistent(capture);
        Ok((frame, capture_mode))
    }
}

pub(super) fn run_windows_capture_loop(
    target: WindowsLiveTarget,
    capture_exclusion: Option<dcc_cua_indicator::BannerCaptureExclusionSource>,
    fps: u32,
    started_interrupt_generation: u64,
    sender: watch::Sender<LiveObservationStatus>,
    shutdown: LiveObservationShutdown,
    publications: PublicationInbox<NativePublicationCheck, ComputerUseResult<()>>,
) {
    let interval = Duration::from_secs_f64(1.0 / f64::from(fps));
    let mut sequence = 0_u64;
    let mut capture = WindowsLiveCapture::new(target);
    loop {
        if shutdown.is_requested()
            || sender.is_closed()
            || interrupt_generation_changed(started_interrupt_generation, interrupt_generation())
        {
            return;
        }
        // The producer services its single outstanding control request before
        // starting another frame, so it never races itself for exclusion.
        if let Some(work) = publications.take() {
            service_native_publication(
                work,
                target,
                capture_exclusion.as_ref(),
                None,
                &sender,
                &shutdown,
            );
            continue;
        }
        let capture_started = Instant::now();
        let captured = (|| {
            let exclusion = capture_exclusion
                .as_ref()
                .map(dcc_cua_indicator::BannerCaptureExclusionSource::begin)
                .transpose()
                .map_err(|error| {
                    ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
                })?;
            let frame = capture.next_frame(target)?;
            if let Some(source) = capture_exclusion.as_ref() {
                source.validate_active().map_err(|error| {
                    ComputerUseError::new(ComputerUseErrorCode::InvalidTarget, error.to_string())
                })?;
            }
            Ok((frame, exclusion))
        })();
        match captured {
            Ok((frame, exclusion)) => {
                if shutdown.is_requested()
                    || interrupt_generation_changed(
                        started_interrupt_generation,
                        interrupt_generation(),
                    )
                {
                    return;
                }
                sequence = sequence.saturating_add(1);
                let measurement = frame.measurement.map_or_else(
                    || {
                        FrameCaptureMeasurement::unavailable(
                            "verified_visible_has_no_compositor_or_split_timing",
                        )
                    },
                    |measurement| FrameCaptureMeasurement::from(measurement.at_publish()),
                );
                sender.send_modify(|status| {
                    status.publish_measured_frame(
                        LiveObservationFrame::new(
                            sequence,
                            frame.bgra,
                            frame.width,
                            frame.height,
                            Instant::now(),
                        )
                        .with_provenance(frame.provenance),
                        capture_started.elapsed(),
                        frame.capture_mode,
                        measurement,
                    )
                });
                // A request queued during readback borrows this already owned
                // guard. It never performs a nested exclusion acquisition.
                if let Some(work) = publications.take() {
                    service_native_publication(
                        work,
                        target,
                        capture_exclusion.as_ref(),
                        exclusion.as_ref(),
                        &sender,
                        &shutdown,
                    );
                }
            }
            Err(error) => {
                let error = target.require_instance().err().unwrap_or(error);
                // A failed native frame must revoke eligibility of the old latest frame.
                // Exact identity/route failures are terminal; temporary proof failures pause.
                if terminal_capture_error(&error) {
                    sender.send_modify(|status| status.record_terminal_error(&error));
                    return;
                }
                sender.send_modify(|status| status.record_paused_error(&error));
                if shutdown
                    .wait_timeout_or_work(PAUSE_RETRY_INTERVAL, || publications.has_pending())
                {
                    return;
                }
                continue;
            }
        }
        if shutdown.wait_timeout_or_work(interval.saturating_sub(capture_started.elapsed()), || {
            publications.has_pending()
        }) {
            return;
        }
    }
}

/// Runs only on the existing capture owner. No Win32 guard crosses a thread or await.
fn service_native_publication(
    mut work: native_publication::PublicationWork<NativePublicationCheck, ComputerUseResult<()>>,
    target: WindowsLiveTarget,
    capture_exclusion: Option<&dcc_cua_indicator::BannerCaptureExclusionSource>,
    held_exclusion: Option<&dcc_cua_indicator::BannerCaptureExclusionGuard>,
    sender: &watch::Sender<LiveObservationStatus>,
    shutdown: &LiveObservationShutdown,
) {
    let check = work.take_payload();
    let metadata = work.metadata();
    let require_live = || {
        work.ensure_live(Instant::now())
            .map_err(native_publication_failure)?;
        if Instant::now() >= work.deadline()
            || shutdown.is_requested()
            || interrupt_generation_changed(metadata.started_generation, interrupt_generation())
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "native recording was interrupted before its first-frame acknowledgement",
            ));
        }
        if metadata != check.metadata
            || metadata.stream_id != target.stream_id
            || check.proof.stream_id != target.stream_id
            || metadata.sequence == 0
            || check.proof.process_id != target.process_id
            || check.proof.window_handle != target.window_handle
            || check.target.pid != target.process_id
            || check.target.window_id != target.window_handle
            || !matches!(
                (check.proof.source, target.route),
                (NativeFrameSource::Wgc, ExactWindowCaptureRoute::Wgc)
                    | (
                        NativeFrameSource::VerifiedVisible,
                        ExactWindowCaptureRoute::VerifiedVisible
                    )
            )
            || check.proof.native_instance.process_creation_time_100ns
                != target.instance.process_creation_time_100ns
            || check.proof.native_instance.window_thread_id != target.instance.window_thread_id
            || check.proof.native_instance.window_class_hash != target.instance.window_class_hash
            || check.proof.native_instance.owner_window_handle
                != target.instance.owner_window_handle
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "native publication request changed its exact captured source",
            ));
        }
        sender
            .borrow()
            .validate_frame_eligibility(metadata.sequence)?;
        crate::interactive_desktop::require_exact_window_observation_available()
    };
    if let Err(error) = require_live() {
        let _ = work.complete(Err(error), Instant::now());
        return;
    }
    let Some(source) = capture_exclusion else {
        let error = ComputerUseError::new(
            ComputerUseErrorCode::OverlayExclusionUnavailable,
            "native recording publication requires its owned overlay exclusion source",
        );
        let _ = work.complete(Err(error), Instant::now());
        return;
    };
    let acquired_exclusion = if held_exclusion.is_none() {
        match source.begin() {
            Ok(guard) => Some(guard),
            Err(error) => {
                let _ = work.complete(Err((check.map_exclusion_error)(error)), Instant::now());
                return;
            }
        }
    } else {
        None
    };
    // Both branches retain an actual guard until the single-use reply is sent.
    let _exclusion = held_exclusion.or(acquired_exclusion.as_ref());
    let result = (|| {
        require_live()?;
        source
            .validate_active()
            .map_err(check.map_exclusion_error)?;
        (check.validate)(&check.proof, &check.target)?;
        source
            .validate_active()
            .map_err(check.map_exclusion_error)?;
        require_live()
    })();
    let _ = work.complete(result, Instant::now());
}

#[cfg(test)]
mod tests;
