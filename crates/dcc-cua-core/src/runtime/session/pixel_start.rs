use super::*;

impl ComputerUseSession {
    /// Start an exact-window observation session without initializing an
    /// accessibility provider. This route is read-only until a later explicit
    /// semantic session is opened and never widens the PID/HWND binding.
    pub async fn start_pixels_only(&mut self) -> ComputerUseResult<Value> {
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
