use super::*;

pub(crate) async fn gated_desktop_observation<T, Operation, Capture>(
    availability: ComputerUseResult<()>,
    capture: Operation,
) -> ComputerUseResult<T>
where
    Operation: FnOnce() -> Capture,
    Capture: Future<Output = ComputerUseResult<T>>,
{
    availability?;
    capture().await
}

#[cfg(test)]
pub(crate) async fn gated_exact_window_observation<T, Check, Operation, Capture>(
    mut check: Check,
    operation: Operation,
) -> ComputerUseResult<T>
where
    Check: FnMut() -> ComputerUseResult<()>,
    Operation: FnOnce() -> Capture,
    Capture: Future<Output = ComputerUseResult<T>>,
{
    check()?;
    let result = operation().await?;
    check()?;
    Ok(result)
}

#[cfg(test)]
pub(crate) async fn gated_exact_window_publication<
    Captured,
    Finalized,
    Published,
    Check,
    Capture,
    CaptureOutput,
    Finalize,
    FinalizeOutput,
    Publish,
>(
    mut check: Check,
    capture: Capture,
    finalize: Finalize,
    publish: Publish,
) -> ComputerUseResult<Published>
where
    Check: FnMut() -> ComputerUseResult<()>,
    Capture: FnOnce() -> CaptureOutput,
    CaptureOutput: Future<Output = ComputerUseResult<Captured>>,
    Finalize: FnOnce(Captured) -> FinalizeOutput,
    FinalizeOutput: Future<Output = ComputerUseResult<Finalized>>,
    Publish: FnOnce(Finalized) -> Published,
{
    check()?;
    let captured = capture().await?;
    check()?;
    let finalized = finalize(captured).await?;
    check()?;
    Ok(publish(finalized))
}

pub(crate) async fn gated_cursor_operation<T, CheckInput, Operation, Output>(
    moves_cursor: bool,
    check_input: CheckInput,
    operation: Operation,
) -> ComputerUseResult<T>
where
    CheckInput: FnOnce() -> ComputerUseResult<()>,
    Operation: FnOnce() -> Output,
    Output: Future<Output = ComputerUseResult<T>>,
{
    if moves_cursor {
        check_input()?;
    }
    operation().await
}

pub(crate) fn ensure_target_available_for_action(target: &WindowTarget) -> ComputerUseResult<()> {
    if target.is_minimized {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetMinimized,
            "target_minimized: automatic_input=false; issue the explicit restore_activate window operation, then take a fresh observation before retrying",
        ));
    }
    if !target.is_on_screen {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetUnavailable,
            "target_unavailable: automatic_input=false; wait for a typed target_available event and take a fresh observation before retrying",
        ));
    }
    Ok(())
}

fn browser_tool_requires_input(name: &str) -> bool {
    matches!(
        name,
        "browser_navigate"
            | "browser_click"
            | "browser_type"
            | "browser_pointer"
            | "browser_set_input_files"
            | "browser_dialog"
    )
}

pub(crate) fn run_preinvalidated_window_mutation<T, E>(
    invalidate: impl FnOnce(),
    mutation: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    invalidate();
    mutation()
}

pub(crate) fn run_gated_preinvalidated_window_mutation<T, E>(
    gate: impl FnOnce() -> Result<(), E>,
    invalidate: impl FnOnce(),
    mutation: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    gate()?;
    run_preinvalidated_window_mutation(invalidate, mutation)
}

#[cfg(test)]
pub(crate) async fn preflight_live_observation_start<T, Revalidate, Output>(
    existing_state: Option<&Value>,
    observation_availability: ComputerUseResult<()>,
    revalidate_target: Revalidate,
) -> ComputerUseResult<(LiveObservationStartDisposition, T)>
where
    Revalidate: FnOnce() -> Output,
    Output: Future<Output = ComputerUseResult<T>>,
{
    observation_availability?;
    let target = revalidate_target().await?;
    Ok((live_observation_start_disposition(existing_state), target))
}

async fn probe_recording_state(
    driver: &ComputerUseDriver,
    session_id: &str,
) -> ComputerUseResult<Value> {
    call_recording_tool_without_refresh(
        driver,
        session_id,
        "get_recording_state",
        "probe CUA recording state",
    )
    .await
}

async fn call_recording_tool_without_refresh(
    driver: &ComputerUseDriver,
    session_id: &str,
    tool: &str,
    operation: &str,
) -> ComputerUseResult<Value> {
    let result = call_driver_tool(
        &driver.driver,
        tool,
        json!({"session": session_id}).to_string(),
        operation,
    )
    .await?;
    ensure_tool_ok(operation, &result)?;
    serde_json::from_str(&result.raw_json).map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            format!("CUA {tool} returned invalid JSON: {error}"),
        )
    })
}

impl std::fmt::Debug for ComputerUseSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ComputerUseSession")
            .field("app_name", &self.app_name)
            .field("session_id", &self.session_id)
            .field("active", &self.active)
            .field("escalated", &self.escalated)
            .finish_non_exhaustive()
    }
}

impl ComputerUseSession {
    pub(super) fn new(
        driver: ComputerUseDriver,
        scope: ComputerUseTargetScope,
        app_name: String,
        agent_name: String,
        session_id: String,
    ) -> ComputerUseResult<Self> {
        scope.validate()?;
        if session_id.trim().is_empty() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "session_id must not be empty",
            ));
        }
        let label = localized_control_label(&agent_name, &app_name);
        Ok(Self {
            driver,
            scope,
            app_name,
            agent_name,
            session_id,
            marker: ComputerUseMarker {
                visible: false,
                label,
                backend: "cua-driver-sdk",
            },
            control_banner: None,
            target: None,
            observation: None,
            live_observation: None,
            post_action_live_sequence_fence: None,
            observation_transition_live_sequence_fence: None,
            showcase: None,
            recording_active: false,
            recording_expected_video: false,
            recording_health: None,
            recording_keepalive: None,
            #[cfg(windows)]
            windows_uia: None,
            last_upstream_session_refresh: None,
            active: false,
            escalated: false,
        })
    }

    /// Start CUA's bounded window session and show its color-coded marker.
    pub async fn start(&mut self) -> ComputerUseResult<Value> {
        self.start_with_request(&ComputerUseSessionStartRequest::default())
            .await
    }

    /// Start one bounded window session with explicit bootstrap options.
    ///
    /// `activate_before` is deliberately opt-in. It restores and activates
    /// only a PID/HWND-bound target before CUA initializes its capture session,
    /// which lets minimized windows recover without exposing an unscoped
    /// native-tool call.
    pub async fn start_with_request(
        &mut self,
        request: &ComputerUseSessionStartRequest,
    ) -> ComputerUseResult<Value> {
        if self.active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "window session is already active",
            ));
        }
        request.validate_for_scope(&self.scope)?;
        let target = self.resolve_target().await?;
        let (target, activation) = if request.activate_before {
            let (target, activation) = self.bootstrap_activate(&target).await?;
            (target, Some(activation))
        } else {
            (target, None)
        };
        self.app_name = resolved_application_name(&self.app_name, &target);
        self.marker.label = localized_control_label(&self.agent_name, &self.app_name);
        self.start_upstream_session("start CUA session").await?;
        self.last_upstream_session_refresh = Some(Instant::now());
        let control_banner = match ControlBanner::start_with_motion(
            BannerTarget {
                process_id: target.pid,
                window_handle: target.window_id,
                agent_name: self.agent_name.clone(),
                application_name: self.app_name.clone(),
            },
            request.indicator_motion,
        ) {
            Ok(banner) => banner,
            Err(error) => {
                cleanup_started_session(&self.driver, &self.session_id).await;
                return Err(map_indicator_error("start visible control banner", error));
            }
        };
        self.target = Some(target.clone());
        self.control_banner = Some(control_banner);
        self.set_banner_activity(BannerActivity::Ready);
        #[cfg(windows)]
        {
            self.windows_uia = None;
        }
        self.active = true;
        self.escalated = false;
        self.marker.visible = true;
        let banner = self.banner_status();
        let mut started = json!({
            "success": true,
            "target": target,
            "marker": self.marker,
            "banner": banner,
            "cursor_theme": MOUSE_CURSOR_THEME,
            "backend": "cua-driver-sdk",
        });
        if let Some(activation) = activation {
            started["activation"] =
                attach_indicator_motion_to_activation(activation, &started["banner"]);
        }
        Ok(started)
    }

    async fn bootstrap_activate(
        &mut self,
        expected: &WindowTarget,
    ) -> ComputerUseResult<(WindowTarget, Value)> {
        self.require_observed_input_available()?;
        self.ensure_observed_target_available(expected)?;
        #[cfg(windows)]
        let activation = {
            let activation = dcc_cua_platform_windows::activate_window(
                dcc_cua_platform_windows::UiaTarget {
                    process_id: expected.pid,
                    window_handle: expected.window_id,
                },
                || windows_platform_input_gate("bootstrap_activation"),
            )
            .map_err(|error| {
                map_windows_window_mutation_error("bootstrap exact CUA window activation", error)
            });
            self.finish_observation_sensitive_attempt(activation)?;
            ComputerUseToolResult {
                value: json!({
                    "success": true,
                    "path": "windows_exact_foreground",
                }),
                text: "Activated the exact Windows PID/HWND target.".into(),
                images: Vec::new(),
                degraded: false,
            }
        };
        #[cfg(not(windows))]
        let activation = {
            let result = call_driver_tool(
                &self.driver.driver,
                "bring_to_front",
                json!({
                    "pid": expected.pid,
                    "window_id": expected.window_id,
                })
                .to_string(),
                "bootstrap exact CUA window activation",
            )
            .await
            .map_err(|error| {
                if error.message.contains("timed out") {
                    activation_completion_unknown(error)
                } else {
                    error
                }
            })?;
            ensure_tool_ok("bootstrap exact CUA window activation", &result)?;
            native_tool_result(result)?
        };
        let (target, activation) = self
            .finish_exact_activation(expected, activation, "bootstrap activation")
            .await?;
        #[cfg(windows)]
        if target.is_minimized || !target.is_on_screen {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InputFailed,
                "CUA bootstrap activation did not restore the exact target to a capturable state",
            ));
        }
        Ok((
            target.clone(),
            json!({
                "success": true,
                "target": target,
                "cua": activation.value,
                "text": activation.text,
                "degraded": activation.degraded,
            }),
        ))
    }

    async fn start_upstream_session(&self, context: &str) -> ComputerUseResult<()> {
        let result = call_driver_tool(
            &self.driver.driver,
            "start_session",
            json!({
                "session": self.session_id,
                "capture_scope": "window",
                "cursor_theme": {"theme_id": MOUSE_CURSOR_THEME, "reduced_motion": "auto"},
                "_public_session_label": self.marker.label,
            })
            .to_string(),
            context,
        )
        .await?;
        ensure_tool_ok(context, &result)?;
        enable_session_marker(&self.driver, &self.session_id, context).await
    }

    async fn refresh_upstream_session_if_needed(&mut self) -> ComputerUseResult<()> {
        if self
            .last_upstream_session_refresh
            .is_some_and(|refreshed| refreshed.elapsed() < SESSION_REFRESH_INTERVAL)
        {
            return Ok(());
        }
        self.start_upstream_session("refresh CUA session").await?;
        self.last_upstream_session_refresh = Some(Instant::now());
        Ok(())
    }

    /// Capture a fresh exact-window observation. Every action must consume it.
    pub async fn screenshot(&mut self) -> ComputerUseResult<ComputerUseScreenshot> {
        self.screenshot_with_bounds(DEFAULT_SNAPSHOT_MAX_ELEMENTS, DEFAULT_SNAPSHOT_MAX_DEPTH)
            .await
    }

    /// Capture a fresh observation with bounded semantic-tree context.
    pub async fn screenshot_with_bounds(
        &mut self,
        max_elements: u32,
        max_depth: u32,
    ) -> ComputerUseResult<ComputerUseScreenshot> {
        self.ensure_active()?;
        let _banner_activity = self.begin_banner_activity(BannerActivity::Observing);
        if self.live_observation.is_some() {
            return self.live_observation_screenshot().await;
        }
        let target = self.require_observed_target_available().await?;
        #[cfg(windows)]
        if self.windows_uia.is_some() {
            return self
                .capture_window_visually(&target, max_elements, max_depth)
                .await;
        }
        self.require_observed_exact_window_observation_available()?;
        let result = call_driver_tool(
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
        .await;
        self.require_observed_exact_window_observation_available()?;
        let target = self.require_observed_target_available().await?;
        let result = match result {
            Ok(result) if result.is_error && is_uia_snapshot_failure(&result) => {
                #[cfg(windows)]
                self.activate_windows_uia_fallback(&target);
                return self
                    .capture_window_visually(&target, max_elements, max_depth)
                    .await;
            }
            Ok(result) => self.finish_observed_tool_attempt("capture CUA window", Ok(result))?,
            Err(error)
                if is_uia_snapshot_message(&error.message)
                    || error.message.contains("capture CUA window state timed out") =>
            {
                #[cfg(windows)]
                self.activate_windows_uia_fallback(&target);
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
        let frame = self
            .live_observation
            .as_mut()
            .expect("live observation was checked")
            .latest_after(after_sequence)
            .await?;
        self.require_observed_exact_window_observation_available()?;
        let target = self.require_observed_target_available().await?;
        let (source_width, source_height) = frame.dimensions();
        let (width, height) =
            fit_dimensions_with_bounds(source_width, source_height, max_dimension, max_dimension);
        let encoded_frame = Arc::clone(&frame);
        let data = tokio::task::spawn_blocking(move || {
            let bgra = resize_bgra(
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

    /// Start exact-window latest-frame prefetch without changing the action fence.
    pub async fn start_live_observation(
        &mut self,
        request: &ComputerUseLiveObservationStartRequest,
    ) -> ComputerUseResult<Value> {
        let outcome = self.ensure_live_observation(request).await?;
        Ok(attach_banner_status(outcome.state, self.banner_status()))
    }

    async fn ensure_live_observation(
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
        self.finish_observed_input_gate(observation_availability)?;
        let target = self.require_observed_target_available().await?;
        let disposition = live_observation_start_disposition(existing_state.as_ref());
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
        self.post_action_live_sequence_fence = None;
        self.observation_transition_live_sequence_fence = None;
        self.observation = None;
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
        self.finish_observed_input_gate(
            interactive_desktop::require_exact_window_observation_available(),
        )?;
        let exact_capture = capture_exact_window(target.window_id).await;
        self.finish_observed_input_gate(
            interactive_desktop::require_exact_window_observation_available(),
        )?;
        let (data, capture_backend, fallback, mut capture_provenance) = match exact_capture {
            Ok(data) => (
                data,
                "cua-platform-windows-window",
                "exact_window",
                json!({
                    "backend": "cua-platform-windows-window",
                    "pixels_captured": true,
                    "scope": "window",
                    "fallback": "exact_window",
                    "accessibility_available": false,
                    "process_id": target.pid,
                    "window_handle": target.window_id,
                    "native_window_bounds": target.bounds,
                }),
            ),
            Err(exact_error) => {
                if !target.is_on_screen || !target.is_foreground {
                    return Err(ComputerUseError::new(
                        ComputerUseErrorCode::CaptureFailed,
                        format!(
                            "exact window capture failed ({exact_error}); desktop visual fallback requires the target to be on-screen and foreground"
                        ),
                    ));
                }
                let observation_availability = self.finish_observed_input_gate(
                    interactive_desktop::require_desktop_observation_available(),
                );
                let result = gated_desktop_observation(observation_availability, || {
                    call_driver_tool(
                        &self.driver.driver,
                        "get_desktop_state",
                        json!({"session": self.session_id}).to_string(),
                        "capture CUA desktop fallback",
                    )
                })
                .await?;
                self.finish_observed_input_gate(
                    interactive_desktop::require_desktop_observation_available(),
                )?;
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

    /// Read CUA's bounded semantic tree without transferring screenshot pixels.
    pub async fn accessibility_snapshot(
        &mut self,
        max_elements: u32,
        max_depth: u32,
    ) -> ComputerUseResult<Value> {
        self.ensure_active()?;
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

    /// Call an extension CUA tool while retaining this session's exact target.
    /// Typed action/browser/lifecycle routes remain the only path for their
    /// sensitive operations.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        validate_native_tool_request(name, &arguments)?;
        if !native_tool_allowed_in_window_session(name) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!("CUA tool {name:?} must use its dedicated typed route"),
            ));
        }
        self.ensure_active()?;
        let _banner_activity = self.begin_banner_activity(banner_activity_for_bound_tool(name));
        let target = self.require_observed_target_available().await?;
        let schema = self.driver.tool_schema(name).await?;
        let mut object = arguments.as_object().cloned().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "native CUA tool arguments must be a JSON object",
            )
        })?;
        let properties = schema["properties"].as_object().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA tool {name:?} returned an invalid input schema"),
            )
        })?;
        for reserved in ["pid", "window_id", "session"] {
            if object.remove(reserved).is_some() && !properties.contains_key(reserved) {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::InvalidAction,
                    format!("native CUA tool argument {reserved:?} is not target-bindable"),
                ));
            }
        }
        if properties.contains_key("pid") {
            object.insert("pid".into(), json!(target.pid));
        }
        if properties.contains_key("window_id") {
            object.insert("window_id".into(), json!(target.window_id));
        }
        if properties.contains_key("session") {
            object.insert("session".into(), json!(self.session_id));
        }
        if !properties.contains_key("pid")
            && !properties.contains_key("window_id")
            && !properties.contains_key("session")
        {
            return self.finish_observation_sensitive_attempt(Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                format!("CUA tool {name:?} is not bindable to an exact window session"),
            )));
        }
        self.require_observed_target_available().await?;
        self.require_observed_exact_window_observation_available()?;
        let result = call_driver_tool(
            &self.driver.driver,
            name,
            Value::Object(object).to_string(),
            &format!("call CUA {name}"),
        )
        .await;
        self.require_observed_exact_window_observation_available()?;
        self.require_observed_target_available().await?;
        let result = self.finish_observed_tool_attempt(&format!("call CUA {name}"), result)?;
        native_tool_result(result)
    }

    /// Call one of CUA's typed browser tools within this exact native window.
    ///
    /// The allow-list is deliberate: browser adapters must not turn the Core
    /// host into an arbitrary CUA command proxy. CUA still owns browser target,
    /// tab, ref, origin, and input-trust validation.
    pub async fn call_browser_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<Value> {
        const ALLOWED_TOOLS: [&str; 8] = [
            "get_browser_state",
            "browser_prepare",
            "browser_navigate",
            "browser_click",
            "browser_type",
            "browser_pointer",
            "browser_set_input_files",
            "browser_dialog",
        ];
        if !ALLOWED_TOOLS.contains(&name) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!("browser tool {name:?} is not exposed by this host"),
            ));
        }
        self.ensure_active()?;
        let _banner_activity = self.begin_banner_activity(banner_activity_for_bound_tool(name));
        let target = self.require_observed_target_available().await?;
        if browser_tool_requires_input(name) {
            self.require_observed_input_available()?;
        }
        let mints_browser_references = matches!(name, "get_browser_state" | "browser_prepare");
        let mut object = arguments.as_object().cloned().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "browser tool arguments must be a JSON object",
            )
        })?;
        object.insert("session".into(), json!(self.session_id));
        if name == "browser_prepare"
            || (name == "get_browser_state" && !object.contains_key("target_id"))
        {
            object.insert("pid".into(), json!(target.pid));
            object.insert("window_id".into(), json!(target.window_id));
        }
        let timeout = if name == "browser_prepare" {
            Duration::from_secs(60)
        } else {
            INPUT_CALL_TIMEOUT
        };
        if mints_browser_references {
            self.require_observed_exact_window_observation_available()?;
        }
        let result = call_driver_tool_with_timeout(
            &self.driver.driver,
            name,
            Value::Object(object).to_string(),
            &format!("call CUA {name}"),
            timeout,
        )
        .await;
        if mints_browser_references {
            self.require_observed_exact_window_observation_available()?;
            self.require_observed_target_available().await?;
        }
        let result = self.finish_observed_tool_attempt(&format!("call CUA {name}"), result)?;
        let value = serde_json::from_str(&result.raw_json).map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA {name} returned invalid JSON: {error}"),
            )
        })?;
        self.set_banner_activity(BannerActivity::Ready);
        Ok(value)
    }

    /// Call the one browser destructive tool through CUA's trusted adapter
    /// ingress. The approval evidence is created here, never accepted from
    /// caller JSON.
    pub async fn call_browser_download_tool(
        &mut self,
        arguments: Value,
    ) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        self.require_observed_target_available().await?;
        self.require_observed_input_available()?;
        let mut object = arguments.as_object().cloned().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "browser download arguments must be a JSON object",
            )
        })?;
        object.insert("session".into(), json!(self.session_id));
        object.insert(
            "_cua_browser_download_mcp_host_approved".into(),
            Value::Bool(true),
        );
        let result = self
            .driver
            .driver
            .call_tool_from_trusted_adapter("browser_download", Value::Object(object))
            .await
            .map_err(|error| map_driver_error("call CUA browser_download", error));
        let result = self.finish_observed_tool_attempt("call CUA browser_download", result)?;
        serde_json::from_str(&result.raw_json).map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA browser_download returned invalid JSON: {error}"),
            )
        })
    }

    /// Read clipboard types, optionally including privacy-sensitive text.
    pub async fn clipboard_read(&mut self, include_text: bool) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        self.revalidate_observed_target().await?;
        self.call_bound_tool_value("clipboard_read", json!({"include_text": include_text}))
            .await
    }

    /// Replace the clipboard with exactly one validated value.
    pub async fn clipboard_write(
        &mut self,
        request: &ComputerUseClipboardWriteRequest,
    ) -> ComputerUseResult<Value> {
        validate_clipboard_write_request(request)?;
        self.ensure_active()?;
        self.revalidate_observed_target().await?;
        self.call_bound_tool_value(
            "clipboard_write",
            serde_json::to_value(request).map_err(|error| {
                ComputerUseError::new(ComputerUseErrorCode::InvalidAction, error.to_string())
            })?,
        )
        .await
    }

    /// Start trajectory/evidence recording for this exact CUA session.
    pub async fn recording_start(
        &mut self,
        request: &ComputerUseRecordingStartRequest,
    ) -> ComputerUseResult<Value> {
        validate_recording_start_request(request)?;
        self.ensure_active()?;
        self.require_observed_target_available().await?;
        if self.recording_active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "showcase recording is already active",
            ));
        }
        let _banner_activity = self.begin_banner_activity(BannerActivity::Recording);
        let trajectory = self
            .call_bound_tool_value(
                "start_recording",
                json!({
                    "output_dir": request.output_dir,
                    "record_video": false,
                }),
            )
            .await?;
        self.recording_active = true;
        self.set_banner_recording(true);
        if !request.record_video {
            self.start_recording_keepalive(false, &trajectory);
            return Ok(trajectory);
        }

        let live_observation = match self
            .ensure_live_observation(&ComputerUseLiveObservationStartRequest {
                fps: 10,
                ..Default::default()
            })
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = self
                    .call_bound_tool_value("stop_recording", json!({}))
                    .await;
                self.recording_active = false;
                self.set_banner_recording(false);
                self.set_banner_activity(BannerActivity::Ready);
                return Err(error);
            }
        };
        let owns_live_observation =
            live_observation.disposition == LiveObservationStartDisposition::StartedNew;
        let observation = self
            .live_observation
            .as_ref()
            .expect("live observation was started");
        match ShowcaseRecorder::start(
            observation.subscribe(),
            &request.output_dir,
            observation.fps(),
        )
        .await
        {
            Ok(recorder) => {
                let video = recorder.state();
                self.showcase = Some(ActiveShowcase {
                    recorder,
                    owns_live_observation,
                });
                self.start_recording_keepalive(true, &trajectory);
                Ok(json!({"trajectory": trajectory, "video": video}))
            }
            Err(error) => {
                let _ = self
                    .call_bound_tool_value("stop_recording", json!({}))
                    .await;
                self.recording_active = false;
                if owns_live_observation {
                    self.stop_live_observation().await;
                }
                self.set_banner_recording(false);
                self.set_banner_activity(BannerActivity::Ready);
                Err(error)
            }
        }
    }

    /// Stop recording and return the finalized recording state.
    pub async fn recording_stop(&mut self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        if !self.recording_active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "showcase recording is not active",
            ));
        }
        self.stop_recording_keepalive().await;
        let _banner_activity = self.begin_banner_activity(BannerActivity::Waiting);
        let video = match self.showcase.take() {
            Some(showcase) => {
                let owns_live_observation = showcase.owns_live_observation;
                let result = showcase.recorder.stop().await.map(Some);
                if owns_live_observation {
                    self.stop_live_observation().await;
                }
                result
            }
            None => Ok(None),
        };
        // Recorder teardown is session-owned, not target-owned. Verify the
        // current owner after the keepalive exits: upstream stop_recording is
        // process-global, so a stale session must never stop a newer owner's
        // trajectory.
        let current = probe_recording_state(&self.driver, &self.session_id).await;
        let trajectory = match current {
            Ok(current) => {
                let owned = self
                    .recording_health
                    .as_ref()
                    .is_some_and(|health| health.observe_trajectory(&current));
                if owned {
                    call_recording_tool_without_refresh(
                        &self.driver,
                        &self.session_id,
                        "stop_recording",
                        "stop CUA recording",
                    )
                    .await
                } else {
                    Ok(current)
                }
            }
            Err(error) => Err(error),
        };
        if trajectory.is_ok() {
            self.recording_active = false;
            self.set_banner_recording(false);
        }
        self.set_banner_activity(BannerActivity::Ready);
        let video = video?;
        let trajectory = trajectory?;
        Ok(video.map_or(
            trajectory.clone(),
            |video| json!({"trajectory": trajectory, "video": video}),
        ))
    }

    /// Read the current recording state without exposing arbitrary CUA calls.
    pub async fn recording_state(&mut self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        // This is session-owned diagnostic state. It must remain queryable
        // when the target HWND disappears so callers can see a degraded lease
        // and stop the recording lifecycle without an unrelated target fence.
        let trajectory = probe_recording_state(&self.driver, &self.session_id).await?;
        let video = self
            .showcase
            .as_ref()
            .map(|showcase| showcase.recorder.state());
        let issues = self
            .recording_health
            .as_ref()
            .map_or_else(Vec::new, |health| {
                if self.recording_active {
                    health.observe_trajectory(&trajectory);
                    health.observe_video(video.as_ref(), self.recording_expected_video);
                }
                health.issue_names()
            });
        Ok(aggregate_recording_state(
            self.recording_active,
            self.recording_expected_video,
            &trajectory,
            video.as_ref(),
            &issues,
        ))
    }

    fn start_recording_keepalive(&mut self, expected_video: bool, trajectory: &Value) {
        debug_assert!(self.recording_keepalive.is_none());
        let health = RecordingHealth::new(self.session_id.as_str());
        let lease_is_healthy = health.observe_trajectory(trajectory);
        let driver = self.driver.clone();
        let keepalive = lease_is_healthy.then(|| {
            RecordingKeepalive::spawn(
                self.session_id.clone(),
                RECORDING_KEEPALIVE_INTERVAL,
                health.clone(),
                move |session_id| {
                    let driver = driver.clone();
                    async move { probe_recording_state(&driver, &session_id).await }
                },
            )
        });
        self.recording_expected_video = expected_video;
        self.recording_health = Some(health);
        self.recording_keepalive = keepalive;
    }

    async fn stop_recording_keepalive(&mut self) {
        if let Some(mut keepalive) = self.recording_keepalive.take() {
            keepalive.stop().await;
        }
    }

    /// Execute one scoped action through CUA after a fresh target fence.
    pub async fn perform_action(
        &mut self,
        action: &ComputerUseAction,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        self.ensure_active()?;
        let _preparing_activity = self.begin_banner_activity(banner_activity_for_action_phase(
            action,
            ActionBannerPhase::Preparing,
        ));
        validate_action(action)?;
        let observation = self.observation.clone().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "take a screenshot before performing Computer Use actions",
            )
        })?;
        if action.observation_id.as_deref() != Some(observation.observation_id.as_str()) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "action observation_id does not match the latest screenshot",
            ));
        }
        let target = self.require_observed_target_available().await?;
        if target.window_id != observation.window_handle || target.pid != observation.process_id {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "the exact target window changed after the screenshot",
            ));
        }
        self.refresh_upstream_session_if_needed().await?;
        let target = self.require_observed_target_available().await?;
        if target.window_id != observation.window_handle || target.pid != observation.process_id {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "the exact target window changed while refreshing the upstream session",
            ));
        }
        validate_action_observation(action, &observation)?;
        if let Some(reason) = explicit_input_backend_rejection(action) {
            let backend_id = action.input_backend_id.as_deref().unwrap_or_default();
            let mut result = input_backend_rejection_result(backend_id, &reason, &target);
            result.value = json!({
                "success": false,
                "action": action,
                "target": target,
                "marker": self.marker,
                "capture_provenance": observation.capture_provenance,
                "cua": result.value,
            });
            return Ok(self.complete_action(result));
        }
        #[cfg(windows)]
        if is_windows_uia_semantic_action(action, &observation) {
            self.require_observed_input_available()?;
            let fallback = self.windows_uia.as_ref().ok_or_else(|| {
                ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "take a fresh Windows UIA snapshot before performing this semantic action",
                )
            })?;
            // UIA Invoke/Toggle/Value patterns are semantic automation, not
            // physical pointer or keyboard injection. Keep the truthful
            // generic activity while the platform adapter performs them.
            let result = fallback.perform(action).await;
            let mut result = self.finish_observation_sensitive_attempt(result)?;
            result.value = json!({
                "success": true,
                "action": action,
                "target": target,
                "marker": self.marker,
                "capture_provenance": observation.capture_provenance,
                "windows_uia": result.value,
            });
            return Ok(self.complete_action(result));
        }
        self.require_observed_input_available()?;
        let fallback = observation.capture_provenance["accessibility_available"] == false;
        let visual_action;
        let effective_action = if fallback || action.input_backend_id.is_some() {
            visual_action = action_for_window_visual_fallback(action, &observation)?;
            &visual_action
        } else {
            action
        };
        let held_click = held_coordinate_click_as_drag(effective_action);
        let effective_action = held_click.as_ref().unwrap_or(effective_action);
        #[cfg(windows)]
        let target = if effective_action.delivery_mode.as_deref() == Some("foreground")
            && !target.is_foreground
        {
            let activation = dcc_cua_platform_windows::activate_window(
                dcc_cua_platform_windows::UiaTarget {
                    process_id: target.pid,
                    window_handle: target.window_id,
                },
                || windows_platform_input_gate("foreground_action_activation"),
            )
            .map_err(|error| {
                map_windows_window_mutation_error(
                    "activate the exact Windows target before pointer input",
                    error,
                )
            });
            self.finish_observation_sensitive_attempt(activation)?;
            let target = self.require_observed_target_available().await?;
            self.require_observed_input_available()?;
            if !target.is_foreground {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::InputFailed,
                    "the exact Windows target is not foreground before pointer input",
                ));
            }
            target
        } else {
            target
        };
        #[cfg(windows)]
        if fallback || effective_action.input_backend_id.is_some() {
            let fast_result = perform_windows_foreground_fast_action(
                effective_action,
                &self.session_id,
                &target,
                self.control_banner.as_ref(),
            )
            .await;
            if let Some(mut result) = self.finish_observation_sensitive_attempt(fast_result)? {
                self.set_banner_activity(BannerActivity::Operating);
                let success = result.value["success"].as_bool().unwrap_or(true);
                result.value = json!({
                    "success": success,
                    "action": action,
                    "target": target,
                    "marker": self.marker,
                    "capture_provenance": observation.capture_provenance,
                    "cua": result.value,
                });
                return Ok(self.complete_action(result));
            }
        }
        #[cfg(windows)]
        let target = {
            let target = self.require_observed_target_available().await?;
            if target.window_id != observation.window_handle || target.pid != observation.process_id
            {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "the exact target window changed before Windows input dispatch",
                ));
            }
            self.require_observed_input_available()?;
            target
        };
        #[cfg(windows)]
        if effective_action.action == "move"
            && effective_action.delivery_mode.as_deref() == Some("foreground")
        {
            let mut point = json!({"x": effective_action.x, "y": effective_action.y});
            map_window_cursor_move(
                point.as_object_mut().expect("cursor point is an object"),
                &target,
            )?;
            let x = point["x"].as_f64().expect("validated cursor x") as i32;
            let y = point["y"].as_f64().expect("validated cursor y") as i32;
            let activation = dcc_cua_platform_windows::activate_window(
                dcc_cua_platform_windows::UiaTarget {
                    process_id: target.pid,
                    window_handle: target.window_id,
                },
                || windows_platform_input_gate("foreground_cursor_move"),
            )
            .map_err(|error| {
                map_windows_window_mutation_error(
                    "validate the exact Windows target before moving the pointer",
                    error,
                )
            });
            self.finish_observation_sensitive_attempt(activation)?;
            {
                let _input_activity = self.begin_banner_activity(banner_activity_for_action_phase(
                    action,
                    ActionBannerPhase::Injecting,
                ));
                platform_windows::input::mouse::move_cursor_desktop(x, y).map_err(|error| {
                    ComputerUseError::new(
                        ComputerUseErrorCode::BackendUnavailable,
                        format!("move foreground cursor: {error}"),
                    )
                })?;
            }
            self.set_banner_activity(BannerActivity::Operating);
            return Ok(self.complete_action(ComputerUseToolResult {
                value: json!({
                    "success": true,
                    "action": action,
                    "target": target,
                    "marker": self.marker,
                    "capture_provenance": observation.capture_provenance,
                    "cua": {"scope": "window", "path": "SetCursorPos", "x": x, "y": y},
                }),
                text: format!("Moved the OS pointer inside the target window to ({x}, {y})."),
                images: Vec::new(),
                degraded: false,
            }));
        }
        let args = action_arguments(effective_action, &self.session_id, &target);
        let name = args["_tool"].as_str().unwrap_or_default().to_string();
        let mut args = args;
        args.as_object_mut()
            .expect("action arguments are an object")
            .remove("_tool");
        let result = match await_input_call(
            self.driver.driver.call_tool(name.clone(), args.to_string()),
            INPUT_CALL_TIMEOUT,
            "action",
        )
        .await
        {
            Ok(result) => result,
            Err(error) => {
                self.invalidate_local_session().await;
                return Err(error);
            }
        }
        .map_err(|error| map_driver_error(&format!("execute CUA {name}"), error));
        let result = self.finish_observation_sensitive_attempt(result)?;
        let validation = ensure_tool_ok(&format!("execute CUA {name}"), &result);
        self.finish_observation_sensitive_attempt(validation)?;
        let result = native_tool_result(result);
        let mut result = self.finish_observation_sensitive_attempt(result)?;
        result.value = json!({
            "success": true,
            "action": action,
            "target": target,
            "marker": self.marker,
            "capture_provenance": observation.capture_provenance,
            "cua": result.value,
        });
        Ok(self.complete_action(result))
    }

    fn complete_action(&mut self, mut result: ComputerUseToolResult) -> ComputerUseToolResult {
        self.post_action_live_sequence_fence = self
            .live_observation
            .as_ref()
            .and_then(LiveObservation::latest_fence);
        self.set_banner_activity(BannerActivity::Ready);
        result.value = attach_banner_status(result.value, self.banner_status());
        result
    }

    /// Invalidate action-scoped evidence without stopping live observation,
    /// showcase, or recording owners.
    pub fn invalidate_action_observations(&mut self) {
        if let Some(fence) = self
            .live_observation
            .as_ref()
            .and_then(LiveObservation::latest_fence)
        {
            self.observation_transition_live_sequence_fence = Some(fence);
        }
        self.observation = None;
        self.post_action_live_sequence_fence = None;
        #[cfg(windows)]
        {
            self.windows_uia = None;
        }
    }

    fn ensure_observed_target_available(&mut self, target: &WindowTarget) -> ComputerUseResult<()> {
        let result = ensure_target_available_for_action(target);
        self.finish_observation_sensitive_attempt(result)
    }

    pub(super) fn finish_observation_sensitive_attempt<T>(
        &mut self,
        result: ComputerUseResult<T>,
    ) -> ComputerUseResult<T> {
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

    fn finish_observed_tool_attempt(
        &mut self,
        context: &str,
        result: ComputerUseResult<cua_driver_sdk::ToolResult>,
    ) -> ComputerUseResult<cua_driver_sdk::ToolResult> {
        let result = self.finish_observation_sensitive_attempt(result)?;
        let validation = ensure_tool_ok(context, &result);
        self.finish_observation_sensitive_attempt(validation)?;
        Ok(result)
    }

    fn finish_observed_target_revalidation(
        &mut self,
        result: ComputerUseResult<WindowTarget>,
    ) -> ComputerUseResult<WindowTarget> {
        self.finish_observation_sensitive_attempt(result)
    }

    pub(super) async fn revalidate_observed_target(&mut self) -> ComputerUseResult<WindowTarget> {
        let result = self.revalidate_target().await;
        self.finish_observed_target_revalidation(result)
    }

    pub(super) async fn require_observed_target_available(
        &mut self,
    ) -> ComputerUseResult<WindowTarget> {
        let target = self.revalidate_observed_target().await?;
        self.ensure_observed_target_available(&target)?;
        Ok(target)
    }

    fn finish_observed_input_attempt<T>(
        &mut self,
        result: ComputerUseResult<T>,
    ) -> ComputerUseResult<T> {
        self.finish_observation_sensitive_attempt(result)
    }

    fn finish_observed_input_gate(
        &mut self,
        result: ComputerUseResult<()>,
    ) -> ComputerUseResult<()> {
        self.finish_observed_input_attempt(result)
    }

    pub(super) fn require_observed_input_available(&mut self) -> ComputerUseResult<()> {
        let result = interactive_desktop::require_input_available();
        self.finish_observed_input_gate(result)
    }

    fn require_observed_exact_window_observation_available(&mut self) -> ComputerUseResult<()> {
        let result = interactive_desktop::require_exact_window_observation_available();
        self.finish_observed_input_gate(result)
    }

    fn rebaseline_live_observation_transition_fence(&mut self) {
        if self.observation_transition_live_sequence_fence.is_some()
            && let Some(fence) = self
                .live_observation
                .as_ref()
                .and_then(LiveObservation::latest_fence)
        {
            self.observation_transition_live_sequence_fence = Some(fence);
        }
    }

    async fn revalidate_observed_exact_publication_target(
        &mut self,
        expected: &WindowTarget,
    ) -> ComputerUseResult<WindowTarget> {
        self.require_observed_exact_window_observation_available()?;
        let observed = self.require_observed_target_available().await?;
        if observed.pid != expected.pid || observed.window_id != expected.window_id {
            return self.finish_observation_sensitive_attempt(Err(ComputerUseError::new(
                ComputerUseErrorCode::TargetUnavailable,
                "the exact target identity changed before observation publication",
            )));
        }
        Ok(observed)
    }

    pub(super) async fn preflight_mutating_bound_tool(
        &mut self,
    ) -> ComputerUseResult<WindowTarget> {
        self.require_observed_input_available()?;
        self.require_observed_target_available().await?;
        self.refresh_upstream_session_if_needed().await?;
        let target = self.require_observed_target_available().await?;
        self.require_observed_input_available()?;
        Ok(target)
    }

    pub(super) async fn call_bound_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<cua_driver_sdk::ToolResult> {
        self.refresh_upstream_session_if_needed().await?;
        self.call_bound_tool_without_refresh(name, arguments).await
    }

    pub(super) async fn call_bound_tool_without_refresh(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<cua_driver_sdk::ToolResult> {
        let mut object = arguments.as_object().cloned().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "bound CUA tool arguments must be a JSON object",
            )
        })?;
        object.insert("session".into(), json!(self.session_id));
        let result = call_driver_tool(
            &self.driver.driver,
            name,
            Value::Object(object).to_string(),
            &format!("call CUA {name}"),
        )
        .await;
        self.finish_observed_tool_attempt(&format!("call CUA {name}"), result)
    }

    async fn call_bound_tool_value(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<Value> {
        let result = self.call_bound_tool(name, arguments).await?;
        serde_json::from_str(&result.raw_json).map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA {name} returned invalid JSON: {error}"),
            )
        })
    }

    pub async fn stop(&mut self) -> ComputerUseResult<Value> {
        if self.recording_active {
            let _ = self.recording_stop().await;
        }
        self.stop_live_observation().await;
        if !self.active {
            self.invalidate_local_session().await;
            return Ok(json!({"success": true, "active": false}));
        }
        self.set_banner_activity(BannerActivity::Stopping);
        let result = call_driver_tool(
            &self.driver.driver,
            "end_session",
            json!({"session": self.session_id}).to_string(),
            "stop CUA session",
        )
        .await;
        let result = match result {
            Ok(result) => ensure_tool_ok("stop CUA session", &result),
            Err(error) => Err(error),
        };
        self.invalidate_local_session().await;
        result?;
        Ok(json!({"success": true, "active": false, "marker": self.marker}))
    }

    async fn invalidate_local_session(&mut self) {
        // Terminal input/transport failures must not leave any local presenter,
        // capture producer, or recorder owning the target after the Host has
        // declared this session unusable. Their Drop implementations abort the
        // producer tasks; clean user-requested stops finalize them above first.
        self.stop_recording_keepalive().await;
        self.showcase.take();
        self.live_observation.take();
        self.post_action_live_sequence_fence = None;
        self.observation_transition_live_sequence_fence = None;
        self.recording_active = false;
        self.set_banner_recording(false);
        self.set_banner_live_observation(false);
        self.control_banner.take();
        self.active = false;
        self.last_upstream_session_refresh = None;
        self.marker.visible = false;
        self.target = None;
        self.observation = None;
        #[cfg(windows)]
        {
            self.windows_uia = None;
        }
    }

    /// Read CUA's live capture policy for this exact session.
    pub async fn session_state(&mut self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        let state = self
            .call_bound_tool_value("get_session_state", json!({}))
            .await?;
        Ok(attach_banner_status(state, self.banner_status()))
    }

    /// Control or inspect the visible CUA mouse marker for this session.
    pub async fn cursor_tool(&mut self, name: &str, arguments: Value) -> ComputerUseResult<Value> {
        if !cursor_tool_allowed(name) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!("cursor tool {name:?} is not exposed by this host"),
            ));
        }
        validate_native_tool_request(name, &arguments)?;
        let mut object = arguments.as_object().cloned().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "cursor tool arguments must be a JSON object",
            )
        })?;
        if object.contains_key("session") {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "cursor tool session is host-owned",
            ));
        }
        let moves_cursor = name == "move_cursor";
        if moves_cursor {
            validate_window_cursor_move(&object)?;
        }
        let enabled = if name == "set_agent_cursor_enabled" {
            Some(
                object
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| {
                        ComputerUseError::new(
                            ComputerUseErrorCode::InvalidAction,
                            "set_agent_cursor_enabled requires enabled",
                        )
                    })?,
            )
        } else {
            None
        };
        self.ensure_active()?;
        let input_availability = if moves_cursor {
            self.require_observed_input_available()
        } else {
            Ok(())
        };
        gated_cursor_operation(
            moves_cursor,
            || input_availability,
            || async move {
                let _banner_activity = self.begin_banner_activity(BannerActivity::Operating);
                let target = if moves_cursor {
                    self.preflight_mutating_bound_tool().await?
                } else {
                    self.revalidate_observed_target().await?
                };
                if moves_cursor {
                    map_window_cursor_move(&mut object, &target)?;
                }
                let result = if moves_cursor {
                    let result = self
                        .call_bound_tool_without_refresh(name, Value::Object(object))
                        .await;
                    let result = self.finish_observation_sensitive_attempt(result)?;
                    self.require_observed_target_available().await?;
                    self.require_observed_input_available()?;
                    serde_json::from_str(&result.raw_json).map_err(|error| {
                        ComputerUseError::new(
                            ComputerUseErrorCode::BackendUnavailable,
                            format!("CUA {name} returned invalid JSON: {error}"),
                        )
                    })?
                } else {
                    self.call_bound_tool_value(name, Value::Object(object))
                        .await?
                };
                if let Some(enabled) = enabled {
                    self.marker.visible = enabled;
                }
                self.set_banner_activity(BannerActivity::Ready);
                Ok(result)
            },
        )
        .await
    }

    /// Explicitly approve pixel fallback after the window accessibility ladder is exhausted.
    pub async fn escalate(
        &mut self,
        reason: &str,
        detail: Option<&str>,
    ) -> ComputerUseResult<Value> {
        validate_escalation_request(reason, detail)?;
        self.ensure_active()?;
        #[cfg(windows)]
        {
            let target = self.require_observed_target_available().await?;
            self.activate_windows_uia_fallback(&target);
        }
        #[cfg(not(windows))]
        self.require_observed_target_available().await?;
        self.escalated = true;
        Ok(json!({
            "approved": true,
            "capture_scope": "window",
            "fallback": "pixel",
            "reason": reason,
            "detail": detail,
        }))
    }

    pub async fn resume_after_user_approval(&mut self) -> ComputerUseResult<Value> {
        if self.active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "stop the active session before resuming after user approval",
            ));
        }
        self.start().await
    }

    /// Return the last CUA-validated exact target without exposing mutable state.
    pub fn target(&self) -> Option<Value> {
        self.target.as_ref().map(|target| json!(target))
    }

    pub fn banner_status(&self) -> Value {
        if self
            .live_observation
            .as_ref()
            .is_some_and(|observation| !observation.is_active())
        {
            self.set_banner_live_observation(false);
        }
        self.control_banner.as_ref().map_or_else(
            || {
                json!({
                    "backend": "unavailable",
                    "healthy": false,
                    "running": false,
                    "last_error": "control banner is not running",
                    "failure": {
                        "kind": "backend",
                        "message": "control banner is not running",
                    },
                    "visible": false,
                    "target_frame_visible": false,
                    "interrupted": false,
                    "stop_key": "Escape",
                    "activity": "stopping",
                    "activity_label": "Stopping…",
                    "recording": false,
                    "live_observation": false,
                    "placement": "unavailable",
                })
            },
            |banner| json!(banner.status()),
        )
    }

    pub fn control_banner_interrupted(&self) -> bool {
        self.control_banner
            .as_ref()
            .is_some_and(ControlBanner::interrupted)
    }

    pub fn control_banner_failure(&self) -> Option<ComputerUseError> {
        let failure = self.control_banner.as_ref()?.failure()?;
        let code = match failure.kind {
            BannerFailureKind::TargetLost => ComputerUseErrorCode::InvalidTarget,
            BannerFailureKind::Backend => ComputerUseErrorCode::BackendUnavailable,
        };
        Some(ComputerUseError::new(code, failure.message))
    }

    pub fn set_banner_activity(&self, activity: BannerActivity) {
        if let Some(banner) = &self.control_banner {
            banner.set_activity(activity);
        }
    }

    fn begin_banner_activity(&self, activity: BannerActivity) -> Option<BannerActivityGuard> {
        self.control_banner
            .as_ref()
            .map(|banner| banner.begin_activity(activity))
    }

    fn set_banner_recording(&self, recording: bool) {
        if let Some(banner) = &self.control_banner {
            banner.set_recording(recording);
        }
    }

    fn set_banner_live_observation(&self, live_observation: bool) {
        if let Some(banner) = &self.control_banner {
            banner.set_live_observation(live_observation);
        }
    }

    /// Revalidate and return the current exact-window state.
    pub async fn window_state(&mut self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        let target = self.revalidate_observed_target().await?;
        if target.is_minimized || !target.is_on_screen {
            self.invalidate_action_observations();
        }
        Ok(json!({
            "process_id": target.pid,
            "window_handle": target.window_id,
            "exists": true,
            "visible": target.is_on_screen,
            "minimized": target.is_minimized,
            "foreground": target.is_foreground,
            "bounds": target.bounds,
        }))
    }

    /// Read the exact target's availability without activating it or sending
    /// input. Windows always probes the PID/HWND bound at session start, even
    /// when discovery originally used a title.
    pub async fn target_availability(
        &mut self,
    ) -> ComputerUseResult<crate::ComputerUseTargetAvailability> {
        self.ensure_active()?;
        #[cfg(windows)]
        let target = {
            let original = self.target.as_ref().ok_or_else(|| {
                ComputerUseError::new(
                    ComputerUseErrorCode::MissingWindow,
                    "session target is missing",
                )
            })?;
            let exact_scope = ComputerUseTargetScope {
                process_id: Some(original.pid),
                window_handle: Some(original.window_id),
                window_title: None,
            };
            match crate::window_target::resolve_exact(&exact_scope) {
                Ok(Some(target))
                    if target.pid == original.pid && target.window_id == original.window_id =>
                {
                    target
                }
                _ => {
                    self.invalidate_action_observations();
                    return Ok(crate::ComputerUseTargetAvailability {
                        status: crate::ComputerUseTargetStatus::Unavailable,
                        code: "target_unavailable".into(),
                        visible: false,
                        minimized: false,
                        foreground: false,
                    });
                }
            }
        };
        #[cfg(not(windows))]
        let target = self.revalidate_observed_target().await?;
        let availability = crate::ComputerUseTargetAvailability {
            status: if target.is_minimized {
                crate::ComputerUseTargetStatus::Minimized
            } else if target.is_on_screen {
                crate::ComputerUseTargetStatus::Available
            } else {
                crate::ComputerUseTargetStatus::Unavailable
            },
            code: if target.is_minimized {
                "target_minimized"
            } else if target.is_on_screen {
                "target_available"
            } else {
                "target_unavailable"
            }
            .into(),
            visible: target.is_on_screen,
            minimized: target.is_minimized,
            foreground: target.is_foreground,
        };
        if availability.status != crate::ComputerUseTargetStatus::Available {
            self.invalidate_action_observations();
        }
        Ok(availability)
    }

    async fn finish_exact_activation(
        &self,
        expected: &WindowTarget,
        mut activation: ComputerUseToolResult,
        context: &str,
    ) -> ComputerUseResult<(WindowTarget, ComputerUseToolResult)> {
        let mut target = self.resolve_target().await?;
        if target.pid != expected.pid || target.window_id != expected.window_id {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::TargetUnavailable,
                format!("the exact target identity changed during {context}"),
            ));
        }
        #[cfg(windows)]
        if !target.is_foreground {
            dcc_cua_platform_windows::activate_window(
                dcc_cua_platform_windows::UiaTarget {
                    process_id: target.pid,
                    window_handle: target.window_id,
                },
                || windows_platform_input_gate("exact_activation_fallback"),
            )
            .map_err(|error| {
                map_windows_window_mutation_error("activate the exact Windows target", error)
            })?;
            target = self.resolve_target().await?;
            if target.pid != expected.pid || target.window_id != expected.window_id {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::TargetUnavailable,
                    format!("the exact target identity changed during {context}"),
                ));
            }
            activation.value["fallback"] = json!("windows_exact_foreground");
        }
        if !target.is_foreground {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InputFailed,
                "CUA reported activation success but the exact target is not foreground",
            ));
        }
        Ok((target, activation))
    }

    /// Activate only the exact target through CUA's scoped window action.
    pub async fn activate(&mut self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        self.require_observed_input_available()?;
        let _banner_activity = self.begin_banner_activity(BannerActivity::Navigating);
        let target = self.require_observed_target_available().await?;
        #[cfg(windows)]
        {
            let expected_pid = target.pid;
            let expected_window = target.window_id;
            let mutation_gate =
                self.finish_observed_input_gate(interactive_desktop::require_input_available());
            run_gated_preinvalidated_window_mutation(
                || mutation_gate,
                || self.invalidate_action_observations(),
                || {
                    dcc_cua_platform_windows::activate_window(
                        dcc_cua_platform_windows::UiaTarget {
                            process_id: expected_pid,
                            window_handle: expected_window,
                        },
                        || windows_platform_input_gate("explicit_activation"),
                    )
                    .map_err(|error| {
                        map_windows_window_mutation_error(
                            "activate the exact Windows target",
                            error,
                        )
                    })
                },
            )?;
            let target = self.require_observed_target_available().await?;
            self.require_observed_input_available()?;
            if target.pid != expected_pid || target.window_id != expected_window {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::TargetUnavailable,
                    "the exact target identity changed during session activation",
                ));
            }
            if !target.is_foreground {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::InputFailed,
                    "the exact Windows target is not foreground after activation; automatic_input=false; blind_retry=false",
                ));
            }
            self.target = Some(target.clone());
            self.set_banner_activity(BannerActivity::Ready);
            Ok(json!({
                "success": true,
                "target": target,
                "cua": {"path": "windows_exact_foreground"},
                "text": "Activated the exact Windows PID/HWND target.",
                "degraded": false,
                "automatic_input": false,
                "fresh_observation_required": true,
            }))
        }
        #[cfg(not(windows))]
        {
            let result = match await_input_call(
                self.driver.driver.call_tool(
                    "bring_to_front".into(),
                    json!({
                        "pid": target.pid,
                        "window_id": target.window_id,
                        "session": self.session_id,
                    })
                    .to_string(),
                ),
                INPUT_CALL_TIMEOUT,
                "window activation",
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    self.invalidate_action_observations();
                    return Err(activation_completion_unknown(error));
                }
            }
            .map_err(|error| map_driver_error("activate CUA window", error))?;
            ensure_tool_ok("activate CUA window", &result)?;
            let activation = native_tool_result(result)?;
            let (target, activation) = self
                .finish_exact_activation(&target, activation, "session activation")
                .await?;
            self.set_banner_activity(BannerActivity::Ready);
            Ok(json!({
                "success": true,
                "target": target,
                "cua": activation.value,
                "text": activation.text,
                "degraded": activation.degraded,
            }))
        }
    }

    /// Explicitly restore and activate only the exact PID/HWND bound to this
    /// session. This operation never runs implicitly from an action retry.
    pub async fn restore_activate(&mut self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        if self.scope.process_id.is_none() || self.scope.window_handle.is_none() {
            return self.finish_observation_sensitive_attempt(Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                "restore_activate requires an exact process_id and window_handle grant binding",
            )));
        }
        self.require_observed_input_available()?;
        let _banner_activity = self.begin_banner_activity(BannerActivity::Navigating);
        let expected = self.revalidate_observed_target().await?;
        #[cfg(windows)]
        let mutation_gate =
            self.finish_observed_input_gate(interactive_desktop::require_input_available());
        #[cfg(windows)]
        run_gated_preinvalidated_window_mutation(
            || mutation_gate,
            || {
                self.invalidate_action_observations();
            },
            || {
                dcc_cua_platform_windows::restore_and_activate_window(
                    dcc_cua_platform_windows::UiaTarget {
                        process_id: expected.pid,
                        window_handle: expected.window_id,
                    },
                    || {
                        windows_platform_input_gate("restore_pre_mutation")
                    },
                    || windows_platform_input_gate("activate_pre_mutation"),
                )
                .map_err(|error| {
                    let mut error = map_windows_window_mutation_error(
                        "restore and activate the exact Windows target",
                        error,
                    );
                    error.message.push_str(
                        "; automatic_input=false; blind_retry=false; fresh_observation_required=true",
                    );
                    error
                })
            },
        )?;
        #[cfg(not(windows))]
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "restore_activate is currently available only for exact Windows PID/HWND targets",
        ));

        #[cfg(windows)]
        {
            let target = self.require_observed_target_available().await?;
            self.require_observed_input_available()?;
            if target.pid != expected.pid || target.window_id != expected.window_id {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::TargetUnavailable,
                    "the exact target identity changed during restore_activate",
                ));
            }
            if !target.is_foreground {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::InputFailed,
                    "restore_activate did not leave the exact target restored, visible, and foreground; automatic_input=false",
                ));
            }
            self.set_banner_activity(BannerActivity::Ready);
            Ok(json!({
                "success": true,
                "automatic_input": false,
                "target": target,
                "fresh_observation_required": true,
            }))
        }
    }

    /// Force-terminate only the exact process bound to this session.
    ///
    /// This is intentionally separate from `stop`: stopping ends the CUA
    /// control session, while termination is a destructive application
    /// operation that requires an explicit Host grant.
    pub async fn terminate_app(&mut self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        let target = self.revalidate_observed_target().await?;
        let termination = self
            .call_bound_tool_value("kill_app", json!({"pid": target.pid}))
            .await?;
        let cleanup = self.stop().await?;
        Ok(json!({
            "success": true,
            "target": target,
            "termination": termination,
            "cleanup": cleanup,
        }))
    }

    pub fn status(&self) -> Value {
        json!({
            "active": self.active,
            "escalated": self.escalated,
            "session_id": self.session_id,
            "target": self.target,
            "banner": self.banner_status(),
            "marker": self.marker,
            "latest_observation_id": self.observation.as_ref().map(|value| &value.observation_id),
            "backend": "cua-driver-sdk",
        })
    }

    async fn resolve_target(&self) -> ComputerUseResult<WindowTarget> {
        #[cfg(windows)]
        if self.scope.window_handle.is_some() {
            let target = crate::window_target::resolve_exact(&self.scope)?.ok_or_else(|| {
                ComputerUseError::new(
                    ComputerUseErrorCode::MissingWindow,
                    "exact native window resolution is unavailable on this platform",
                )
            })?;
            validate_target_policy(&target)?;
            return Ok(target);
        }
        let rows = self.list_windows().await?;
        let matches = rows
            .into_iter()
            .filter(|row| self.scope.matches(row))
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::MissingWindow,
                format!(
                    "expected exactly one scoped window, found {}",
                    matches.len()
                ),
            ));
        }
        let target = matches.into_iter().next().expect("one match");
        validate_target_policy(&target)?;
        Ok(target)
    }

    pub(super) async fn revalidate_target(&self) -> ComputerUseResult<WindowTarget> {
        let target = self.resolve_target().await?;
        let original = self.target.as_ref().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::MissingWindow,
                "session target is missing",
            )
        })?;
        if target.pid != original.pid || target.window_id != original.window_id {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::TargetUnavailable,
                "the exact target identity changed",
            ));
        }
        Ok(target)
    }

    async fn list_windows(&self) -> ComputerUseResult<Vec<WindowTarget>> {
        let mut windows =
            list_windows_with_driver(&self.driver.driver, self.scope.process_id, false)
                .await?
                .into_iter()
                .filter_map(|value| WindowTarget::from_value(&value))
                .collect::<Vec<_>>();
        mark_foreground_window(&mut windows);
        Ok(windows)
    }

    pub(super) fn ensure_active(&self) -> ComputerUseResult<()> {
        if self.active {
            Ok(())
        } else {
            Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "Computer Use session is not active",
            ))
        }
    }
}

pub(crate) fn resolved_application_name(configured: &str, target: &WindowTarget) -> String {
    if !configured.trim().eq_ignore_ascii_case("application") && !configured.trim().is_empty() {
        return configured.trim().to_owned();
    }
    let title = target.title.trim();
    if !title.is_empty() {
        return title.to_owned();
    }
    let process = target.app_name.trim();
    process
        .strip_suffix(".exe")
        .or_else(|| process.strip_suffix(".EXE"))
        .unwrap_or(process)
        .to_owned()
}

#[cfg(test)]
mod target_transition_tests {
    use super::*;

    #[tokio::test]
    async fn target_transition_clears_action_evidence_but_keeps_a_live_freshness_fence() {
        let driver = ComputerUseDriver::create().unwrap();
        let mut session = driver
            .session(
                ComputerUseTargetScope {
                    process_id: Some(42),
                    window_handle: Some(77),
                    window_title: None,
                },
                "Test DCC",
                "session-1",
            )
            .unwrap();
        session.observation = Some(ComputerUseObservation {
            observation_id: "observation-before-transition".into(),
            window_handle: 77,
            process_id: 42,
            window_title: "Test DCC".into(),
            width: 800,
            height: 600,
            source_rect: [0, 0, 800, 600],
            capture_backend: "test".into(),
            capture_provenance: json!({"accessibility_backend": "windows_uia"}),
            session_id: "session-1".into(),
        });
        session.post_action_live_sequence_fence = Some(LiveObservationFence::new(7, 11));
        session.live_observation = Some(LiveObservation::from_test_frame(7, 12));
        session.recording_active = true;
        session.recording_expected_video = true;
        #[cfg(windows)]
        {
            session.windows_uia = Some(WindowsUiaFallback::new(42, 77));
        }

        session.invalidate_action_observations();

        assert!(session.observation.is_none());
        assert!(session.post_action_live_sequence_fence.is_none());
        assert_eq!(
            session.observation_transition_live_sequence_fence,
            Some(LiveObservationFence::new(7, 12))
        );
        #[cfg(windows)]
        assert!(session.windows_uia.is_none());
        assert!(session.recording_active);
        assert!(session.recording_expected_video);
        assert!(session.live_observation.is_some());
        assert!(session.showcase.is_none());
    }

    #[tokio::test]
    async fn unavailable_input_gate_makes_direct_core_action_evidence_stale_after_resume() {
        let driver = ComputerUseDriver::create().unwrap();
        let mut session = driver
            .session(
                ComputerUseTargetScope {
                    process_id: Some(42),
                    window_handle: Some(77),
                    window_title: None,
                },
                "Test DCC",
                "session-1",
            )
            .unwrap();
        session.active = true;
        session.observation = Some(ComputerUseObservation {
            observation_id: "observation-before-lock".into(),
            window_handle: 77,
            process_id: 42,
            window_title: "Test DCC".into(),
            width: 800,
            height: 600,
            source_rect: [0, 0, 800, 600],
            capture_backend: "test".into(),
            capture_provenance: json!({"accessibility_backend": "windows_uia"}),
            session_id: "session-1".into(),
        });
        session.post_action_live_sequence_fence = Some(LiveObservationFence::new(7, 11));
        session.recording_active = true;
        session.recording_expected_video = true;
        #[cfg(windows)]
        {
            session.windows_uia = Some(WindowsUiaFallback::new(42, 77));
        }

        let error = session
            .finish_observed_input_gate(Err(ComputerUseError::new(
                ComputerUseErrorCode::InteractiveDesktopUnavailable,
                "workstation locked",
            )))
            .unwrap_err();

        assert_eq!(
            error.code,
            ComputerUseErrorCode::InteractiveDesktopUnavailable
        );
        assert!(session.observation.is_none());
        assert!(session.post_action_live_sequence_fence.is_none());
        #[cfg(windows)]
        assert!(session.windows_uia.is_none());
        assert!(session.recording_active);
        assert!(session.recording_expected_video);
        assert!(session.live_observation.is_none());
        assert!(session.showcase.is_none());

        let retry = session
            .perform_action(&ComputerUseAction {
                action: "click".into(),
                observation_id: Some("observation-before-lock".into()),
                x: Some(10.0),
                y: Some(10.0),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(retry.code, ComputerUseErrorCode::StaleObservation);
    }

    #[tokio::test]
    async fn target_revalidation_failure_makes_direct_core_action_evidence_stale_after_recovery() {
        let driver = ComputerUseDriver::create().unwrap();
        let mut session = driver
            .session(
                ComputerUseTargetScope {
                    process_id: Some(42),
                    window_handle: Some(77),
                    window_title: None,
                },
                "Test DCC",
                "session-1",
            )
            .unwrap();
        session.active = true;
        session.observation = Some(ComputerUseObservation {
            observation_id: "observation-before-target-loss".into(),
            window_handle: 77,
            process_id: 42,
            window_title: "Test DCC".into(),
            width: 800,
            height: 600,
            source_rect: [0, 0, 800, 600],
            capture_backend: "test".into(),
            capture_provenance: json!({"accessibility_backend": "windows_uia"}),
            session_id: "session-1".into(),
        });
        session.post_action_live_sequence_fence = Some(LiveObservationFence::new(7, 11));
        session.recording_active = true;
        session.recording_expected_video = true;
        #[cfg(windows)]
        {
            session.windows_uia = Some(WindowsUiaFallback::new(42, 77));
        }

        let error = session
            .finish_observed_target_revalidation(Err(ComputerUseError::new(
                ComputerUseErrorCode::TargetUnavailable,
                "exact target identity changed",
            )))
            .unwrap_err();

        assert_eq!(error.code, ComputerUseErrorCode::TargetUnavailable);
        assert!(session.observation.is_none());
        assert!(session.post_action_live_sequence_fence.is_none());
        #[cfg(windows)]
        assert!(session.windows_uia.is_none());
        assert!(session.recording_active);
        assert!(session.recording_expected_video);

        let retry = session
            .perform_action(&ComputerUseAction {
                action: "click".into(),
                observation_id: Some("observation-before-target-loss".into()),
                x: Some(10.0),
                y: Some(10.0),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(retry.code, ComputerUseErrorCode::StaleObservation);
    }

    #[tokio::test]
    async fn direct_core_resume_rebaselines_past_frames_cached_while_input_was_suspended() {
        let driver = ComputerUseDriver::create().unwrap();
        let mut session = driver
            .session(
                ComputerUseTargetScope {
                    process_id: Some(42),
                    window_handle: Some(77),
                    window_title: None,
                },
                "Test DCC",
                "session-1",
            )
            .unwrap();
        let (live_observation, publisher) = LiveObservation::from_test_stream(7, 10);
        session.live_observation = Some(live_observation);

        session
            .finish_observed_input_gate(Err(ComputerUseError::new(
                ComputerUseErrorCode::InteractiveDesktopUnavailable,
                "workstation locked",
            )))
            .unwrap_err();
        publisher.publish_frame(20, "suspended_capture");

        session.finish_observed_input_gate(Ok(())).unwrap();
        session.rebaseline_live_observation_transition_fence();
        assert_eq!(
            session.observation_transition_live_sequence_fence,
            Some(LiveObservationFence::new(7, 20))
        );
        let after_sequence = observation_sequence_fence(
            7,
            None,
            None,
            session.observation_transition_live_sequence_fence,
        );
        publisher.publish_frame(21, "resumed_capture");
        let frame = session
            .live_observation
            .as_mut()
            .unwrap()
            .latest_after(after_sequence)
            .await
            .unwrap();

        assert_eq!(frame.sequence(), 21);
    }

    #[test]
    fn browser_routes_that_can_change_page_state_require_interactive_input() {
        for tool in [
            "browser_navigate",
            "browser_click",
            "browser_type",
            "browser_pointer",
            "browser_set_input_files",
            "browser_dialog",
        ] {
            assert!(browser_tool_requires_input(tool), "{tool}");
        }
        assert!(!browser_tool_requires_input("get_browser_state"));
        assert!(!browser_tool_requires_input("browser_prepare"));
    }
}
