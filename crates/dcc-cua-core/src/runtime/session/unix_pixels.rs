//! Exact native read-only observation and owned worker cleanup on Unix desktops.
use super::*;
use crate::runtime::unix_pixels::{self, NativePixels};

impl ComputerUseSession {
    pub(super) fn require_unix_cleanup_settled(&self) -> ComputerUseResult<()> {
        if !self.active && self.unix_pixels.is_some() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::CompletionUnknown,
                "previous exact-window capture cleanup is pending; finish stop before reusing this session",
            ));
        }
        Ok(())
    }

    pub(super) async fn drain_unix_pixels(&mut self) {
        // Keep the handle in the session across await. Canceling stop must not
        // make a still-owned read disappear or allow an immediate new bind.
        if let Some(handle) = self.unix_pixels.clone() {
            if let Err(error) = unix_pixels::operate(handle, |_| Ok(())).await {
                self.local_cleanup
                    .remember(ComputerUseCleanupPhase::LiveObservationStop, error);
            }
            self.unix_pixels = None;
        }
    }

    pub(super) fn require_not_read_only_pixels(&self) -> ComputerUseResult<()> {
        if self.pixel_observation_route == Some(PixelObservationRoute::ExplicitPixelsOnly) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "the selected operation has no authorized exact-window pixel route",
            )
            .with_details(ComputerUseErrorDetails {
                action_attempted: Some(false),
                input_sent: Some(ComputerUseInputState::NotSent),
                automatic_rebind: Some(false),
                ..Default::default()
            }));
        }
        Ok(())
    }

    pub(super) async fn start_unix_pixels(&mut self) -> ComputerUseResult<Value> {
        if self.active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "window session is already active",
            ));
        }
        let request = ComputerUseSessionStartRequest::default();
        request.validate_for_scope(&self.scope)?;
        let scope = self.scope.clone();
        let authorization = self
            .native_task_authorization
            .as_ref()
            .map(|authorization| (self.session_id.clone(), Arc::clone(authorization)));
        let (handle, target) =
            tokio::task::spawn_blocking(move || NativePixels::bind(scope, authorization))
                .await
                .map_err(|error| {
                    ComputerUseError::new(
                        ComputerUseErrorCode::BackendUnavailable,
                        error.to_string(),
                    )
                })??;
        self.upstream_session_state = UpstreamSessionState::UnixPixelsOnly {
            input_available: false,
        };
        self.last_upstream_session_refresh = None;
        self.pixel_observation_route = Some(PixelObservationRoute::ExplicitPixelsOnly);
        let result = self.finish_started_session(target, &request, None).await?;
        self.unix_pixels = Some(handle);
        Ok(result)
    }

    #[cfg(target_os = "linux")]
    pub async fn enable_guarded_x11_input(
        &mut self,
        lease: Arc<crate::RecordingAuthorization>,
    ) -> ComputerUseResult<()> {
        self.ensure_active()?;
        let handle = self.unix_pixels.clone().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "guarded X11 input requires a retained exact pixel session",
            )
        })?;
        unix_pixels::operate(handle, |bound| {
            lease.with_active(|| ()).map_err(|_| {
                ComputerUseError::new(
                    ComputerUseErrorCode::InvalidTarget,
                    "X11 input lease expired or was revoked",
                )
            })?;
            bound.input_lease = Some(lease);
            Ok(())
        })
        .await?;
        self.upstream_session_state = UpstreamSessionState::UnixPixelsOnly {
            input_available: true,
        };
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub(super) async fn guarded_x11_action(
        &mut self,
        action: &ComputerUseAction,
        observation: ComputerUseObservation,
        deadline: Option<Instant>,
        generation: u64,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        use std::sync::atomic::AtomicBool;
        struct Cancel(Arc<AtomicBool>);
        impl Drop for Cancel {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let deadline = deadline.ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "guarded X11 input requires the connected task deadline",
            )
        })?;
        super::super::pixel_input_policy::validate_pixel_action(action)?;
        self.require_capture_preparation_settled()?;
        let target = self.require_observed_target_available().await?;
        reject_ambiguous_embedded_browser_navigation(action, &target)?;
        validate_action_observation(action, &observation)?;
        let handle = self.unix_pixels.clone().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                "missing X11 input binding",
            )
        })?;
        let cancel = Cancel(Arc::new(AtomicBool::new(false)));
        let cancelled = Arc::clone(&cancel.0);
        let action = action.clone();
        self.invalidate_action_observations();
        let result = unix_pixels::operate(handle, move |bound| {
            bound.guarded_action(action, observation, deadline, generation, cancelled)
        })
        .await;
        let result = self.finish_observation_sensitive_attempt(result)?;
        Ok(self.complete_action(result))
    }

    pub(super) async fn screenshot_unix_pixels(
        &mut self,
        diagnostics: bool,
    ) -> ComputerUseResult<ComputerUseScreenshot> {
        self.ensure_active()?;
        self.require_capture_preparation_settled()?;
        if diagnostics {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "Windows byte diagnostics are unavailable on this read-only native route",
            ));
        }
        let handle = self.unix_pixels.clone().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "native capture requires start_pixels_only on the same session",
            )
        })?;
        // Revoke before await, including if the caller cancels this future.
        // The retained handle still owns/drains the bounded native read.
        self.invalidate_action_observations();
        let result = unix_pixels::operate(handle, NativePixels::capture).await;
        let (data, target, provenance) = self.finish_observation_sensitive_attempt(result)?;
        let (width, height) = png_dimensions(&data).ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "native capture returned invalid PNG bytes",
            )
        })?;
        let observation = ComputerUseObservation {
            observation_id: format!(
                "{}-{}",
                self.session_id,
                OBSERVATION_COUNTER.fetch_add(1, Ordering::Relaxed)
            ),
            window_handle: target.window_id,
            process_id: target.pid,
            window_title: target.title.clone(),
            width,
            height,
            source_rect: target.bounds,
            capture_backend: unix_pixels::CAPTURE_BACKEND.into(),
            capture_provenance: provenance,
            session_id: self.session_id.clone(),
        };
        let accessibility = json!({"accessibility_available":false,"degraded":false,
            "observation_mode":"pixels_only","pid":target.pid,"window_id":target.window_id});
        self.target = Some(target);
        self.observation = Some(observation.clone());
        Ok(ComputerUseScreenshot {
            data,
            observation,
            accessibility,
        })
    }
}
