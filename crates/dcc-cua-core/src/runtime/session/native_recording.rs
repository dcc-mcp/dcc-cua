//! Explicit native-pixels sessions record video without an upstream trajectory.
//! The session keeps the existing live worker, watch and ShowcaseRecorder owner.

use super::*;
#[cfg(windows)]
use crate::live_observation::{NativePublicationCheck, NativePublicationPhase};
#[cfg(any(windows, test))]
use dcc_cua_showcase::{FrameCaptureProvenance, NativeFrameProvenance};

impl ComputerUseSession {
    pub(super) fn uses_native_video_recording(&self) -> bool {
        #[cfg(any(windows, test))]
        return self.pixel_observation_route == Some(PixelObservationRoute::ExplicitPixelsOnly);
        #[cfg(not(any(windows, test)))]
        false
    }

    pub(super) async fn native_recording_start(
        &mut self,
        request: &ComputerUseRecordingStartRequest,
    ) -> ComputerUseResult<Value> {
        self.ensure_local_cleanup_reusable()?;
        if !request.record_video {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "pixels_only recording requires record_video=true; no trajectory is available",
            ));
        }
        if self.recording_active || self.showcase.is_some() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "native video recording is already active",
            ));
        }
        #[cfg(feature = "test-support")]
        if self.synthetic_test_session {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                "synthetic sessions cannot start native recording",
            ));
        }
        #[cfg(not(windows))]
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "native pixel recording is unavailable on this platform",
        ));
        #[cfg(windows)]
        {
            let started_generation = dcc_cua_interrupt::interrupt_generation();
            let requested_at = Instant::now();
            let start_fence = self
                .live_observation
                .as_ref()
                .and_then(LiveObservation::latest_fence);
            self.require_observed_exact_window_observation_available()?;
            let target = self.require_observed_target_available().await?;
            let _banner_activity = self.begin_banner_activity(BannerActivity::Recording);
            let outcome = self
                .ensure_live_observation(&ComputerUseLiveObservationStartRequest {
                    fps: 10,
                    ..Default::default()
                })
                .await?;
            let owns_live_observation =
                outcome.disposition == LiveObservationStartDisposition::StartedNew;
            let prepared = async {
                self.require_native_recording_not_interrupted(started_generation)?;
                let observation = self
                    .live_observation
                    .as_mut()
                    .expect("live source was started");
                let stream_id = observation.stream_id();
                let fence = crate::live_observation::observation_sequence_fence(
                    stream_id,
                    start_fence,
                    self.post_action_live_sequence_fence,
                    self.observation_transition_live_sequence_fence,
                );
                let frame = observation.latest_after(fence).await?;
                let proof =
                    validate_native_recording_frame(&frame, &target, stream_id, requested_at)?;
                observation.validate_frame_eligibility(frame.sequence())?;
                self.validate_native_recording_publication(
                    proof,
                    &target,
                    frame.sequence(),
                    started_generation,
                    NativePublicationPhase::Prepare,
                )
                .await?;
                let (frames, fps) = {
                    let observation = self
                        .live_observation
                        .as_ref()
                        .expect("live source remains owned");
                    (observation.subscribe_showcase(), observation.fps())
                };
                self.start_owned_native_recorder(
                    frames,
                    &request.output_dir,
                    fps,
                    owns_live_observation,
                )
                .await?;
                // The first encoder acknowledgement is necessary but not sufficient:
                // a pause, replacement or stop during encoding still refuses startup.
                let first_encoded = self
                    .showcase
                    .as_ref()
                    .expect("ready recorder ownership is attached")
                    .recorder
                    .first_frame()
                    .clone();
                let publication = async {
                    let encoded_proof = validate_native_recording_metadata(
                        first_encoded.provenance(),
                        first_encoded.captured_at(),
                        &target,
                        stream_id,
                        requested_at,
                    )?;
                    if first_encoded.sequence() < frame.sequence()
                        || encoded_proof.native_instance != proof.native_instance
                        || encoded_proof.source != proof.source
                    {
                        return Err(ComputerUseError::new(
                            ComputerUseErrorCode::StaleObservation,
                            "the first encoded frame changed its prepared native source",
                        ));
                    }
                    self.live_observation
                        .as_ref()
                        .expect("live source remains owned")
                        .validate_frame_eligibility(first_encoded.sequence())?;
                    self.validate_native_recording_publication(
                        encoded_proof,
                        &target,
                        first_encoded.sequence(),
                        started_generation,
                        NativePublicationPhase::Encoded,
                    )
                    .await
                }
                .await;
                if let Err(error) = publication {
                    return self.refuse_native_recording_startup(error).await;
                }
                Ok(())
            }
            .await;
            match prepared {
                Ok(()) => {
                    self.last_recording_video = None;
                    self.recording_active = true;
                    self.recording_expected_video = true;
                    self.recording_health = None;
                    debug_assert!(self.recording_keepalive.is_none());
                    self.set_banner_recording(true);
                    Ok(self.native_recording_state())
                }
                Err(error) => {
                    if owns_live_observation && self.live_observation.is_some() {
                        self.stop_live_observation().await;
                    }
                    self.set_banner_recording(false);
                    if self.last_recording_video.is_none() {
                        self.last_recording_video = Some(
                            RecordingVideoTerminalEvidence::from_failed_stop(
                                json!({"startup_error":{"code":error.code,"message":error.message}}),
                                &error,
                            ),
                        );
                    }
                    Err(error)
                }
            }
        }
    }

    /// Pending is recorded before the encoder can create a partial file. A
    /// canceled startup cannot turn absence of an attached owner into success.
    #[cfg(any(windows, test))]
    pub(super) async fn start_owned_native_recorder(
        &mut self,
        frames: tokio::sync::watch::Receiver<dcc_cua_showcase::LiveObservationStatus>,
        output_dir: &str,
        fps: u32,
        owns_live_observation: bool,
    ) -> ComputerUseResult<()> {
        self.ensure_local_cleanup_reusable()?;
        self.local_cleanup.recorder_pending = true;
        if self.last_recording_video.is_none() {
            self.last_recording_video = Some(RecordingVideoTerminalEvidence::from_failed_stop(
                json!({}),
                &ComputerUseError::new(
                    ComputerUseErrorCode::CompletionUnknown,
                    "native recorder startup has not acknowledged cleanup or transferred ownership",
                ),
            ));
        }
        match ShowcaseRecorder::start_with_outcome(frames, output_dir, fps).await {
            Ok(recorder) => {
                self.showcase = Some(ActiveShowcase {
                    recorder,
                    owns_live_observation,
                });
                self.local_cleanup.recorder_pending = false;
                Ok(())
            }
            Err((error, outcome)) => {
                let error = map_showcase_error(error);
                let mut evidence =
                    RecordingVideoTerminalEvidence::from_failed_stop(outcome, &error);
                evidence.record_startup_error(&error);
                self.last_recording_video = Some(evidence);
                // The failed start API joins both tasks before returning.
                self.local_cleanup.recorder_pending = false;
                Err(error)
            }
        }
    }

    #[cfg(any(windows, test))]
    pub(super) async fn refuse_native_recording_startup(
        &mut self,
        mut error: ComputerUseError,
    ) -> ComputerUseResult<()> {
        if let Err(cleanup_error) = self.finalize_owned_recording_video(Some(&error)).await {
            error.message.push_str(&format!(
                "; recorder cleanup failed: {}",
                cleanup_error.message
            ));
        }
        Err(error)
    }

    #[cfg(windows)]
    fn require_native_recording_not_interrupted(&self, started: u64) -> ComputerUseResult<()> {
        if self.control_banner_interrupted()
            || dcc_cua_interrupt::interrupt_generation_changed(
                started,
                dcc_cua_interrupt::interrupt_generation(),
            )
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "native recording was interrupted before its first-frame acknowledgement",
            ));
        }
        Ok(())
    }

    #[cfg(windows)]
    async fn validate_native_recording_publication(
        &self,
        proof: &NativeFrameProvenance,
        target: &WindowTarget,
        sequence: u64,
        started_generation: u64,
        phase: NativePublicationPhase,
    ) -> ComputerUseResult<()> {
        self.require_native_recording_not_interrupted(started_generation)?;
        if self
            .live_observation
            .as_ref()
            .is_none_or(|source| !source.is_active() || source.stream_id() != proof.stream_id)
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "native recording source stopped or changed before startup completed",
            ));
        }
        interactive_desktop::require_exact_window_observation_available()?;
        let source = self
            .live_observation
            .as_ref()
            .expect("the exact live source was checked");
        source
            .validate_native_publication(NativePublicationCheck::new(
                phase,
                proof.clone(),
                target.clone(),
                sequence,
                started_generation,
                validate_native_live_publication,
                |error| map_indicator_error("validate native recording source", error),
            ))
            .await?;
        self.require_native_recording_not_interrupted(started_generation)?;
        if !source.is_active() || source.stream_id() != proof.stream_id {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "native recording source stopped or changed before startup completed",
            ));
        }
        source.validate_frame_eligibility(sequence)?;
        interactive_desktop::require_exact_window_observation_available()
    }

    pub(super) async fn native_recording_stop(&mut self) -> ComputerUseResult<Value> {
        if !self.recording_active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "native video recording is not active",
            ));
        }
        // Teardown is owned by this session even after the target disappears.
        // It deliberately performs no target, accessibility or upstream probe.
        self.stop_recording_keepalive().await;
        let _activity = self.begin_banner_activity(BannerActivity::Waiting);
        self.finalize_owned_recording_video(None).await?;
        Ok(self.native_recording_state())
    }

    pub(super) async fn finalize_owned_recording_video(
        &mut self,
        startup_error: Option<&ComputerUseError>,
    ) -> ComputerUseResult<()> {
        self.recording_active = false;
        self.set_banner_recording(false);
        // Keep actual partial paths/progress before consuming the owner. If
        // this future is cancelled, no later stop may claim it was finalized.
        if let Some(showcase) = self.showcase.as_ref() {
            self.local_cleanup.recorder_pending = true;
            let error = ComputerUseError::new(
                ComputerUseErrorCode::CompletionUnknown,
                "recording finalization has not been acknowledged",
            );
            let mut evidence =
                RecordingVideoTerminalEvidence::from_failed_stop(showcase.recorder.state(), &error);
            if let Some(error) = startup_error {
                evidence.record_startup_error(error);
            }
            self.last_recording_video = Some(evidence);
        }
        let (result, outcome, owns_live) = match self.showcase.take() {
            Some(showcase) => {
                let owns_live = showcase.owns_live_observation;
                let (result, outcome) = showcase.recorder.stop_with_outcome().await;
                self.local_cleanup.recorder_pending = false;
                (result.map_err(map_showcase_error), outcome, owns_live)
            }
            None => {
                self.local_cleanup.recorder_pending = true;
                let error = ComputerUseError::new(
                    ComputerUseErrorCode::CompletionUnknown,
                    "active native recording lost its recorder owner",
                );
                (Err(error), json!({"active":false,"finalized":false}), false)
            }
        };
        // Retain encoder evidence and failures before awaiting source cleanup.
        let result = result.and_then(RecordingVideoTerminalEvidence::try_from_finalized);
        let result = match result {
            Ok(evidence) => {
                let mut evidence = evidence;
                if let Some(error) = startup_error {
                    evidence.record_startup_error(error);
                }
                self.last_recording_video = Some(evidence);
                Ok(())
            }
            Err(error) => {
                let mut evidence =
                    RecordingVideoTerminalEvidence::from_failed_stop(outcome, &error);
                if let Some(error) = startup_error {
                    evidence.record_startup_error(error);
                }
                self.last_recording_video = Some(evidence);
                self.local_cleanup
                    .remember(ComputerUseCleanupPhase::RecordingStop, error.clone());
                Err(error)
            }
        };
        if owns_live {
            self.stop_live_observation().await;
            if result.is_ok() && self.local_cleanup.source_pending {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::CompletionUnknown,
                    "recording media drained but its owned live source did not acknowledge shutdown",
                ));
            }
        }
        self.set_banner_activity(BannerActivity::Ready);
        result
    }

    pub(super) fn native_recording_state(&self) -> Value {
        let active_video = self
            .showcase
            .as_ref()
            .map(|showcase| showcase.recorder.state());
        let video = active_video.as_ref().or_else(|| {
            self.last_recording_video
                .as_ref()
                .map(RecordingVideoTerminalEvidence::state)
        });
        let mut state = project_native_recording_state(
            self.recording_active,
            video,
            self.live_observation.as_ref().map(LiveObservation::state),
        );
        state["cleanup_pending"] = json!(self.local_cleanup.pending());
        state["cleanup_issues"] = json!(self.local_cleanup.stop_issues());
        state
    }
}

#[cfg(any(windows, test))]
fn validate_native_recording_frame<'a>(
    frame: &'a crate::live_observation::LiveObservationFrame,
    target: &WindowTarget,
    stream_id: u64,
    requested_at: Instant,
) -> ComputerUseResult<&'a NativeFrameProvenance> {
    validate_native_recording_metadata(
        frame.provenance(),
        frame.captured_at(),
        target,
        stream_id,
        requested_at,
    )
}

#[cfg(any(windows, test))]
fn validate_native_recording_metadata<'a>(
    provenance: &'a FrameCaptureProvenance,
    captured_at: Instant,
    target: &WindowTarget,
    stream_id: u64,
    requested_at: Instant,
) -> ComputerUseResult<&'a NativeFrameProvenance> {
    let FrameCaptureProvenance::NativeExactWindow(proof) = provenance else {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "native video requires actual exact-window capture provenance",
        ));
    };
    if proof.process_id != target.pid
        || proof.window_handle != target.window_id
        || proof.stream_id != stream_id
        || proof.capture_generation == 0
        || proof.native_instance.process_creation_time_100ns == 0
        || proof.native_instance.window_thread_id == 0
        || captured_at < requested_at
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "native recording needs a fresh frame from its exact target and stream",
        ));
    }
    Ok(proof)
}

fn project_native_recording_state(
    active: bool,
    video: Option<&Value>,
    source: Option<Value>,
) -> Value {
    let mut issues = Vec::new();
    let video_active = video.and_then(|video| video["active"].as_bool()) == Some(true);
    let paused = active && video_active && video.is_some_and(|video| video["paused"] == true);
    if active && !video_active {
        issues.push("video_stopped");
    }
    if paused {
        issues.push("video_paused");
    }
    if video.is_some_and(|video| !video["terminal_reason"].is_null())
        || source
            .as_ref()
            .is_some_and(|source| !source["terminal_reason"].is_null())
    {
        issues.push("source_terminal");
    }
    if active
        && !paused
        && source
            .as_ref()
            .is_some_and(|source| source["paused"] == true)
    {
        issues.push("source_pause_pending");
    }
    let failed_finalization = video.is_some_and(|video| !video["error"].is_null());
    let failed_start = video.is_some_and(|video| !video["startup_error"].is_null());
    let failed = failed_finalization || failed_start;
    if failed_finalization {
        issues.push("video_finalization_failed");
    }
    if failed_start {
        issues.push("recording_start_failed");
    }
    let status = if failed {
        "failed"
    } else if !active {
        "stopped"
    } else if paused {
        "paused"
    } else if issues.is_empty() {
        "active"
    } else {
        "degraded"
    };
    json!({
        "backend":"native_pixels_video", "status":status, "active":active,
        "healthy":issues.is_empty(), "expected_components":["video"], "issues":issues,
        "trajectory_available":false, "trajectory":null, "video":video, "source":source,
    })
}

#[cfg(test)]
mod tests;
