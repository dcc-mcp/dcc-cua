#[cfg(not(windows))]
use super::action_result::validated_action_effect;
use super::*;

pub(super) fn exact_physical_window_frame(
    request: &ComputerUseWindowFrameRequest,
) -> ComputerUseResult<[i32; 4]> {
    request.validate()?;
    let values = [request.x, request.y, request.width, request.height];
    if values
        .iter()
        .any(|v| v.fract() != 0.0 || *v < i32::MIN as f64 || *v > i32::MAX as f64)
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "native physical frame requires exact i32 integers",
        ));
    }
    let frame = values.map(|v| v as i32);
    if frame[0].checked_add(frame[2]).is_none() || frame[1].checked_add(frame[3]).is_none() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "native physical frame extents overflow",
        ));
    }
    Ok(frame)
}

#[cfg(windows)]
fn native_frame_attempt_error(mut error: ComputerUseError, attempted: bool) -> ComputerUseError {
    let details = error.details.get_or_insert_default();
    details.phase = Some(if attempted {
        ComputerUseErrorPhase::LocalMutationDispatch
    } else {
        ComputerUseErrorPhase::PreDispatch
    });
    details.action_attempted = Some(attempted);
    details.input_sent = Some(ComputerUseInputState::NotSent);
    details.completion = Some(if attempted {
        ComputerUseCompletionState::Unknown
    } else {
        ComputerUseCompletionState::Known
    });
    details.effect_unknown = Some(attempted);
    details.automatic_input = Some(false);
    details.blind_retry = Some(false);
    details.fresh_observation_required = Some(true);
    error
}

#[cfg(windows)]
pub(super) struct NativeWindowFrameMetadata {
    id: String,
    session_id: String,
    read_at: Instant,
    state: dcc_cua_platform_windows::ExactWindowNativeState,
}

#[cfg(windows)]
impl NativeWindowFrameMetadata {
    fn validate(
        &self,
        id: &str,
        scope: &ComputerUseTargetScope,
        session: &str,
        age: Duration,
    ) -> ComputerUseResult<()> {
        if id.is_empty()
            || self.id != id
            || self.session_id != session
            || age > Duration::from_secs(5)
            || scope.process_id != Some(self.state.process_id)
            || scope.window_handle != Some(self.state.window_handle)
            || self.state.instance.process_creation_time_100ns == 0
            || self.state.instance.window_thread_id == 0
            || self.state.process_id == 0
            || self.state.window_handle == 0
            || self.state.dpi == 0
            || self.state.bounds.is_none()
            || self.state.visible_bounds.is_none()
            || !self.state.visible
            || self.state.minimized
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "set_window_frame requires the latest exact native window_state_id, at most five seconds old",
            ));
        }
        for frame in [self.state.bounds, self.state.visible_bounds] {
            if frame.is_none_or(|frame| {
                dcc_cua_platform_windows::validate_exact_window_frame(frame).is_err()
            }) {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "native frame metadata requires actual valid Win32 and DWM physical bounds",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
fn retain_fresh_native_frame_metadata(
    metadata: Option<NativeWindowFrameMetadata>,
    state_id: &str,
    scope: &ComputerUseTargetScope,
    session_id: &str,
) -> Option<NativeWindowFrameMetadata> {
    metadata.filter(|metadata| {
        metadata
            .validate(state_id, scope, session_id, metadata.read_at.elapsed())
            .is_ok()
    })
}

#[cfg(windows)]
fn native_frame_confirmed_result(
    requested: [i32; 4],
    state_id: &str,
    state: dcc_cua_platform_windows::ExactWindowNativeState,
) -> Value {
    let actual = json!({"process_id":state.process_id,"window_handle":state.window_handle,"exists":true,
        "visible":state.visible && !state.minimized,"minimized":state.minimized,"foreground":state.foreground,
        "bounds":state.bounds,"visible_bounds":state.visible_bounds,"dpi":state.dpi,
        "native_instance":state.instance,"backend":"windows-exact-native-state"});
    json!({"success":true,"effect":"confirmed","operation":"set_window_frame",
        "requested_frame":{"x":requested[0],"y":requested[1],"width":requested[2],"height":requested[3]},
        "applied_frame":requested,"state":actual,
        "target":{"process_id":state.process_id,"window_handle":state.window_handle},
        "native_instance":state.instance,"window_state_id":state_id,
        "fresh_observation_required":true,"automatic_input":false,"process_terminated":false,
        "cua":{"path":"windows_exact_instance_set_window_pos"}})
}

#[cfg(windows)]
pub(super) fn exact_native_observation_instance(
    observation: &ComputerUseObservation,
    requested_id: &str,
    scope: &ComputerUseTargetScope,
    session_id: &str,
) -> ComputerUseResult<dcc_cua_platform_windows::ExactWindowPixelInstanceEvidence> {
    let stale = || {
        ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "the action requires the latest exact-window observation with actual native capture identity",
        )
    };
    let provenance = &observation.capture_provenance;
    if requested_id.is_empty()
        || observation.observation_id != requested_id
        || observation.session_id != session_id
        || scope.process_id != Some(observation.process_id)
        || scope.window_handle != Some(observation.window_handle)
        || provenance["process_id"].as_u64() != Some(u64::from(observation.process_id))
        || provenance["window_handle"].as_u64() != Some(observation.window_handle)
        || provenance["pixels_captured"] != true
        || provenance["whole_desktop_capture"] != false
        || provenance["scope"] != "window"
        || !provenance["capture_generation"]
            .as_u64()
            .is_some_and(|value| value > 0)
        || !provenance["window_dpi"]
            .as_u64()
            .is_some_and(|value| value > 0)
        || !matches!(
            observation.capture_backend.as_str(),
            "dcc-cua-wgc-exact-window" | "dcc-cua-visible-exact-window"
        )
    {
        return Err(stale());
    }
    let instance = &provenance["native_instance"];
    let creation = instance["process_creation_time_100ns"]
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(stale)?;
    let thread = instance["window_thread_id"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(stale)?;
    Ok(dcc_cua_platform_windows::ExactWindowPixelInstanceEvidence {
        process_creation_time_100ns: creation,
        window_thread_id: thread,
        window_class_hash: instance["window_class_hash"].as_u64().ok_or_else(stale)?,
        owner_window_handle: instance["owner_window_handle"].as_u64().ok_or_else(stale)?,
    })
}

#[cfg(windows)]
pub(super) fn exact_native_observation_geometry(
    observation: &ComputerUseObservation,
) -> ComputerUseResult<dcc_cua_platform_windows::NativeWindowGeometry> {
    let stale = || {
        ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "the latest native observation requires complete unscaled physical capture geometry",
        )
    };
    let provenance = &observation.capture_provenance;
    let native = dcc_cua_platform_windows::NativeWindowGeometry {
        win32_bounds: serde_json::from_value(provenance["native_window_bounds"].clone())
            .map_err(|_| stale())?,
        dwm_bounds: Some(
            serde_json::from_value(provenance["native_visible_bounds"].clone())
                .map_err(|_| stale())?,
        ),
        dpi: provenance["window_dpi"]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(stale)?,
    };
    dcc_cua_platform_windows::validate_native_window_geometry(native, native)
        .map_err(|_| stale())?;
    if [observation.source_rect[2], observation.source_rect[3]]
        != [
            i32::try_from(observation.width).map_err(|_| stale())?,
            i32::try_from(observation.height).map_err(|_| stale())?,
        ]
    {
        return Err(stale());
    }
    match observation.capture_backend.as_str() {
        "dcc-cua-wgc-exact-window" => {
            let proof: dcc_cua_platform_windows::ResolvedWgcGeometry =
                serde_json::from_value(provenance["wgc_geometry"].clone()).map_err(|_| stale())?;
            let resolved = dcc_cua_platform_windows::resolve_exact_wgc_geometry(
                native,
                native,
                proof.frame,
                proof.bgra_byte_len,
            )
            .map_err(|_| stale())?;
            if resolved != proof || resolved.source_rect != observation.source_rect {
                return Err(stale());
            }
        }
        "dcc-cua-visible-exact-window" => {
            if Some(observation.source_rect) != native.dwm_bounds
                || !provenance["wgc_geometry"].is_null()
            {
                return Err(stale());
            }
        }
        _ => return Err(stale()),
    }
    Ok(native)
}

#[cfg(windows)]
fn validate_observed_minimize_geometry(
    geometry: dcc_cua_platform_windows::NativeWindowGeometry,
    instance: dcc_cua_platform_windows::ExactWindowPixelInstanceEvidence,
    current: &dcc_cua_platform_windows::ExactWindowPixelEvidence,
) -> ComputerUseResult<()> {
    let actual = dcc_cua_platform_windows::NativeWindowGeometry {
        win32_bounds: current.bounds,
        dwm_bounds: Some(current.visible_bounds),
        dpi: current.dpi,
    };
    if current.instance != instance
        || !current.visible
        || current.minimized
        || dcc_cua_platform_windows::validate_native_window_geometry(geometry, actual).is_err()
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "exact captured Win32/DWM geometry or native instance changed before minimize",
        )
        .with_details(ComputerUseErrorDetails {
            phase: Some(ComputerUseErrorPhase::PreDispatch),
            action_attempted: Some(false),
            input_sent: Some(ComputerUseInputState::NotSent),
            completion: Some(ComputerUseCompletionState::Known),
            effect_unknown: Some(false),
            automatic_input: Some(false),
            blind_retry: Some(false),
            fresh_observation_required: Some(true),
            ..Default::default()
        }));
    }
    Ok(())
}

impl ComputerUseSession {
    #[cfg(windows)]
    pub(super) fn remember_native_frame_metadata(
        &mut self,
        state: dcc_cua_platform_windows::ExactWindowNativeState,
    ) -> Option<String> {
        if self.pixel_observation_route != Some(PixelObservationRoute::ExplicitPixelsOnly) {
            return None;
        }
        let id = format!("native-state-{}", uuid::Uuid::new_v4());
        let metadata = NativeWindowFrameMetadata {
            id: id.clone(),
            session_id: self.session_id.clone(),
            read_at: Instant::now(),
            state,
        };
        if metadata
            .validate(&id, &self.scope, &self.session_id, Duration::ZERO)
            .is_err()
        {
            return None;
        }
        self.native_frame_metadata = Some(metadata);
        Some(id)
    }

    /// A successful explicit native state read may transition Host availability.
    /// Preserve only that same trusted fresh object while clearing prior action
    /// evidence. This neither reconstructs metadata from JSON nor extends its age.
    #[cfg(windows)]
    pub fn invalidate_action_observations_preserving_native_state(&mut self, state_id: &str) {
        let metadata = retain_fresh_native_frame_metadata(
            self.native_frame_metadata.take(),
            state_id,
            &self.scope,
            &self.session_id,
        );
        self.invalidate_action_observations();
        self.native_frame_metadata = metadata;
    }

    /// Consume a fresh metadata-only token; it never authorizes pixel or semantic input.
    pub async fn set_window_frame_from_native_state(
        &mut self,
        window_state_id: &str,
        request: &ComputerUseWindowFrameRequest,
    ) -> ComputerUseResult<Value> {
        #[cfg(not(windows))]
        {
            let _ = (window_state_id, request);
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "native metadata frame mutation is available only on Windows",
            ));
        }
        #[cfg(windows)]
        {
            let started_generation = dcc_cua_interrupt::interrupt_generation();
            let metadata = self.native_frame_metadata.take();
            // All attempts consume both metadata and prior input observations.
            self.invalidate_action_observations();
            let result: ComputerUseResult<Value> = async {
            self.ensure_active()?;
            #[cfg(feature="test-support")]
            if self.synthetic_test_session { return Err(ComputerUseError::new(ComputerUseErrorCode::BackendUnavailable,"synthetic test sessions cannot dispatch native frame mutations")); }
            if self.pixel_observation_route!=Some(PixelObservationRoute::ExplicitPixelsOnly) {
                return Err(ComputerUseError::new(ComputerUseErrorCode::InvalidAction,"native metadata frame mutation requires explicit pixels_only mode"));
            }
            let requested=exact_physical_window_frame(request)?;
            let metadata=metadata.ok_or_else(||ComputerUseError::new(ComputerUseErrorCode::StaleObservation,"read fresh native window state before set_window_frame"))?;
            metadata.validate(window_state_id,&self.scope,&self.session_id,metadata.read_at.elapsed())?;
            let target=self.revalidate_observed_target().await?;
            if target.pid!=metadata.state.process_id || target.window_id!=metadata.state.window_handle {
                return Err(ComputerUseError::new(ComputerUseErrorCode::StaleObservation,"native metadata target changed before frame mutation"));
            }
            let mut pre_dispatch_error=None;
            let outcome=dcc_cua_platform_windows::set_exact_window_frame(
                dcc_cua_platform_windows::UiaTarget{process_id:target.pid,window_handle:target.window_id},metadata.state,requested,
                || {
                    if let Err(error)=metadata.validate(window_state_id,&self.scope,&self.session_id,metadata.read_at.elapsed()) {
                        pre_dispatch_error=Some(error);
                        return Err(dcc_cua_platform_windows::UiaError::InvalidTarget("native metadata expired before frame dispatch".into()));
                    }
                    if let Err(error)=super::windows_pixel_input::validate_pixel_interrupt(started_generation,dcc_cua_interrupt::interrupt_generation(),self.control_banner_interrupted()) {
                        pre_dispatch_error=Some(error);
                        return Err(dcc_cua_platform_windows::UiaError::PermissionDenied("exact frame mutation was interrupted".into()));
                    }
                    windows_platform_window_activation_gate("exact_frame_pre_dispatch")?;
                    if let Err(error)=super::windows_pixel_input::validate_pixel_interrupt(started_generation,dcc_cua_interrupt::interrupt_generation(),self.control_banner_interrupted()) {
                        pre_dispatch_error=Some(error);
                        return Err(dcc_cua_platform_windows::UiaError::PermissionDenied("exact frame mutation was interrupted after availability validation".into()));
                    }
                    Ok(())
                });
            let state=outcome.map_err(|failure|{
                if let Some(error)=pre_dispatch_error {return native_frame_attempt_error(error,false);}
                let (attempted,source)=match failure {
                    dcc_cua_platform_windows::ExactWindowFrameError::BeforeDispatch(error)=>(false,error),
                    dcc_cua_platform_windows::ExactWindowFrameError::AfterDispatch(error)=>(true,error),
                };
                native_frame_attempt_error(map_windows_window_mutation_error("set the exact native instance frame",source),attempted)
            })?;
            self.target=Some(WindowTarget {bounds:state.bounds.expect("confirmed physical frame"),..target});
            Ok(native_frame_confirmed_result(requested,window_state_id,state))
            }.await;
            result.map_err(|error| {
                let attempted = error
                    .details
                    .as_ref()
                    .and_then(|details| details.action_attempted)
                    .unwrap_or(false);
                native_frame_attempt_error(error, attempted)
            })
        }
    }

    /// One exact-instance minimize request authorized by the latest actual
    /// native capture. A state read or semantic-only token cannot authorize it.
    pub async fn minimize_window(&mut self, observation_id: &str) -> ComputerUseResult<Value> {
        #[cfg(windows)]
        let started_generation = dcc_cua_interrupt::interrupt_generation();
        self.ensure_active()?;
        #[cfg(feature = "test-support")]
        if self.synthetic_test_session {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "synthetic test sessions cannot dispatch native window mutations",
            ));
        }
        #[cfg(not(windows))]
        {
            let _ = observation_id;
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "exact-instance minimize is available only on Windows",
            ));
        }
        #[cfg(windows)]
        {
            let observation = self.observation.clone().ok_or_else(|| {
                ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "take a fresh exact native screenshot before minimize",
                )
            })?;
            let instance = exact_native_observation_instance(
                &observation,
                observation_id,
                &self.scope,
                &self.session_id,
            )?;
            let geometry = exact_native_observation_geometry(&observation)?;
            let target = self.revalidate_observed_target().await?;
            if target.pid != observation.process_id
                || target.window_id != observation.window_handle
                || target.bounds != geometry.win32_bounds
            {
                self.invalidate_action_observations();
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "exact minimize target changed after observation",
                ));
            }
            let mut pre_dispatch_error = None;
            let outcome = dcc_cua_platform_windows::minimize_exact_window(
                dcc_cua_platform_windows::UiaTarget {
                    process_id: target.pid,
                    window_handle: target.window_id,
                },
                instance,
                || {
                    if let Err(error) = super::windows_pixel_input::validate_pixel_interrupt(
                        started_generation,
                        dcc_cua_interrupt::interrupt_generation(),
                        self.control_banner_interrupted(),
                    ) {
                        pre_dispatch_error = Some(error);
                        return Err(dcc_cua_platform_windows::UiaError::PermissionDenied(
                            "exact window mutation was interrupted before native dispatch".into(),
                        ));
                    }
                    windows_platform_window_activation_gate("exact_minimize_pre_dispatch")?;
                    let current = dcc_cua_platform_windows::exact_window_pixel_evidence(
                        target.pid,
                        target.window_id,
                    )
                    .map_err(|_| {
                        dcc_cua_platform_windows::UiaError::InvalidTarget(
                            "exact captured physical geometry is unavailable before minimize"
                                .into(),
                        )
                    })?;
                    if let Err(error) =
                        validate_observed_minimize_geometry(geometry, instance, &current)
                    {
                        pre_dispatch_error = Some(error);
                        return Err(dcc_cua_platform_windows::UiaError::InvalidTarget(
                            "exact captured physical geometry changed before minimize".into(),
                        ));
                    }
                    if let Err(error) = super::windows_pixel_input::validate_pixel_interrupt(
                        started_generation,
                        dcc_cua_interrupt::interrupt_generation(),
                        self.control_banner_interrupted(),
                    ) {
                        pre_dispatch_error = Some(error);
                        return Err(dcc_cua_platform_windows::UiaError::PermissionDenied(
                            "exact window mutation was interrupted after native geometry validation".into()));
                    }
                    Ok(())
                },
            );
            // Dispatch may have occurred even when readback fails. Never reuse the token.
            self.invalidate_action_observations();
            let state = outcome.map_err(|failure| {
                if let Some(error) = pre_dispatch_error {
                    return error;
                }
                let (attempted, source) = match failure {
                    dcc_cua_platform_windows::ExactWindowMinimizeError::BeforeDispatch(error) => {
                        (false, error)
                    }
                    dcc_cua_platform_windows::ExactWindowMinimizeError::AfterDispatch(error) => {
                        (true, error)
                    }
                };
                let mut error =
                    map_windows_window_mutation_error("minimize the exact native instance", source);
                let details = error.details.get_or_insert_default();
                details.phase = Some(if attempted {
                    ComputerUseErrorPhase::LocalMutationDispatch
                } else {
                    ComputerUseErrorPhase::PreDispatch
                });
                details.action_attempted = Some(attempted);
                details.input_sent = Some(ComputerUseInputState::NotSent);
                details.completion = Some(if attempted {
                    ComputerUseCompletionState::Unknown
                } else {
                    ComputerUseCompletionState::Known
                });
                details.effect_unknown = Some(attempted);
                details.automatic_input = Some(false);
                details.blind_retry = Some(false);
                details.fresh_observation_required = Some(true);
                error
            })?;
            Ok(
                json!({"success":true,"effect":"confirmed","operation":"minimize",
                "target":{"process_id":state.process_id,"window_handle":state.window_handle},
                "state":state,"native_instance":state.instance,
                "observation_id":observation_id,"fresh_observation_required":true,
                "process_terminated":false,"automatic_input":false,
                "cua":{"path":"windows_exact_instance_minimize"}}),
            )
        }
    }

    /// Request a polite close for the exact Windows PID/HWND target.
    /// This never terminates the owning process.
    pub async fn close_window(&mut self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        if self.scope.process_id.is_none() || self.scope.window_handle.is_none() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                "close requires an exact process_id and window_handle grant binding",
            ));
        }
        #[cfg(windows)]
        {
            let target = self.require_observed_target_available().await?;
            self.invalidate_action_observations();
            dcc_cua_platform_windows::post_close_window(
                dcc_cua_platform_windows::UiaTarget {
                    process_id: target.pid,
                    window_handle: target.window_id,
                },
                || Ok(()),
            )
            .map_err(|error| {
                map_windows_window_mutation_error("close the exact Windows target", error)
            })?;
            Ok(json!({
                "success": true,
                "effect": "confirmed",
                "target": {"process_id": target.pid, "window_handle": target.window_id},
                "cua": {"path": "windows_exact_post_wm_close"},
                "process_terminated": false,
                "fresh_observation_required": true,
            }))
        }
        #[cfg(not(windows))]
        Err(ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "exact polite window close is currently available only on Windows",
        ))
    }

    /// Set and independently revalidate the exact target window frame through CUA.
    pub async fn set_window_frame(
        &mut self,
        request: &ComputerUseWindowFrameRequest,
    ) -> ComputerUseResult<Value> {
        #[cfg(windows)]
        if self.pixel_observation_route == Some(PixelObservationRoute::ExplicitPixelsOnly) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "pixels_only set_window_frame requires a fresh native window_state_id",
            ));
        }
        request.validate()?;
        self.ensure_active()?;
        #[cfg(windows)]
        {
            self.require_observed_window_activation_available()?;
            let target = self.require_observed_target_available().await?;
            let requested = [
                request.x.round() as i32,
                request.y.round() as i32,
                request.width.round() as i32,
                request.height.round() as i32,
            ];
            self.invalidate_action_observations();
            let applied = dcc_cua_platform_windows::set_window_frame(
                dcc_cua_platform_windows::UiaTarget {
                    process_id: target.pid,
                    window_handle: target.window_id,
                },
                requested,
                || windows_platform_window_activation_gate("set_window_frame"),
            )
            .map_err(|error| {
                map_windows_window_mutation_error("set the exact Windows target frame", error)
            })?;
            let target = self.require_observed_target_available().await?;
            self.target = Some(target.clone());
            Ok(json!({
                "success": true,
                "effect": "confirmed",
                "requested_frame": request,
                "applied_frame": applied,
                "target": target,
                "cua": {"path": "windows_exact_set_window_pos"},
                "text": "Set and verified the exact Windows PID/HWND frame.",
                "degraded": false,
            }))
        }

        #[cfg(not(windows))]
        {
            let target = self.preflight_mutating_bound_tool().await?;

            // The backend may mutate before a timeout or partial result is observed.
            self.invalidate_action_observations();
            let result = self
                .call_bound_tool_without_refresh(
                    "set_window_frame",
                    json!({
                        "pid": target.pid,
                        "window_id": target.window_id,
                        "x": request.x,
                        "y": request.y,
                        "width": request.width,
                        "height": request.height,
                    }),
                )
                .await;
            let result = self.finish_observation_sensitive_attempt(result)?;
            let effect = validated_action_effect(&result, "set_window_frame")?;
            let result = native_tool_result(result)?;
            let target = self.require_observed_target_available().await?;
            self.require_observed_input_available()?;
            self.target = Some(target.clone());
            let success = effect == "confirmed";

            Ok(json!({
                "success": success,
                "effect": effect,
                "requested_frame": request,
                "target": target,
                "cua": result.value,
                "text": result.text,
                "degraded": result.degraded,
            }))
        }
    }
}

#[cfg(test)]
mod native_frame_tests {
    use super::*;

    #[test]
    fn physical_frame_request_requires_exact_bounded_integers() {
        let request = ComputerUseWindowFrameRequest {
            x: 50.0,
            y: 800.0,
            width: 926.0,
            height: 680.0,
        };
        assert_eq!(
            exact_physical_window_frame(&request).unwrap(),
            [50, 800, 926, 680]
        );
        for values in [
            [0.5, 0.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, -1.0],
            [f64::NAN, 0.0, 1.0, 1.0],
            [0.0, f64::INFINITY, 1.0, 1.0],
            [i32::MAX as f64, 0.0, 1.0, 1.0],
            [0.0, 0.0, i32::MAX as f64 + 1.0, 1.0],
            [0.0, i32::MAX as f64, 1.0, 1.0],
        ] {
            assert!(
                exact_physical_window_frame(&ComputerUseWindowFrameRequest {
                    x: values[0],
                    y: values[1],
                    width: values[2],
                    height: values[3]
                })
                .is_err()
            );
        }
    }

    #[cfg(windows)]
    fn metadata() -> NativeWindowFrameMetadata {
        NativeWindowFrameMetadata {
            id: "native-state-1".into(),
            session_id: "session-1".into(),
            read_at: Instant::now(),
            state: dcc_cua_platform_windows::ExactWindowNativeState {
                process_id: 42,
                window_handle: 99,
                bounds: Some([80, 80, 1500, 1400]),
                visible_bounds: Some([93, 80, 1474, 1387]),
                dpi: 240,
                visible: true,
                minimized: false,
                foreground: false,
                instance: dcc_cua_platform_windows::ExactWindowPixelInstanceEvidence {
                    process_creation_time_100ns: 7,
                    window_thread_id: 8,
                    window_class_hash: 9,
                    owner_window_handle: 0,
                },
            },
        }
    }

    #[cfg(windows)]
    #[test]
    fn native_frame_metadata_is_session_target_and_five_second_bound() {
        let scope = ComputerUseTargetScope {
            process_id: Some(42),
            window_handle: Some(99),
            window_title: None,
        };
        metadata()
            .validate(
                "native-state-1",
                &scope,
                "session-1",
                Duration::from_secs(5),
            )
            .unwrap();
        for change in 0..12 {
            let mut proof = metadata();
            let mut actual_scope = scope.clone();
            let mut age = Duration::ZERO;
            let mut id = "native-state-1";
            let mut session = "session-1";
            match change {
                0 => id = "snapshot-1",
                1 => id = "",
                2 => session = "session-2",
                3 => age = Duration::from_secs(5) + Duration::from_nanos(1),
                4 => actual_scope.process_id = Some(43),
                5 => actual_scope.window_handle = Some(100),
                6 => proof.state.instance.process_creation_time_100ns = 0,
                7 => proof.state.instance.window_thread_id = 0,
                8 => proof.state.dpi = 0,
                9 => proof.state.bounds = None,
                10 => proof.state.visible = false,
                _ => proof.state.minimized = true,
            }
            assert!(
                proof.validate(id, &actual_scope, session, age).is_err(),
                "change {change}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn native_frame_failure_projection_never_claims_confirmed_effect() {
        for attempted in [false, true] {
            let error = native_frame_attempt_error(
                ComputerUseError::new(ComputerUseErrorCode::UserInterrupted, "stopped"),
                attempted,
            );
            assert_eq!(error.code, ComputerUseErrorCode::UserInterrupted);
            let details = error.details.unwrap();
            assert_eq!(details.action_attempted, Some(attempted));
            assert_eq!(details.effect_unknown, Some(attempted));
            assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
            assert_eq!(
                details.completion,
                Some(if attempted {
                    ComputerUseCompletionState::Unknown
                } else {
                    ComputerUseCompletionState::Known
                })
            );
            assert_eq!(details.automatic_input, Some(false));
            assert_eq!(details.blind_retry, Some(false));
            assert_eq!(details.fresh_observation_required, Some(true));
        }
    }

    #[cfg(windows)]
    #[test]
    fn native_frame_availability_transition_never_extends_or_reconstructs_authority() {
        let scope = ComputerUseTargetScope {
            process_id: Some(42),
            window_handle: Some(99),
            window_title: None,
        };
        let original = metadata();
        let read_at = original.read_at;
        let kept = retain_fresh_native_frame_metadata(
            Some(original),
            "native-state-1",
            &scope,
            "session-1",
        )
        .unwrap();
        assert_eq!(kept.read_at, read_at);
        let mut expired = metadata();
        expired.read_at = Instant::now() - Duration::from_secs(6);
        assert!(
            retain_fresh_native_frame_metadata(
                Some(expired),
                "native-state-1",
                &scope,
                "session-1"
            )
            .is_none()
        );
        assert!(
            retain_fresh_native_frame_metadata(Some(metadata()), "snapshot-1", &scope, "session-1")
                .is_none()
        );
        assert!(
            retain_fresh_native_frame_metadata(None, "native-state-1", &scope, "session-1")
                .is_none()
        );
        let mut missing = metadata();
        missing.state.visible_bounds = None;
        assert!(
            retain_fresh_native_frame_metadata(
                Some(missing),
                "native-state-1",
                &scope,
                "session-1"
            )
            .is_none()
        );
    }

    #[cfg(windows)]
    #[test]
    fn native_frame_confirmed_wire_uses_integer_frame_and_no_new_metadata_token() {
        let requested = [50, 800, 926, 680];
        let mut actual = metadata().state;
        actual.bounds = Some(requested);
        let wire = native_frame_confirmed_result(requested, "consumed-native-state", actual);
        assert_eq!(
            wire["requested_frame"],
            json!({"x":50,"y":800,"width":926,"height":680})
        );
        assert!(wire["requested_frame"]["x"].is_i64());
        assert_eq!(wire["applied_frame"], json!(requested));
        assert_eq!(wire["state"]["native_instance"], wire["native_instance"]);
        assert_eq!(wire["state"]["bounds"], wire["applied_frame"]);
        assert!(wire["state"].get("window_state_id").is_none());
        assert!(wire["state"].get("instance").is_none());
        assert_eq!(wire["window_state_id"], "consumed-native-state");
        assert_eq!(wire["success"], true);
        assert_eq!(wire["effect"], "confirmed");
    }
}

#[cfg(all(test, windows))]
mod minimize_tests {
    use super::*;
    use rstest::rstest;

    fn observation() -> ComputerUseObservation {
        ComputerUseObservation {
            observation_id: "obs-1".into(),
            window_handle: 99,
            process_id: 42,
            window_title: String::new(),
            width: 1500,
            height: 1400,
            source_rect: [80, 80, 1500, 1400],
            capture_backend: "dcc-cua-wgc-exact-window".into(),
            session_id: "test".into(),
            capture_provenance: json!({"process_id":42,"window_handle":99,"pixels_captured":true,
                "whole_desktop_capture":false,"scope":"window","capture_generation":1,"window_dpi":240,
                "native_instance":{"process_creation_time_100ns":7,"window_thread_id":8,
                    "window_class_hash":9,"owner_window_handle":0}}),
        }
    }

    #[rstest]
    #[case("valid", true)]
    #[case("wrong_id", false)]
    #[case("wrong_session", false)]
    #[case("wrong_pid", false)]
    #[case("wrong_hwnd", false)]
    #[case("missing_instance", false)]
    #[case("zero_creation", false)]
    #[case("zero_thread", false)]
    #[case("thread_overflow", false)]
    #[case("missing_owner", false)]
    #[case("no_pixels", false)]
    #[case("wrong_backend", false)]
    #[case("zero_generation", false)]
    fn exact_minimize_requires_actual_latest_capture_instance(
        #[case] scenario: &str,
        #[case] valid: bool,
    ) {
        let mut observed = observation();
        let requested = if scenario == "wrong_id" {
            "obs-old"
        } else {
            "obs-1"
        };
        match scenario {
            "wrong_session" => observed.session_id = "other".into(),
            "wrong_pid" => observed.process_id = 43,
            "wrong_hwnd" => observed.window_handle = 100,
            "missing_instance" => observed.capture_provenance["native_instance"] = Value::Null,
            "zero_creation" => {
                observed.capture_provenance["native_instance"]["process_creation_time_100ns"] =
                    json!(0)
            }
            "zero_thread" => {
                observed.capture_provenance["native_instance"]["window_thread_id"] = json!(0)
            }
            "thread_overflow" => {
                observed.capture_provenance["native_instance"]["window_thread_id"] =
                    json!(4294967296_u64)
            }
            "missing_owner" => {
                observed.capture_provenance["native_instance"]
                    .as_object_mut()
                    .unwrap()
                    .remove("owner_window_handle");
            }
            "no_pixels" => observed.capture_provenance["pixels_captured"] = json!(false),
            "wrong_backend" => observed.capture_backend = "cua-driver-sdk".into(),
            "zero_generation" => observed.capture_provenance["capture_generation"] = json!(0),
            _ => {}
        }
        let scope = ComputerUseTargetScope {
            process_id: Some(42),
            window_handle: Some(99),
            window_title: None,
        };
        assert_eq!(
            exact_native_observation_instance(&observed, requested, &scope, "test").is_ok(),
            valid
        );
    }

    #[test]
    fn native_observation_geometry_requires_actual_unscaled_wgc_proof() {
        use dcc_cua_platform_windows::*;
        let mut observed = observation();
        observed.width = 646;
        observed.height = 495;
        observed.source_rect = [73, 80, 646, 495];
        let native = NativeWindowGeometry {
            win32_bounds: [60, 80, 672, 508],
            dwm_bounds: Some(observed.source_rect),
            dpi: 240,
        };
        let shape = WgcFrameGeometry {
            item_size_before: [646, 495],
            item_size_after: [646, 495],
            pool_size: [646, 495],
            content_size: [646, 495],
            texture_size: [646, 495],
            row_pitch_bytes: 2688,
        };
        let proof = resolve_exact_wgc_geometry(native, native, shape, 646 * 495 * 4).unwrap();
        observed.capture_provenance["native_window_bounds"] = json!(native.win32_bounds);
        observed.capture_provenance["native_visible_bounds"] = json!(native.dwm_bounds.unwrap());
        observed.capture_provenance["wgc_geometry"] = json!(proof);
        assert_eq!(
            exact_native_observation_geometry(&observed).unwrap(),
            native
        );
        for change in 0..7 {
            let mut invalid = observed.clone();
            match change {
                0 => invalid.width -= 1,
                1 => invalid.source_rect[0] -= 1,
                2 => invalid.capture_provenance["native_visible_bounds"] = Value::Null,
                3 => invalid.capture_provenance["wgc_geometry"] = Value::Null,
                4 => {
                    invalid.capture_provenance["wgc_geometry"]["frame"]["item_size_after"][0] =
                        json!(647)
                }
                5 => invalid.capture_provenance["wgc_geometry"]["bgra_byte_len"] = json!(1),
                _ => invalid.capture_provenance["wgc_geometry"]["origin"] = json!("win32_window"),
            }
            assert!(
                exact_native_observation_geometry(&invalid).is_err(),
                "change {change}"
            );
        }
        let instance = ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: 7,
            window_thread_id: 8,
            window_class_hash: 9,
            owner_window_handle: 0,
        };
        let current = ExactWindowPixelEvidence {
            process_id: 42,
            window_handle: 99,
            bounds: native.win32_bounds,
            visible_bounds: native.dwm_bounds.unwrap(),
            dpi: 240,
            visible: true,
            minimized: false,
            unobscured: false,
            instance,
        };
        validate_observed_minimize_geometry(native, instance, &current).unwrap();
        for index in 0..4 {
            let mut changed = current;
            changed.visible_bounds[index] += 1;
            assert!(validate_observed_minimize_geometry(native, instance, &changed).is_err());
        }
    }
}
