use super::*;
use crate::runtime::{
    capture_verified_visible_bgra, live_native_evidence, map_capture_identity_error,
    next_exact_capture_generation, validate_exact_bgra_dimensions, validate_live_native_evidence,
};
use dcc_cua_platform_windows::{
    ExactWindowCaptureRoute, ExactWindowPixelEvidence, ExactWindowPixelInstanceEvidence,
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
        source: NativeFrameSource,
        generation: u64,
    ) -> FrameCaptureProvenance {
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
            source_rect: if source == NativeFrameSource::VerifiedVisible {
                evidence.visible_bounds
            } else {
                evidence.bounds
            },
            window_dpi: evidence.dpi,
            capture_generation: generation,
            stream_id: self.stream_id,
        })
    }
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
        dcc_cua_platform_windows::PersistentWgcCapture::new(target.process_id, target.window_handle)
            .map_or(Self::Uninitialized, Self::Persistent)
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
                    NativeFrameSource::VerifiedVisible,
                    visible.generation,
                ),
            });
        }
        target.require_wgc_route()?;
        validate_live_native_evidence(&before, &before, false)?;
        let (frame, capture_mode) = match self {
            Self::Persistent(capture) => match capture.next_measured_frame(FIRST_FRAME_TIMEOUT) {
                Ok(frame) => (frame, "persistent_wgc"),
                Err(_) => self.reinitialize(target)?,
            },
            Self::Uninitialized => self.reinitialize(target)?,
            Self::VerifiedVisible => unreachable!("visible producer returned above"),
        };
        target.require_wgc_route()?;
        let after = target.evidence()?;
        validate_live_native_evidence(&before, &after, false)?;
        validate_exact_bgra_dimensions(frame.bgra.len(), frame.width, frame.height, after.bounds)?;
        crate::interactive_desktop::require_exact_window_observation_available()?;
        Ok(WindowsCapturedFrame {
            bgra: frame.bgra,
            width: frame.width,
            height: frame.height,
            capture_mode,
            provenance: target.provenance(
                after,
                NativeFrameSource::Wgc,
                next_exact_capture_generation(),
            ),
            measurement: Some(frame.measurement),
        })
    }

    fn reinitialize(
        &mut self,
        target: WindowsLiveTarget,
    ) -> ComputerUseResult<(dcc_cua_platform_windows::PersistentWgcFrame, &'static str)> {
        target.require_wgc_route()?;
        let mut capture = dcc_cua_platform_windows::PersistentWgcCapture::new(
            target.process_id,
            target.window_handle,
        )
        .map_err(|error| {
            ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
        })?;
        let frame = capture
            .next_measured_frame(FIRST_FRAME_TIMEOUT)
            .map_err(|error| {
                ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
            })?;
        *self = Self::Persistent(capture);
        Ok((frame, "reinitialized_wgc_recovery"))
    }
}

pub(super) fn run_windows_capture_loop(
    target: WindowsLiveTarget,
    capture_exclusion: Option<dcc_cua_indicator::BannerCaptureExclusionSource>,
    fps: u32,
    started_interrupt_generation: u64,
    sender: watch::Sender<LiveObservationStatus>,
    shutdown: LiveObservationShutdown,
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
            Ok((frame, _exclusion)) => {
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
                if shutdown.wait_timeout(PAUSE_RETRY_INTERVAL) {
                    return;
                }
                continue;
            }
        }
        if shutdown.wait_timeout(interval.saturating_sub(capture_started.elapsed())) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
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
            target.provenance(evidence, NativeFrameSource::VerifiedVisible, 9)
        else {
            panic!("native proof");
        };
        assert_eq!(proof.source_rect, evidence.visible_bounds);
        assert_ne!(proof.source_rect, proof.native_window_bounds);
        assert_eq!(proof.stream_id, 7);
        assert_eq!(
            proof.native_instance.window_thread_id,
            evidence.instance.window_thread_id
        );
        let timing = FrameCaptureMeasurement::unavailable(
            "verified_visible_has_no_compositor_or_split_timing",
        );
        assert!(timing.source_wait.is_none() && timing.gpu_copy_map.is_none());
        assert_eq!(timing.compositor.as_json()["status"], "unavailable");
    }
}
