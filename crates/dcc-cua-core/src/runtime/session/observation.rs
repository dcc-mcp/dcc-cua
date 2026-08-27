use super::*;

impl ComputerUseSession {
    /// Report whether a user-confirmed action must renew its exact-target
    /// evidence before dispatch. The renewal itself remains read-only; callers
    /// must still preserve the original action and authorization binding.
    #[must_use]
    pub fn confirmed_action_evidence_refresh_due(&self) -> bool {
        self.active
            && matches!(self.upstream_session_state, UpstreamSessionState::Active)
            && self.upstream_session_refresh_due()
    }

    pub(crate) fn finish_observation_sensitive_attempt<T>(
        &mut self,
        result: ComputerUseResult<T>,
    ) -> ComputerUseResult<T> {
        if result
            .as_ref()
            .is_err_and(|error| error.code == ComputerUseErrorCode::MissingWindow)
        {
            // The exact PID/HWND fence no longer exists. Drop local target
            // ownership now so stop() cannot re-enter an upstream provider
            // that was bound to the vanished window.
            self.target = None;
        }
        if result.as_ref().is_err_and(|error| {
            matches!(
                error.code,
                ComputerUseErrorCode::MissingWindow
                    | ComputerUseErrorCode::InvalidTarget
                    | ComputerUseErrorCode::TargetMinimized
                    | ComputerUseErrorCode::TargetUnavailable
                    | ComputerUseErrorCode::InteractiveDesktopUnavailable
            )
        }) {
            self.invalidate_action_observations();
        }
        result
    }

    pub(super) fn reject_owned_modal_takeover(
        &mut self,
        target: &WindowTarget,
    ) -> ComputerUseResult<()> {
        #[cfg(windows)]
        if let Some(modal) = crate::window_target::windows_foreground_owned_takeover(target)? {
            // Detection never widens the exact target grant. Clear the stale
            // parent Banner and require a fresh explicit PID/HWND bind.
            self.control_banner = None;
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::TargetModalChanged,
                "a same-process owned window has taken over input",
            )
            .with_details(ComputerUseErrorDetails {
                phase: Some(ComputerUseErrorPhase::PreDispatch),
                action_attempted: Some(false),
                input_sent: Some(ComputerUseInputState::NotSent),
                automatic_rebind: Some(false),
                explicit_rebind_required: Some(true),
                suggested_target: Some(ComputerUseTargetScope {
                    process_id: Some(modal.pid),
                    window_handle: Some(modal.window_id),
                    window_title: Some(modal.title),
                }),
                ..Default::default()
            }));
        }
        #[cfg(not(windows))]
        let _ = target;
        Ok(())
    }
}

impl ComputerUseSession {
    pub async fn screenshot(&mut self) -> ComputerUseResult<ComputerUseScreenshot> {
        self.screenshot_with_bounds(DEFAULT_SNAPSHOT_MAX_ELEMENTS, DEFAULT_SNAPSHOT_MAX_DEPTH)
            .await
    }

    /// Capture pixels for the exact PID/HWND without consulting accessibility.
    pub async fn screenshot_pixels_only(&mut self) -> ComputerUseResult<ComputerUseScreenshot> {
        #[cfg(not(windows))]
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "pixels-only exact native window capture is unavailable on this platform",
        ));
        #[cfg(windows)]
        {
            self.ensure_active()?;
            if self.pixel_observation_route != Some(PixelObservationRoute::ExplicitPixelsOnly) {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::InvalidAction,
                    "pixels-only capture requires start_pixels_only on the same exact-window session",
                ));
            }
            let target = self.require_observed_target_available().await?;
            self.capture_window_pixels(&target, PixelObservationRoute::ExplicitPixelsOnly)
                .await
        }
    }

    /// Capture a fresh observation with bounded semantic-tree context.
    pub async fn screenshot_with_bounds(
        &mut self,
        max_elements: u32,
        max_depth: u32,
    ) -> ComputerUseResult<ComputerUseScreenshot> {
        self.ensure_active()?;
        let _banner_activity = self.begin_banner_activity(BannerActivity::Observing);
        self.refresh_upstream_session_before_observation_if_needed()
            .await?;
        if self.live_observation.is_some() {
            return self.live_observation_screenshot().await;
        }
        let target = self.require_observed_target_available().await?;
        #[cfg(windows)]
        if let Some(route) = self.pixel_observation_route {
            return self.capture_window_pixels(&target, route).await;
        }
        #[cfg(windows)]
        if matches!(
            self.upstream_session_state,
            UpstreamSessionState::VisualOnly { .. }
        ) {
            let route = PixelObservationRoute::AccessibilityTimeoutDegraded;
            self.pixel_observation_route = Some(route);
            return self.capture_window_pixels(&target, route).await;
        }
        #[cfg(windows)]
        if self.windows_uia.is_some() {
            return self
                .capture_window_visually(&target, max_elements, max_depth)
                .await;
        }
        let result = gated_exact_window_observation(
            interactive_desktop::require_exact_window_observation_available,
            || {
                call_driver_tool(
                    &self.driver.driver,
                    "get_window_state",
                    json!({
                        "window_id": target.window_id,
                        "pid": target.pid,
                        "include_screenshot": true,
                        "max_elements": bounded_snapshot_elements(max_elements),
                        "max_depth": bounded_snapshot_depth(max_depth),
                        "session": self.session_id,
                    })
                    .to_string(),
                    "capture CUA window state",
                )
            },
        )
        .await;
        let result = self.finish_observation_sensitive_attempt(result);
        let target = self.require_observed_target_available().await?;
        let result = match result {
            Ok(result)
                if result.is_error && pixel_route_for_uia_tool_failure(&result).is_some() =>
            {
                #[cfg(windows)]
                {
                    let route = pixel_route_for_uia_tool_failure(&result)
                        .expect("guard accepted UIA tool degradation");
                    self.pixel_observation_route = Some(route);
                    return self.capture_window_pixels(&target, route).await;
                }
                #[cfg(not(windows))]
                return self
                    .capture_window_visually(&target, max_elements, max_depth)
                    .await;
            }
            Ok(result) => self.finish_observed_tool_attempt("capture CUA window", Ok(result))?,
            Err(error) if pixel_route_for_accessibility_failure(&error).is_some() => {
                #[cfg(windows)]
                {
                    let route = pixel_route_for_accessibility_failure(&error)
                        .expect("guard accepted accessibility degradation");
                    self.pixel_observation_route = Some(route);
                    return self.capture_window_pixels(&target, route).await;
                }
                #[cfg(not(windows))]
                return self
                    .capture_window_visually(&target, max_elements, max_depth)
                    .await;
            }
            Err(error) => {
                return self.finish_observation_sensitive_attempt(Err(error));
            }
        };
        let image = result.images.first().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "CUA window state returned no screenshot",
            )
        })?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(&image.data_base64)
            .map_err(|error| {
                ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
            })?;
        let (width, height) = png_dimensions(&data).ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "CUA returned a non-PNG or truncated screenshot",
            )
        })?;
        let accessibility = result
            .structured_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_else(|| json!({}));
        #[cfg(windows)]
        let mut accessibility = accessibility;
        #[cfg(windows)]
        if !crate::windows_uia_fallback::accessibility_has_closed_policy_tiers(&accessibility) {
            match self
                .windows_accessibility_snapshot(&target, max_elements, max_depth)
                .await
            {
                Ok(windows_accessibility) => accessibility = windows_accessibility,
                Err(error) if pixel_route_for_accessibility_failure(&error).is_some() => {
                    let route = pixel_route_for_accessibility_failure(&error)
                        .expect("guard accepted accessibility degradation");
                    self.pixel_observation_route = Some(route);
                    self.windows_uia = None;
                    return self.capture_window_pixels(&target, route).await;
                }
                Err(_) => self.windows_uia = None,
            }
        }
        let target = self
            .revalidate_observed_exact_publication_target(&target)
            .await?;
        let accessibility_backend = accessibility["backend"]
            .as_str()
            .unwrap_or("cua-driver-sdk");
        let accessibility_available = accessibility["elements"].as_array().is_some();
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
            capture_backend: "cua-driver-sdk".into(),
            capture_provenance: json!({
                "backend": "cua-driver-sdk",
                "pixels_captured": true,
                "scope": "window",
                "accessibility_available": accessibility_available,
                "accessibility_backend": accessibility_backend,
                "process_id": target.pid,
                "window_handle": target.window_id,
            }),
            session_id: self.session_id.clone(),
        };
        self.target = Some(target);
        self.observation = Some(observation.clone());
        self.set_banner_activity(BannerActivity::Ready);
        Ok(ComputerUseScreenshot {
            data,
            observation,
            accessibility,
        })
    }

    async fn live_observation_screenshot(&mut self) -> ComputerUseResult<ComputerUseScreenshot> {
        let stream_id = self
            .live_observation
            .as_ref()
            .expect("live observation was checked")
            .stream_id();
        self.require_observed_exact_window_observation_available()?;
        self.require_observed_target_available().await?;
        self.require_observed_exact_window_observation_available()?;
        self.rebaseline_live_observation_transition_fence();
        let previous_fence = self.observation.as_ref().and_then(|observation| {
            Some(LiveObservationFence::new(
                observation.capture_provenance["live_stream_id"].as_u64()?,
                observation.capture_provenance["live_frame_sequence"].as_u64()?,
            ))
        });
        let action_completion_fence = self.post_action_live_sequence_fence;
        let transition_fence = self.observation_transition_live_sequence_fence;
        let after_sequence = observation_sequence_fence(
            stream_id,
            previous_fence,
            action_completion_fence,
            transition_fence,
        );
        let previous_sequence = previous_fence.and_then(|fence| fence.sequence_for(stream_id));
        let action_completion_sequence =
            action_completion_fence.and_then(|fence| fence.sequence_for(stream_id));
        let max_dimension = self
            .live_observation
            .as_ref()
            .expect("live observation was checked")
            .max_dimension();
        let publication = gated_exact_window_publication(
            interactive_desktop::require_exact_window_observation_available,
            || {
                self.live_observation
                    .as_mut()
                    .expect("live observation was checked")
                    .latest_after(after_sequence)
            },
            |frame| async move {
                let (source_width, source_height) = frame.dimensions();
                let (width, height) = fit_dimensions_with_bounds(
                    source_width,
                    source_height,
                    max_dimension,
                    max_dimension,
                );
                let encoded_frame = Arc::clone(&frame);
                let data = tokio::task::spawn_blocking(move || {
                    if (source_width, source_height) == (width, height)
                        && let Some(encoded_png) = encoded_frame.encoded_png()
                    {
                        return Ok(encoded_png.to_vec());
                    }
                    let bgra = resize_bgra_if_needed(
                        encoded_frame.bgra(),
                        source_width,
                        source_height,
                        width,
                        height,
                    );
                    encode_bgra_to_png(&bgra, width, height)
                })
                .await
                .map_err(|error| {
                    ComputerUseError::new(
                        ComputerUseErrorCode::CaptureFailed,
                        format!("live frame PNG encoding task failed: {error}"),
                    )
                })??;
                Ok((frame, data, source_width, source_height, width, height))
            },
            |publication| publication,
        )
        .await;
        let (frame, data, source_width, source_height, width, height) =
            self.finish_observation_sensitive_attempt(publication)?;
        let target = self.require_observed_target_available().await?;
        let target = self
            .revalidate_observed_exact_publication_target(&target)
            .await?;
        let accessibility = json!({
            "degraded": true,
            "accessibility_available": false,
            "fallback": "live_observation",
            "window_id": target.window_id,
            "pid": target.pid,
        });
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
            capture_backend: "cua-live-wgc-latest-frame".into(),
            capture_provenance: json!({
                "backend": "cua-live-wgc-latest-frame",
                "pixels_captured": true,
                "scope": "window",
                "process_id": target.pid,
                "window_handle": target.window_id,
                "live_stream_id": stream_id,
                "live_frame_sequence": frame.sequence(),
                "post_action_sequence_fence": action_completion_sequence,
                "post_action_stream_id": action_completion_fence.map(LiveObservationFence::stream_id),
                "transition_sequence_fence": transition_fence.and_then(|fence| fence.sequence_for(stream_id)),
                "transition_stream_id": transition_fence.map(LiveObservationFence::stream_id),
                "captured_at_ms": frame.captured_at_ms(),
                "frame_age_ms": frame.age_ms(),
                "native_frame_width": source_width,
                "native_frame_height": source_height,
                "max_dimension": max_dimension,
                "frames_skipped_since_previous_observation": previous_sequence
                    .map_or(0, |sequence| frame.sequence().saturating_sub(sequence).saturating_sub(1)),
                "accessibility_available": false,
            }),
            session_id: self.session_id.clone(),
        };
        self.post_action_live_sequence_fence = None;
        self.observation_transition_live_sequence_fence = None;
        self.target = Some(target);
        self.observation = Some(observation.clone());
        self.set_banner_activity(BannerActivity::Observing);
        Ok(ComputerUseScreenshot {
            data,
            observation,
            accessibility,
        })
    }

    /// Start exact-window latest-frame prefetch, fencing prior action evidence
    /// whenever the capture producer must be replaced.
    pub async fn start_live_observation(
        &mut self,
        request: &ComputerUseLiveObservationStartRequest,
    ) -> ComputerUseResult<Value> {
        let outcome = self.ensure_live_observation(request).await?;
        Ok(attach_banner_status(outcome.state, self.banner_status()))
    }

    pub(super) async fn ensure_live_observation(
        &mut self,
        request: &ComputerUseLiveObservationStartRequest,
    ) -> ComputerUseResult<LiveObservationStartOutcome> {
        self.ensure_active()?;
        request.validate()?;
        let existing_state = self.live_observation.as_ref().map(LiveObservation::state);
        #[cfg(windows)]
        let observation_availability =
            interactive_desktop::require_exact_window_observation_available();
        #[cfg(not(windows))]
        let observation_availability = Ok(());
        let preflight = preflight_live_observation_start(
            existing_state.as_ref(),
            observation_availability,
            || self.require_observed_target_available(),
        )
        .await;
        let (disposition, target) = self.finish_observation_sensitive_attempt(preflight)?;
        if disposition == LiveObservationStartDisposition::ReuseExisting {
            self.set_banner_live_observation(true);
            return Ok(LiveObservationStartOutcome {
                state: existing_state.expect("active observation has state"),
                disposition,
            });
        }
        if let Some(observation) = self.live_observation.take() {
            let _ = observation.stop().await;
            self.set_banner_live_observation(false);
        }
        self.begin_live_observation_replacement();
        let _banner_activity = self.begin_banner_activity(BannerActivity::Observing);
        let observation = LiveObservation::start(
            self.driver.clone(),
            self.session_id.clone(),
            target.pid,
            target.window_id,
            request,
        )
        .await?;
        let state = observation.state();
        self.live_observation = Some(observation);
        self.set_banner_live_observation(true);
        Ok(LiveObservationStartOutcome { state, disposition })
    }

    pub(super) fn begin_live_observation_replacement(&mut self) {
        self.invalidate_action_observations();
    }

    #[must_use]
    pub fn live_observation_state(&self) -> Value {
        let state = self
            .live_observation
            .as_ref()
            .map_or_else(|| json!({"active": false}), LiveObservation::state);
        self.set_banner_live_observation(state["active"] == true);
        attach_banner_status(state, self.banner_status())
    }

    pub async fn stop_live_observation(&mut self) -> Value {
        self.post_action_live_sequence_fence = None;
        self.observation_transition_live_sequence_fence = None;
        let result = match self.live_observation.take() {
            Some(observation) => observation.stop().await,
            None => json!({"active": false}),
        };
        self.set_banner_live_observation(false);
        self.set_banner_activity(BannerActivity::Ready);
        attach_banner_status(result, self.banner_status())
    }

    /// Capture a native-resolution crop from the latest window observation.
    pub async fn zoom(
        &mut self,
        request: &ComputerUseZoomRequest,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        self.ensure_active()?;
        let observation = self.observation.clone().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "take a screenshot before zooming an observation region",
            )
        })?;
        validate_zoom_request(request, &observation)?;
        self.require_current_upstream_session_for_evidence()?;
        let target = self.require_observed_target_available().await?;
        if target.window_id != observation.window_handle || target.pid != observation.process_id {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "the exact target window changed after the screenshot",
            ));
        }
        self.require_observed_exact_window_observation_available()?;
        let result = self
            .call_bound_tool(
                "zoom",
                json!({
                    "pid": target.pid,
                    "window_id": target.window_id,
                    "x1": request.x1,
                    "y1": request.y1,
                    "x2": request.x2,
                    "y2": request.y2,
                }),
            )
            .await;
        self.require_observed_exact_window_observation_available()?;
        self.require_observed_target_available().await?;
        let result = self.finish_observation_sensitive_attempt(result)?;
        native_tool_result(result)
    }

    async fn capture_window_visually(
        &mut self,
        target: &WindowTarget,
        max_elements: u32,
        max_depth: u32,
    ) -> ComputerUseResult<ComputerUseScreenshot> {
        if !self.escalated {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "UIA window snapshot timed out; call escalate_session with explicit approval before using the visual fallback",
            ));
        }
        if target.is_minimized {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "UIA window snapshot timed out and the target is minimized",
            ));
        }
        let exact_capture = gated_exact_window_observation(
            interactive_desktop::require_exact_window_observation_available,
            || capture_exact_window(target.pid, target.window_id),
        )
        .await;
        let exact_capture = self.finish_observation_sensitive_attempt(exact_capture);
        let (data, capture_backend, fallback, mut capture_provenance) = match exact_capture {
            Ok(capture) => (
                capture.data,
                capture.backend,
                capture.fallback,
                json!({
                    "backend": capture.backend,
                    "pixels_captured": true,
                    "scope": "window",
                    "fallback": capture.fallback,
                    "accessibility_available": false,
                    "process_id": target.pid,
                    "window_handle": target.window_id,
                    "native_window_bounds": target.bounds,
                }),
            ),
            Err(exact_error) => {
                if !exact_capture_failure_allows_desktop_fallback(exact_error.code) {
                    return Err(exact_error);
                }
                if !target.is_on_screen || !target.is_foreground {
                    return Err(ComputerUseError::new(
                        ComputerUseErrorCode::CaptureFailed,
                        format!(
                            "exact window capture failed ({exact_error}); desktop visual fallback requires the target to be on-screen and foreground"
                        ),
                    ));
                }
                let result = gated_exact_window_observation(
                    interactive_desktop::require_desktop_observation_available,
                    || {
                        call_driver_tool(
                            &self.driver.driver,
                            "get_desktop_state",
                            json!({"session": self.session_id}).to_string(),
                            "capture CUA desktop fallback",
                        )
                    },
                )
                .await;
                let result = self.finish_observation_sensitive_attempt(result)?;
                let result =
                    self.finish_observed_tool_attempt("capture CUA desktop fallback", Ok(result))?;
                let image = result.images.first().ok_or_else(|| {
                    ComputerUseError::new(
                        ComputerUseErrorCode::CaptureFailed,
                        "CUA desktop fallback returned no screenshot",
                    )
                })?;
                let desktop = base64::engine::general_purpose::STANDARD
                    .decode(&image.data_base64)
                    .map_err(|error| {
                        ComputerUseError::new(
                            ComputerUseErrorCode::CaptureFailed,
                            error.to_string(),
                        )
                    })?;
                let (crop_bounds, window_dpi) = desktop_crop_bounds(target)?;
                let data = crop_png_to_bounds(&desktop, crop_bounds)?;
                let desktop_state = result
                    .structured_json
                    .as_deref()
                    .and_then(|json| serde_json::from_str(json).ok())
                    .unwrap_or_else(|| json!({}));
                (
                    data,
                    "cua-driver-sdk-desktop-crop",
                    "desktop_crop",
                    json!({
                        "backend": "cua-driver-sdk-desktop-crop",
                        "pixels_captured": true,
                        "scope": "window",
                        "fallback": "desktop_crop",
                        "accessibility_available": false,
                        "process_id": target.pid,
                        "window_handle": target.window_id,
                        "native_window_bounds": target.bounds,
                        "desktop_crop_bounds": crop_bounds,
                        "window_dpi": window_dpi,
                        "desktop_state": desktop_state,
                    }),
                )
            }
        };
        let (width, height) = png_dimensions(&data).ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "desktop fallback crop returned a non-PNG screenshot",
            )
        })?;
        let accessibility = self
            .visual_fallback_accessibility(target, max_elements, max_depth, fallback)
            .await;
        let target = self
            .revalidate_observed_exact_publication_target(target)
            .await?;
        let accessibility_available = accessibility["accessibility_available"] == true;
        capture_provenance["accessibility_available"] = json!(accessibility_available);
        if accessibility_available {
            capture_provenance["accessibility_backend"] = json!("windows_uia");
        }
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
            capture_backend: capture_backend.into(),
            capture_provenance,
            session_id: self.session_id.clone(),
        };
        self.target = Some(target.clone());
        self.observation = Some(observation.clone());
        self.set_banner_activity(BannerActivity::Ready);
        Ok(ComputerUseScreenshot {
            data,
            observation,
            accessibility,
        })
    }

    #[cfg(windows)]
    async fn capture_window_pixels(
        &mut self,
        target: &WindowTarget,
        route: PixelObservationRoute,
    ) -> ComputerUseResult<ComputerUseScreenshot> {
        validate_exact_window_pixel_target_state(target, true)?;
        let capture = gated_exact_window_observation(
            interactive_desktop::require_exact_window_observation_available,
            || capture_exact_window(target.pid, target.window_id),
        )
        .await;
        let capture = self.finish_observation_sensitive_attempt(capture)?;
        let (width, height) = png_dimensions(&capture.data).ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "exact-window pixel capture returned a non-PNG or truncated screenshot",
            )
        })?;
        let after = self.require_observed_target_available().await?;
        if target.bounds != capture.bounds || after.bounds != capture.bounds {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "native inventory bounds and exact pixel capture bounds do not match",
            ));
        }
        validate_exact_window_pixel_publication(
            target,
            &after,
            capture.dpi,
            capture.dpi,
            capture.generation,
            capture.generation,
        )?;
        let after = self
            .revalidate_observed_exact_publication_target(target)
            .await?;
        let mut provenance = exact_window_pixel_provenance(
            route,
            &after,
            capture.generation,
            capture.dpi,
            capture.backend,
        );
        provenance["fallback"] = json!(capture.fallback);
        let accessibility = json!({
            "accessibility_available": false,
            "degraded": route.degraded(),
            "observation_mode": route.observation_mode(),
            "fallback": capture.fallback,
            "pid": after.pid,
            "window_id": after.window_id,
        });
        let observation = ComputerUseObservation {
            observation_id: format!(
                "{}-{}",
                self.session_id,
                OBSERVATION_COUNTER.fetch_add(1, Ordering::Relaxed)
            ),
            window_handle: after.window_id,
            process_id: after.pid,
            window_title: after.title.clone(),
            width,
            height,
            source_rect: after.bounds,
            capture_backend: capture.backend.into(),
            capture_provenance: provenance,
            session_id: self.session_id.clone(),
        };
        self.target = Some(after);
        self.observation = Some(observation.clone());
        self.set_banner_activity(BannerActivity::Ready);
        Ok(ComputerUseScreenshot {
            data: capture.data,
            observation,
            accessibility,
        })
    }

    /// Read CUA's bounded semantic tree without transferring screenshot pixels.
    pub async fn accessibility_snapshot(
        &mut self,
        max_elements: u32,
        max_depth: u32,
    ) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        let _banner_activity = self.begin_banner_activity(BannerActivity::Observing);
        self.refresh_upstream_session_before_observation_if_needed()
            .await?;
        let target = self.require_observed_target_available().await?;
        self.require_observed_exact_window_observation_available()?;
        #[cfg(windows)]
        let accessibility = self
            .windows_accessibility_snapshot(&target, max_elements, max_depth)
            .await?;
        #[cfg(not(windows))]
        let accessibility = {
            let result = call_driver_tool(
                &self.driver.driver,
                "get_window_state",
                json!({
                    "window_id": target.window_id,
                    "pid": target.pid,
                    "include_screenshot": false,
                    "max_elements": bounded_snapshot_elements(max_elements),
                    "max_depth": bounded_snapshot_depth(max_depth),
                    "session": self.session_id,
                })
                .to_string(),
                "capture CUA accessibility state",
            )
            .await;
            let result =
                self.finish_observed_tool_attempt("capture CUA accessibility state", result)?;
            result
                .structured_json
                .as_deref()
                .and_then(|json| serde_json::from_str(json).ok())
                .ok_or_else(|| {
                    ComputerUseError::new(
                        ComputerUseErrorCode::CaptureFailed,
                        "CUA window state returned no structured accessibility state",
                    )
                })?
        };
        self.require_observed_exact_window_observation_available()?;
        let target = self.require_observed_target_available().await?;
        self.observation = Some(semantic_observation(
            &self.session_id,
            &target,
            &accessibility,
        ));
        self.target = Some(target);
        Ok(accessibility)
    }

    /// Verify bounded structured predicates against this exact native window.
    ///
    /// Verification is read-only and remains target-bound; the CUA driver
    /// owns predicate semantics and returns tri-state evidence.
    pub async fn verify_state(
        &mut self,
        expect: Value,
        timeout_ms: Option<u64>,
        stable_samples: Option<u64>,
        include_screenshot: bool,
    ) -> ComputerUseResult<ComputerUseVerification> {
        self.ensure_active()?;
        validate_verify_state_request(&expect, timeout_ms, stable_samples)?;
        self.require_current_upstream_session_for_evidence()?;
        let _banner_activity = self.begin_banner_activity(BannerActivity::Waiting);
        let expect = expect.as_array().expect("verify state was validated");
        let target = self.require_observed_target_available().await?;
        if include_screenshot {
            self.require_observed_exact_window_observation_available()?;
        }
        let result = self
            .call_bound_tool(
                "verify_state",
                json!({
                    "pid": target.pid,
                    "window_id": target.window_id,
                    "expect": expect,
                    "timeout_ms": timeout_ms,
                    "stable_samples": stable_samples,
                    "include_screenshot": include_screenshot,
                }),
            )
            .await?;
        if include_screenshot {
            self.require_observed_exact_window_observation_available()?;
        }
        self.require_observed_target_available().await?;
        let value = serde_json::from_str(&result.raw_json).map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA verify_state returned invalid JSON: {error}"),
            )
        })?;
        let image = result.images.first().map(|image| {
            base64::engine::general_purpose::STANDARD
                .decode(&image.data_base64)
                .map(|data| ComputerUseImage {
                    data,
                    mime_type: image.mime_type.clone(),
                })
        });
        let image = image.transpose().map_err(|error| {
            ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
        })?;
        self.set_banner_activity(BannerActivity::Ready);
        Ok(ComputerUseVerification { value, image })
    }
}
