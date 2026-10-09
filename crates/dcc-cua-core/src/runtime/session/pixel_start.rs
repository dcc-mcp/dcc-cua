use super::*;

impl ComputerUseSession {
    /// Start an exact-window observation session without initializing an
    /// accessibility provider. Observation is read-only by default; callers
    /// may explicitly activate this same exact target through the existing
    /// native activation gates. The PID/HWND binding is never widened.
    pub async fn start_pixels_only(&mut self) -> ComputerUseResult<Value> {
        self.ensure_local_cleanup_reusable()?;
        self.require_capture_preparation_settled()?;
        #[cfg(not(windows))]
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "pixels-only exact native window capture is unavailable on this platform",
        ));
        #[cfg(windows)]
        {
            if self.active {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::InvalidAction,
                    "window session is already active",
                ));
            }
            let request = ComputerUseSessionStartRequest::default();
            request.validate_for_scope(&self.scope)?;
            let target = self.resolve_target().await?;
            self.upstream_session_state = UpstreamSessionState::VisualOnly {
                reason: "explicit pixels-only observation; accessibility provider was not started"
                    .into(),
            };
            self.last_upstream_session_refresh = None;
            self.pixel_observation_route = Some(PixelObservationRoute::ExplicitPixelsOnly);
            self.finish_started_session(target, &request, None).await
        }
    }
}
