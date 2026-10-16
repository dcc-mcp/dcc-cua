//! Linux's first pixels-only slice deliberately grants observation alone.
use super::*;
use crate::runtime::linux_pixels::{self, LinuxPixels};

impl ComputerUseSession {
    pub(super) fn require_linux_cleanup_settled(&self) -> ComputerUseResult<()> {
        if !self.active && self.linux_pixels.is_some() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::CompletionUnknown,
                "previous X11 capture cleanup is pending; finish stop before reusing this session",
            ));
        }
        Ok(())
    }

    pub(super) async fn drain_linux_pixels(&mut self) {
        // Keep the handle in the session across await. Canceling stop must not
        // make a still-owned read disappear or allow an immediate new bind.
        if let Some(handle) = self.linux_pixels.clone() {
            if let Err(error) = linux_pixels::operate(handle, |_| Ok(())).await {
                self.local_cleanup
                    .remember(ComputerUseCleanupPhase::LiveObservationStop, error);
            }
            self.linux_pixels = None;
        }
    }

    pub(super) fn require_not_linux_pixels(&self) -> ComputerUseResult<()> {
        if self.pixel_observation_route == Some(PixelObservationRoute::ExplicitPixelsOnly) {
            return Err(ComputerUseError::new(ComputerUseErrorCode::BackendUnavailable,
                "Linux X11 pixels-only sessions are read-only; input, activation, accessibility and recording are unsupported")
                .with_details(ComputerUseErrorDetails { action_attempted: Some(false),
                    input_sent: Some(ComputerUseInputState::NotSent), automatic_rebind: Some(false),
                    ..Default::default() }));
        }
        Ok(())
    }

    pub(super) async fn start_linux_pixels(&mut self) -> ComputerUseResult<Value> {
        if self.active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "window session is already active",
            ));
        }
        let request = ComputerUseSessionStartRequest::default();
        request.validate_for_scope(&self.scope)?;
        let scope = self.scope.clone();
        let (handle, target) = tokio::task::spawn_blocking(move || LinuxPixels::bind(scope))
            .await
            .map_err(|error| {
                ComputerUseError::new(ComputerUseErrorCode::BackendUnavailable, error.to_string())
            })??;
        self.upstream_session_state = UpstreamSessionState::LinuxPixelsOnly;
        self.last_upstream_session_refresh = None;
        self.pixel_observation_route = Some(PixelObservationRoute::ExplicitPixelsOnly);
        let result = self.finish_started_session(target, &request, None).await?;
        self.linux_pixels = Some(handle);
        Ok(result)
    }

    pub(super) async fn screenshot_linux_pixels(
        &mut self,
        diagnostics: bool,
    ) -> ComputerUseResult<ComputerUseScreenshot> {
        self.ensure_active()?;
        self.require_capture_preparation_settled()?;
        if diagnostics {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "Windows byte diagnostics are unavailable on the read-only X11 route",
            ));
        }
        let handle = self.linux_pixels.clone().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "X11 capture requires start_pixels_only on the same session",
            )
        })?;
        // Revoke before await, including if the caller cancels this future.
        // The retained handle still owns/drains the bounded native read.
        self.invalidate_action_observations();
        let result = linux_pixels::operate(handle, LinuxPixels::capture).await;
        let (data, target, provenance) = self.finish_observation_sensitive_attempt(result)?;
        let (width, height) = png_dimensions(&data).ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "X11 returned invalid PNG bytes",
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
            capture_backend: "x11-get-image-exact".into(),
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
