//! Passive evidence preparation is separate from keyboard foreground activation.
use super::*;
use dcc_cua_protocol::capture_preparation::{
    CapturePreparationAuthorization, CapturePreparationBeginRequest, CapturePreparationStatus,
    PreparationError, PreparationFailure,
};

pub(crate) fn preparation_error(error: PreparationError) -> ComputerUseError {
    ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string()).with_details(
        ComputerUseErrorDetails {
            capture_preparation: Some(error),
            input_sent: Some(ComputerUseInputState::NotSent),
            automatic_input: Some(false),
            blind_retry: Some(false),
            fresh_observation_required: Some(true),
            ..Default::default()
        },
    )
}

#[cfg(not(windows))]
fn unavailable() -> ComputerUseError {
    ComputerUseError::new(
        ComputerUseErrorCode::BackendUnavailable,
        "temporary passive capture preparation is available only on Windows",
    )
}

#[cfg(any(windows, test))]
fn trusted_lease_deadline(
    tick_before_wall_read: u64,
    lease_expires_at_unix_ms: u64,
    unix_now_ms: u64,
) -> Result<u64, PreparationError> {
    let remaining = lease_expires_at_unix_ms
        .checked_sub(unix_now_ms)
        .filter(|remaining| *remaining > 0)
        .ok_or_else(|| PreparationError::new(PreparationFailure::Expired))?;
    tick_before_wall_read
        .checked_add(remaining)
        .ok_or_else(|| PreparationError::new(PreparationFailure::InvalidBinding))
}

impl ComputerUseSession {
    /// Consume fresh native metadata and start one explicitly authorized,
    /// ownerless exact-root preparation. No input observation is produced.
    pub async fn capture_preparation_begin(
        &mut self,
        request: &CapturePreparationBeginRequest,
        authorization: &CapturePreparationAuthorization,
        trusted_lease_expires_at_unix_ms: u64,
    ) -> ComputerUseResult<CapturePreparationStatus> {
        request.validate().map_err(preparation_error)?;
        self.ensure_local_cleanup_reusable()?;
        if request.lifetime_ms > authorization.max_lifetime_ms {
            return Err(preparation_error(PreparationError::new(
                PreparationFailure::AuthorizationDenied,
            )));
        }
        #[cfg(not(windows))]
        {
            let _ = (authorization, trusted_lease_expires_at_unix_ms);
            Err(unavailable())
        }
        #[cfg(windows)]
        {
            use dcc_cua_platform_windows::capture_preparation::{
                CapturePreparationHandle, CapturePreparationSpec,
                read_capture_preparation_clock_ms, read_capture_preparation_identity,
            };
            // Identity reads and child launch consume this fixed lease budget.
            // Sample monotonic time before reading the wall-clock expiry.
            let tick_before_wall_read = read_capture_preparation_clock_ms();
            let unix_now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| {
                    preparation_error(PreparationError::new(PreparationFailure::InvalidBinding))
                })?
                .as_millis();
            let unix_now_ms = u64::try_from(unix_now_ms).map_err(|_| {
                preparation_error(PreparationError::new(PreparationFailure::InvalidBinding))
            })?;
            let authorization_deadline_ms = trusted_lease_deadline(
                tick_before_wall_read,
                trusted_lease_expires_at_unix_ms,
                unix_now_ms,
            )
            .map_err(preparation_error)?;
            self.ensure_active()?;
            if self.pixel_observation_route != Some(PixelObservationRoute::ExplicitPixelsOnly)
                || self.recording_active
                || self.live_observation.is_some()
            {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::InvalidAction,
                    "capture preparation requires an explicit pixels_only session without an active capture producer",
                ));
            }
            self.require_capture_preparation_settled()?;
            let metadata = self.native_frame_metadata.take();
            self.invalidate_action_observations();
            let metadata = metadata.ok_or_else(|| {
                ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "capture preparation requires fresh native window_state_id metadata",
                )
            })?;
            metadata.validate(
                &request.window_state_id,
                &self.scope,
                &self.session_id,
                metadata.read_at.elapsed(),
            )?;
            let target = read_capture_preparation_identity(
                metadata.state.process_id,
                metadata.state.window_handle,
            )
            .map_err(preparation_error)?;
            let actual = target.native_instance;
            let expected = metadata.state.instance;
            if actual.process_creation_time_100ns != expected.process_creation_time_100ns
                || actual.window_thread_id != expected.window_thread_id
                || actual.window_class_hash != expected.window_class_hash
                || actual.owner_window_handle != expected.owner_window_handle
            {
                return Err(preparation_error(PreparationError::new(
                    PreparationFailure::IdentityChanged,
                )));
            }
            let spec = CapturePreparationSpec {
                preparation_id: *uuid::Uuid::new_v4().as_bytes(),
                target,
                allowed_affected_scope: Vec::new(),
                lifetime_ms: request.lifetime_ms,
                authorization_deadline_ms,
                journal_directory: authorization.journal_directory.clone().into(),
            };
            let generation = dcc_cua_interrupt::interrupt_generation();
            let handle = CapturePreparationHandle::begin(spec, |_| {
                metadata
                    .validate(
                        &request.window_state_id,
                        &self.scope,
                        &self.session_id,
                        metadata.read_at.elapsed(),
                    )
                    .map_err(|_| PreparationError::new(PreparationFailure::InvalidBinding))?;
                if dcc_cua_interrupt::interrupt_generation() != generation
                    || self.control_banner_interrupted()
                {
                    return Err(PreparationError::new(PreparationFailure::Stopped));
                }
                interactive_desktop::require_exact_window_observation_available()
                    .map_err(|_| PreparationError::new(PreparationFailure::DesktopUnavailable))
            })
            .map_err(preparation_error)?;
            // Attach ownership before polling so even a failed status read
            // leaves an owner that can revoke and report pending restoration.
            self.capture_preparation = Some(handle);
            self.capture_preparation_state()
        }
    }

    pub fn capture_preparation_state(&self) -> ComputerUseResult<CapturePreparationStatus> {
        #[cfg(not(windows))]
        return Err(unavailable());
        #[cfg(windows)]
        self.capture_preparation
            .as_ref()
            .ok_or_else(|| preparation_error(PreparationError::new(PreparationFailure::NotActive)))?
            .state()
            .map_err(preparation_error)
    }

    pub fn capture_preparation_stop(&mut self) -> ComputerUseResult<CapturePreparationStatus> {
        self.invalidate_action_observations();
        #[cfg(not(windows))]
        return Err(unavailable());
        #[cfg(windows)]
        self.capture_preparation
            .as_ref()
            .ok_or_else(|| preparation_error(PreparationError::new(PreparationFailure::NotActive)))?
            .stop(PreparationFailure::Stopped)
            .map_err(preparation_error)
    }

    /// Capture pixels without creating an actionable observation or input token.
    pub async fn capture_preparation_snapshot(
        &mut self,
    ) -> ComputerUseResult<(ComputerUseImage, Value)> {
        self.ensure_active()?;
        self.invalidate_action_observations();
        #[cfg(not(windows))]
        return Err(unavailable());
        #[cfg(windows)]
        {
            let guard = self
                .capture_preparation
                .as_ref()
                .ok_or_else(|| {
                    preparation_error(PreparationError::new(PreparationFailure::NotActive))
                })?
                .prepared_guard()
                .map_err(preparation_error)?;
            let _exclusion = self
                .control_banner
                .as_ref()
                .map(ControlBanner::begin_capture_exclusion)
                .transpose()
                .map_err(|error| {
                    map_indicator_error("exclude indicator from prepared evidence", error)
                })?;
            self.require_observed_exact_window_observation_available()?;
            let frame = guard.capture_frame().map_err(preparation_error)?;
            validate_exact_bgra_dimensions(
                frame.capture.bgra.len(),
                frame.capture.width,
                frame.capture.height,
                frame.capture.bounds,
            )?;
            validate_live_native_evidence(&frame.evidence_before, &frame.evidence_after, true)?;
            let data = encode_bgra_to_png(
                &frame.capture.bgra,
                frame.capture.width,
                frame.capture.height,
            )?;
            self.require_observed_exact_window_observation_available()?;
            let final_state = guard.validate().map_err(preparation_error)?;
            let final_evidence = live_native_evidence(
                final_state.identity.process_id,
                final_state.identity.window_handle,
            )?;
            validate_live_native_evidence(&frame.evidence_after, &final_evidence, true)?;
            let metadata = json!({
                "schema":"dcc-cua-passive-prepared-evidence-v1", "passive":true,
                "input_authorized":false, "preparation_id":frame.preparation_id,
                "process_id":final_state.identity.process_id,
                "window_handle":final_state.identity.window_handle,
                "native_instance":final_state.identity.native_instance,
                "foreground_at_capture":frame.actual_foreground,
                "foreground_at_publication":final_state.foreground,
                "captured_at_ms":frame.captured_at_ms,
                "capture_clock":"windows_get_tick_count64_ms",
                "width":frame.capture.width, "height":frame.capture.height,
                "bounds":frame.capture.bounds, "whole_desktop_capture":false,
            });
            // A slow final native proof must not outlive the preparation lease.
            guard.validate().map_err(preparation_error)?;
            Ok((
                ComputerUseImage {
                    data,
                    mime_type: "image/png".into(),
                },
                metadata,
            ))
        }
    }

    pub(super) fn require_capture_preparation_settled(&self) -> ComputerUseResult<()> {
        #[cfg(windows)]
        if let Some(preparation) = &self.capture_preparation {
            let state = preparation.state().map_err(preparation_error)?;
            if !state.cleanup_verified {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::CompletionUnknown,
                    "passive capture preparation owns pending restoration; stop and wait for verified cleanup",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
