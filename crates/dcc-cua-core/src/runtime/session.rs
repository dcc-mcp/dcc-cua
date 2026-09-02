use super::*;
mod gates;
#[cfg(any(windows, test))]
use gates::run_gated_preinvalidated_window_mutation;
use gates::{BrowserToolDisposition, browser_tool_requires_input, browser_tool_route};
pub(crate) use gates::{
    ensure_target_available_for_action, ensure_target_available_for_bootstrap_activation,
    gated_cursor_operation, gated_exact_window_observation, gated_exact_window_publication,
    gated_upstream_session_refresh, preflight_live_observation_start,
};
mod browser;
mod error_contracts;
mod observation;
mod pixel_start;
#[cfg(test)]
mod tests;
use error_contracts::*;

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

#[cfg(windows)]
fn windows_fast_preflight_rejection(
    action: &ComputerUseAction,
    target: &WindowTarget,
) -> Option<ComputerUseToolResult> {
    if action.action != "drag" || !uses_windows_foreground_fast_path(action) {
        return None;
    }
    select_windows_foreground_drag_backend(action)
        .err()
        .map(|reason| {
            let backend_id = action.input_backend_id.as_deref().unwrap_or_default();
            input_backend_rejection_result(backend_id, &reason, target)
        })
}

impl ComputerUseSession {
    /// Return whether this exact-window session was started through the
    /// provider-free pixels-only route.
    ///
    /// The host uses this bit to keep post-action evidence on the same
    /// capture route.  In particular, a pixels-only session must not
    /// accidentally fall back to UIA when `capture_after` is requested.
    #[must_use]
    pub fn is_pixels_only(&self) -> bool {
        #[cfg(windows)]
        {
            self.pixel_observation_route == Some(PixelObservationRoute::ExplicitPixelsOnly)
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// Return the monotonic receipt for all action-scoped evidence.
    #[must_use]
    pub const fn action_evidence_epoch(&self) -> ActionEvidenceEpoch {
        self.action_evidence_epoch
    }

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
            action_evidence_epoch: ActionEvidenceEpoch::default(),
            live_observation: None,
            post_action_live_sequence_fence: None,
            observation_transition_live_sequence_fence: None,
            showcase: None,
            last_recording_video: None,
            recording_active: false,
            recording_expected_video: false,
            recording_health: None,
            recording_keepalive: None,
            #[cfg(windows)]
            windows_uia: None,
            upstream_session_state: UpstreamSessionState::Inactive,
            last_upstream_session_refresh: None,
            active: false,
            escalated: false,
            uia_timeout_escalated: false,
            pixel_observation_route: None,
            #[cfg(feature = "test-support")]
            synthetic_test_session: false,
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
        self.pixel_observation_route = None;
        request.validate_for_scope(&self.scope)?;
        let target = self.resolve_target().await?;
        let (target, activation) = if request.activate_before {
            let (target, activation) = self.bootstrap_activate(&target).await?;
            (target, Some(activation))
        } else {
            (target, None)
        };
        self.upstream_session_state = match self.start_upstream_session("start CUA session").await {
            Ok(()) => {
                self.last_upstream_session_refresh = Some(Instant::now());
                UpstreamSessionState::Active
            }
            Err(error) => {
                #[cfg(windows)]
                if let Some(reason) = visual_only_start_degradation(&error) {
                    self.last_upstream_session_refresh = None;
                    UpstreamSessionState::VisualOnly { reason }
                } else {
                    return Err(error);
                }
                #[cfg(not(windows))]
                return Err(error);
            }
        };
        self.finish_started_session(target, request, activation)
            .await
    }

    async fn bootstrap_activate(
        &mut self,
        expected: &WindowTarget,
    ) -> ComputerUseResult<(WindowTarget, Value)> {
        self.require_observed_window_activation_available()?;
        let availability = ensure_target_available_for_bootstrap_activation(expected);
        self.finish_observation_sensitive_attempt(availability)?;
        #[cfg(windows)]
        let activation = {
            let activation = dcc_cua_platform_windows::activate_window(
                dcc_cua_platform_windows::UiaTarget {
                    process_id: expected.pid,
                    window_handle: expected.window_id,
                },
                || windows_platform_window_activation_gate("bootstrap_activation"),
            )
            .map_err(|error| {
                map_windows_window_mutation_error("bootstrap exact CUA window activation", error)
            });
            self.finish_observation_sensitive_attempt(activation)?;
            ComputerUseToolResult {
                status: ComputerUseToolStatus::Succeeded,
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
                if error
                    .details
                    .as_ref()
                    .is_some_and(|details| details.timed_out == Some(true))
                {
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

    async fn refresh_upstream_session_before_state_if_needed(&mut self) -> ComputerUseResult<()> {
        if !matches!(self.upstream_session_state, UpstreamSessionState::Active) {
            return Ok(());
        }
        if !self.upstream_session_refresh_due() {
            return Ok(());
        }
        let input_availability = self.require_observed_input_available();
        let result = gated_upstream_session_refresh(input_availability, || {
            self.start_upstream_session("refresh CUA session before state read")
        })
        .await;
        self.finish_upstream_refresh_attempt(result)?;
        self.complete_upstream_session_refresh();
        Ok(())
    }

    async fn refresh_upstream_session_before_observation_if_needed(
        &mut self,
    ) -> ComputerUseResult<()> {
        if !matches!(self.upstream_session_state, UpstreamSessionState::Active) {
            return Ok(());
        }
        if !self.upstream_session_refresh_due() {
            return Ok(());
        }
        let input_availability = self.require_observed_input_available();
        let result = gated_upstream_session_refresh(input_availability, || {
            self.start_upstream_session("refresh CUA session before observation")
        })
        .await;
        self.finish_upstream_refresh_attempt(result)?;
        self.complete_upstream_session_refresh();
        Ok(())
    }

    fn complete_upstream_session_refresh(&mut self) {
        self.last_upstream_session_refresh = Some(Instant::now());
        self.invalidate_action_observations();
    }

    fn upstream_session_refresh_due(&self) -> bool {
        self.last_upstream_session_refresh
            .is_none_or(|refreshed| refreshed.elapsed() >= SESSION_REFRESH_INTERVAL)
    }

    fn require_current_upstream_session_for_evidence(&mut self) -> ComputerUseResult<()> {
        if !matches!(self.upstream_session_state, UpstreamSessionState::Active) {
            self.invalidate_action_observations();
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "the upstream semantic session is unavailable; this session is restricted to explicitly approved exact-window visual fallback",
            ));
        }
        if !self.upstream_session_refresh_due() {
            return Ok(());
        }
        self.invalidate_action_observations();
        Err(ComputerUseError::new(
            ComputerUseErrorCode::SessionRefreshRequired,
            "the upstream session requires a fresh exact-target refresh",
        )
        .with_details(ComputerUseErrorDetails {
            phase: Some(ComputerUseErrorPhase::PreDispatch),
            action_attempted: Some(false),
            input_sent: Some(ComputerUseInputState::NotSent),
            completion: Some(ComputerUseCompletionState::Known),
            local_session_invalidated: Some(false),
            session_remains_active: Some(true),
            automatic_input: Some(false),
            blind_retry: Some(false),
            fresh_observation_required: Some(true),
            exact_target_revalidation_required: Some(true),
            ..Default::default()
        }))
    }

    fn finish_upstream_refresh_attempt<T>(
        &mut self,
        result: ComputerUseResult<T>,
    ) -> ComputerUseResult<T> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                self.invalidate_action_observations();
                if error.code != ComputerUseErrorCode::InputFailed
                    || error
                        .details
                        .as_ref()
                        .is_none_or(|details| details.timed_out != Some(true))
                {
                    return Err(error);
                }
                Err(
                    ComputerUseError::new(ComputerUseErrorCode::CompletionUnknown, error.message)
                        .with_details(ComputerUseErrorDetails {
                            timed_out: Some(true),
                            phase: Some(ComputerUseErrorPhase::UpstreamSessionRefresh),
                            action_attempted: Some(false),
                            input_sent: Some(ComputerUseInputState::NotSent),
                            completion: Some(ComputerUseCompletionState::Unknown),
                            local_session_invalidated: Some(false),
                            session_remains_active: Some(true),
                            automatic_input: Some(false),
                            blind_retry: Some(false),
                            fresh_observation_required: Some(true),
                            exact_target_revalidation_required: Some(true),
                            ..Default::default()
                        }),
                )
            }
        }
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
        self.require_current_upstream_session_for_evidence()?;
        self.require_observed_input_available()?;
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
        let context = format!("call CUA {name}");
        let result = await_input_call(
            self.driver
                .driver
                .call_tool(name.to_owned(), Value::Object(object).to_string()),
            INPUT_CALL_TIMEOUT,
            &context,
        )
        .await;
        let result = self.finish_typed_dispatch_result(&context, result).await;
        let result = self.finish_observed_tool_attempt(&context, result)?;
        self.require_observed_exact_window_observation_available()?;
        self.require_observed_target_available().await?;
        native_tool_result(result)
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
        self.recording_start_after_target_validation(request).await
    }

    async fn recording_start_after_target_validation(
        &mut self,
        request: &ComputerUseRecordingStartRequest,
    ) -> ComputerUseResult<Value> {
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
            self.last_recording_video.take();
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
            observation.subscribe_showcase(),
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
                self.last_recording_video.take();
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
                Err(map_showcase_error(error))
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
                let result = showcase
                    .recorder
                    .stop()
                    .await
                    .map_err(map_showcase_error)
                    .map(Some);
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
        if let Some(video) = video.as_ref() {
            self.last_recording_video = Some(RecordingVideoTerminalEvidence::try_from_finalized(
                video.clone(),
            )?);
        }
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
        let active_video = self
            .showcase
            .as_ref()
            .map(|showcase| showcase.recorder.state());
        let video = active_video.as_ref().or_else(|| {
            self.last_recording_video
                .as_ref()
                .map(RecordingVideoTerminalEvidence::state)
        });
        let issues = self
            .recording_health
            .as_ref()
            .map_or_else(Vec::new, |health| {
                if self.recording_active {
                    health.observe_trajectory(&trajectory);
                    health.observe_video(video, self.recording_expected_video);
                }
                health.issue_names()
            });
        Ok(aggregate_recording_state(
            self.recording_active,
            self.recording_expected_video,
            &trajectory,
            video,
            &issues,
        ))
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
        validate_window_action_coordinates(action)?;
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
        if self.action_requires_current_upstream_evidence(action, &observation) {
            self.require_current_upstream_session_for_evidence()?;
        }
        let target = self.require_observed_target_available().await?;
        if target.window_id != observation.window_handle || target.pid != observation.process_id {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "the exact target window changed after the screenshot",
            ));
        }
        self.reject_owned_modal_takeover(&target)?;
        if action_requires_physical_input_desktop(action, &observation) {
            self.require_observed_input_available()?;
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
            let result = self.finish_local_mutation_attempt(result);
            let mut result = self.finish_observation_sensitive_attempt(result)?;
            result.value = json!({
                "success": true,
                "action": action,
                "target": target,
                "marker": self.marker,
                "capture_provenance": observation.capture_provenance,
                "windows_uia": result.value,
            });
            return Ok(self.complete_mutating_action(result));
        }
        self.require_observed_input_available()?;
        let fallback = observation.capture_provenance["accessibility_available"] == false;
        let visual_action;
        let effective_action = if fallback || Self::action_uses_local_window_coordinates(action) {
            visual_action = action_for_window_visual_fallback(action, &observation)?;
            &visual_action
        } else {
            action
        };
        let held_click = held_coordinate_click_as_drag(effective_action);
        let effective_action = held_click.as_ref().unwrap_or(effective_action);
        #[cfg(windows)]
        let uses_local_windows_fast_path = fallback
            || effective_action.input_backend_id.is_some()
            || uses_windows_local_foreground_path(effective_action);
        #[cfg(windows)]
        if uses_local_windows_fast_path
            && let Some(mut result) = windows_fast_preflight_rejection(effective_action, &target)
        {
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
        let target = if effective_action.delivery_mode.as_deref() == Some("foreground")
            && !target.is_foreground
        {
            let activation_preflight = self.require_observed_input_available();
            self.run_gated_implicit_activation_attempt(activation_preflight, || {
                dcc_cua_platform_windows::activate_window(
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
                })
            })?;
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
        if uses_local_windows_fast_path {
            let fast_result = perform_windows_foreground_fast_action(
                effective_action,
                &self.session_id,
                &target,
                self.control_banner.as_ref(),
            )
            .await;
            let fast_result = self.finish_local_mutation_attempt(fast_result);
            if let Some(mut result) = self.finish_observation_sensitive_attempt(fast_result)? {
                self.set_banner_activity(BannerActivity::Operating);
                let success = result.status == ComputerUseToolStatus::Succeeded;
                result.value = json!({
                    "success": success,
                    "action": action,
                    "target": target,
                    "marker": self.marker,
                    "capture_provenance": observation.capture_provenance,
                    "cua": result.value,
                });
                return Ok(self.complete_attempted_fast_action(result));
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
            let activation_preflight = self.require_observed_input_available();
            self.run_gated_implicit_activation_attempt(activation_preflight, || {
                dcc_cua_platform_windows::activate_window(
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
                })
            })?;
            {
                let _input_activity = self.begin_banner_activity(banner_activity_for_action_phase(
                    action,
                    ActionBannerPhase::Injecting,
                ));
                let result = dcc_cua_platform_windows::move_cursor_desktop(x, y).map_err(|error| {
                    ComputerUseError::new(
                        ComputerUseErrorCode::BackendUnavailable,
                        format!("move foreground cursor: {error}"),
                    )
                });
                self.finish_local_mutation_attempt(result)?;
            }
            self.set_banner_activity(BannerActivity::Operating);
            return Ok(self.complete_mutating_action(ComputerUseToolResult {
                status: ComputerUseToolStatus::Succeeded,
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
        self.require_current_upstream_session_for_evidence()?;
        let command = action_arguments(effective_action, &self.session_id, &target)?;
        let name = command.tool;
        let result = await_input_call(
            self.driver
                .driver
                .call_tool(name.to_owned(), command.arguments.to_string()),
            INPUT_CALL_TIMEOUT,
            "action",
        )
        .await;
        let result = self
            .finish_typed_dispatch_result(&format!("execute CUA {name}"), result)
            .await;
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

    #[cfg(any(windows, test))]
    fn complete_mutating_action(&mut self, result: ComputerUseToolResult) -> ComputerUseToolResult {
        self.invalidate_action_observations();
        self.complete_action(result)
    }

    #[cfg(windows)]
    fn complete_attempted_fast_action(
        &mut self,
        result: ComputerUseToolResult,
    ) -> ComputerUseToolResult {
        self.complete_mutating_action(result)
    }

    #[cfg(any(windows, test))]
    fn run_gated_implicit_activation_attempt<T>(
        &mut self,
        preflight: ComputerUseResult<()>,
        activation: impl FnOnce() -> ComputerUseResult<T>,
    ) -> ComputerUseResult<T> {
        run_gated_preinvalidated_window_mutation(
            || preflight,
            || self.invalidate_action_observations(),
            || activation().map_err(local_activation_attempt_failure),
        )
    }

    #[cfg(any(windows, test))]
    fn finish_local_mutation_attempt<T>(
        &mut self,
        result: ComputerUseResult<T>,
    ) -> ComputerUseResult<T> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                self.invalidate_action_observations();
                Err(local_mutation_attempt_failure(error))
            }
        }
    }

    /// Invalidate action-scoped evidence without stopping live observation,
    /// showcase, or recording owners.
    pub fn invalidate_action_observations(&mut self) {
        self.action_evidence_epoch = self.action_evidence_epoch.advanced();
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
        #[cfg(feature = "test-support")]
        if self.synthetic_test_session {
            return Ok(());
        }
        let result = ensure_target_available_for_action(target);
        self.finish_observation_sensitive_attempt(result)
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

    async fn finish_typed_dispatch_result<T>(
        &mut self,
        context: &str,
        result: ComputerUseResult<Result<T, cua_driver_sdk::DriverError>>,
    ) -> ComputerUseResult<T> {
        match result {
            Ok(Ok(value)) => {
                self.invalidate_action_observations();
                Ok(value)
            }
            Ok(Err(cua_driver_sdk::DriverError::ActionInterrupted {
                completion: cua_driver_sdk::worker::ActionCompletion::Unknown,
                reason,
            })) => {
                let error = ComputerUseError::new(
                    ComputerUseErrorCode::InputFailed,
                    format!("{context}: {reason}"),
                );
                self.invalidate_local_session().await;
                Err(action_dispatch_completion_unknown(error))
            }
            Ok(Err(cua_driver_sdk::DriverError::ActionInterrupted {
                completion: cua_driver_sdk::worker::ActionCompletion::NotStarted,
                reason,
            })) => Err(pre_dispatch_failure(ComputerUseError::new(
                ComputerUseErrorCode::InputFailed,
                format!("{context}: {reason}"),
            ))),
            Ok(Err(cua_driver_sdk::DriverError::ActionInterrupted {
                completion: cua_driver_sdk::worker::ActionCompletion::Completed,
                reason,
            })) => {
                self.invalidate_action_observations();
                Err(ComputerUseError::new(
                    ComputerUseErrorCode::InputFailed,
                    format!("{context}: {reason}"),
                )
                .with_details(ComputerUseErrorDetails {
                    phase: Some(ComputerUseErrorPhase::ActionDispatch),
                    action_attempted: Some(true),
                    input_sent: Some(ComputerUseInputState::Unknown),
                    completion: Some(ComputerUseCompletionState::Known),
                    local_session_invalidated: Some(false),
                    session_remains_active: Some(true),
                    automatic_input: Some(false),
                    blind_retry: Some(false),
                    fresh_observation_required: Some(true),
                    ..Default::default()
                }))
            }
            Ok(Err(error @ cua_driver_sdk::DriverError::Tool { .. })) => {
                self.invalidate_action_observations();
                Err(mutation_known_failure(context, error))
            }
            Ok(Err(
                error @ (cua_driver_sdk::DriverError::Transport { .. }
                | cua_driver_sdk::DriverError::Protocol { .. }
                | cua_driver_sdk::DriverError::Worker { .. }
                | cua_driver_sdk::DriverError::Remote { .. }),
            )) => {
                let error = map_driver_error(context, error);
                self.invalidate_local_session().await;
                Err(action_dispatch_completion_unknown(error))
            }
            Ok(Err(
                error @ (cua_driver_sdk::DriverError::Configuration { .. }
                | cua_driver_sdk::DriverError::InvalidArguments { .. }
                | cua_driver_sdk::DriverError::Shutdown
                | cua_driver_sdk::DriverError::RuntimeAlreadyExists),
            )) => Err(mutation_pre_dispatch_failure(context, error)),
            Err(error) => {
                self.invalidate_local_session().await;
                Err(action_dispatch_completion_unknown(error))
            }
        }
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
        #[cfg(feature = "test-support")]
        if self.synthetic_test_session {
            return Ok(());
        }
        let result = interactive_desktop::require_input_available();
        self.finish_observed_input_gate(result)
    }

    pub(super) fn require_observed_window_activation_available(&mut self) -> ComputerUseResult<()> {
        let result = interactive_desktop::require_window_activation_available();
        self.finish_observed_input_gate(result)
    }

    fn require_observed_exact_window_observation_available(&mut self) -> ComputerUseResult<()> {
        #[cfg(feature = "test-support")]
        if self.synthetic_test_session {
            return Ok(());
        }
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
        self.require_current_upstream_session_for_evidence()?;
        self.require_observed_input_available()?;
        let target = self.require_observed_target_available().await?;
        self.require_observed_input_available()?;
        Ok(target)
    }

    pub(super) async fn call_bound_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<cua_driver_sdk::ToolResult> {
        self.require_current_upstream_session_for_evidence()?;
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

    async fn call_mutating_bound_tool_without_refresh(
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
        let context = format!("call CUA {name}");
        let result = await_input_call(
            self.driver
                .driver
                .call_tool(name.to_owned(), Value::Object(object).to_string()),
            INPUT_CALL_TIMEOUT,
            &context,
        )
        .await;
        let result = self.finish_typed_dispatch_result(&context, result).await;
        self.finish_observed_tool_attempt(&context, result)
    }

    async fn call_bound_tool_value(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<Value> {
        let result = self.call_bound_tool(name, arguments).await?;
        parse_bound_tool_value(name, &result)
    }

    async fn call_bound_tool_value_without_refresh(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<Value> {
        let result = self
            .call_bound_tool_without_refresh(name, arguments)
            .await?;
        parse_bound_tool_value(name, &result)
    }

    pub async fn stop(&mut self) -> ComputerUseResult<ComputerUseSessionStopResult> {
        let mut cleanup_issues = Vec::new();
        if self.recording_active
            && let Err(error) = self.recording_stop().await
        {
            cleanup_issues.push(ComputerUseCleanupIssue::from_error(
                ComputerUseCleanupPhase::RecordingStop,
                error,
            ));
        }
        self.stop_live_observation().await;
        if !self.active {
            self.invalidate_local_session().await;
            return Ok(ComputerUseSessionStopResult::completed(
                self.marker.clone(),
                cleanup_issues,
            ));
        }
        self.set_banner_activity(BannerActivity::Stopping);
        let result = if self.target.is_some()
            && matches!(self.upstream_session_state, UpstreamSessionState::Active)
        {
            match call_driver_tool(
                &self.driver.driver,
                "end_session",
                json!({"session": self.session_id}).to_string(),
                "stop CUA session",
            )
            .await
            {
                Ok(result) => ensure_tool_ok("stop CUA session", &result),
                Err(error) => Err(error),
            }
        } else {
            Ok(())
        };
        self.invalidate_local_session().await;
        result?;
        Ok(ComputerUseSessionStopResult::completed(
            self.marker.clone(),
            cleanup_issues,
        ))
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
        self.upstream_session_state = UpstreamSessionState::Inactive;
        self.pixel_observation_route = None;
        self.last_upstream_session_refresh = None;
        self.marker.visible = false;
        self.target = None;
        self.invalidate_action_observations();
    }

    /// Read CUA's live capture policy for this exact session.
    pub async fn session_state(&mut self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        self.refresh_upstream_session_before_state_if_needed()
            .await?;
        let state = self
            .call_bound_tool_value_without_refresh("get_session_state", json!({}))
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
                        .call_mutating_bound_tool_without_refresh(name, Value::Object(object))
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
        self.uia_timeout_escalated = reason == "uia_timeout";
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
            BannerFailureKind::Rendering => return None,
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
        activation: ComputerUseToolResult,
        context: &str,
    ) -> ComputerUseResult<(WindowTarget, ComputerUseToolResult)> {
        let target = self.resolve_target().await?;
        if target.pid != expected.pid || target.window_id != expected.window_id {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::TargetUnavailable,
                format!("the exact target identity changed during {context}"),
            ));
        }
        #[cfg(windows)]
        let (target, activation) = {
            let mut target = target;
            let mut activation = activation;
            if !target.is_foreground {
                dcc_cua_platform_windows::activate_window(
                    dcc_cua_platform_windows::UiaTarget {
                        process_id: target.pid,
                        window_handle: target.window_id,
                    },
                    || windows_platform_window_activation_gate("exact_activation_fallback"),
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
            (target, activation)
        };
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
        self.require_observed_window_activation_available()?;
        let _banner_activity = self.begin_banner_activity(BannerActivity::Navigating);
        let target = self.require_observed_target_available().await?;
        #[cfg(windows)]
        {
            let expected_pid = target.pid;
            let expected_window = target.window_id;
            let mutation_gate = self.finish_observed_input_gate(
                interactive_desktop::require_window_activation_available(),
            );
            run_gated_preinvalidated_window_mutation(
                || mutation_gate,
                || self.invalidate_action_observations(),
                || {
                    dcc_cua_platform_windows::activate_window(
                        dcc_cua_platform_windows::UiaTarget {
                            process_id: expected_pid,
                            window_handle: expected_window,
                        },
                        || windows_platform_window_activation_gate("explicit_activation"),
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
            self.require_observed_window_activation_available()?;
            if target.pid != expected_pid || target.window_id != expected_window {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::TargetUnavailable,
                    "the exact target identity changed during session activation",
                ));
            }
            if !target.is_foreground {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::ForegroundActivationRefused,
                    "the exact Windows target is not foreground after activation",
                )
                .with_details(ComputerUseErrorDetails {
                    phase: Some(ComputerUseErrorPhase::ActivationDispatch),
                    focus_mutation_attempted: Some(true),
                    action_attempted: Some(false),
                    input_sent: Some(ComputerUseInputState::NotSent),
                    completion: Some(ComputerUseCompletionState::Known),
                    automatic_input: Some(false),
                    blind_retry: Some(false),
                    fresh_observation_required: Some(true),
                    background_delivery_viable: Some(true),
                    suggested_delivery_mode: Some("background".into()),
                    ..Default::default()
                }));
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
        #[cfg(windows)]
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
                    || windows_platform_input_gate("restore_pre_mutation"),
                    || windows_platform_input_gate("activate_pre_mutation"),
                )
                .map_err(|error| {
                    let mut error = map_windows_window_mutation_error(
                        "restore and activate the exact Windows target",
                        error,
                    );
                    let details = error.details.get_or_insert_default();
                    details.automatic_input = Some(false);
                    details.blind_retry = Some(false);
                    details.fresh_observation_required = Some(true);
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
                return Err(local_activation_validation_failure(
                    ComputerUseErrorCode::TargetUnavailable,
                    "the exact target identity changed during restore_activate",
                ));
            }
            if !target.is_foreground {
                return Err(local_activation_validation_failure(
                    ComputerUseErrorCode::InputFailed,
                    "restore_activate did not leave the exact target restored, visible, and foreground",
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

    pub(super) async fn resolve_target(&self) -> ComputerUseResult<WindowTarget> {
        #[cfg(feature = "test-support")]
        if self.synthetic_test_session {
            return self.target.clone().ok_or_else(|| {
                ComputerUseError::new(
                    ComputerUseErrorCode::MissingWindow,
                    "synthetic test session target is missing",
                )
            });
        }
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
