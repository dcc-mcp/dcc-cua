#[cfg(not(windows))]
use super::action_result::validated_action_effect;
use super::*;

#[cfg(windows)]
fn minimize_observation_instance(
    observation: &ComputerUseObservation,
    requested_id: &str,
    scope: &ComputerUseTargetScope,
    session_id: &str,
) -> ComputerUseResult<dcc_cua_platform_windows::ExactWindowPixelInstanceEvidence> {
    let stale = || {
        ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "minimize requires the latest exact-window observation with actual native capture identity",
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

impl ComputerUseSession {
    /// One exact-instance minimize request authorized by the latest actual
    /// native capture. A state read or semantic-only token cannot authorize it.
    pub async fn minimize_window(&mut self, observation_id: &str) -> ComputerUseResult<Value> {
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
            let instance = minimize_observation_instance(
                &observation,
                observation_id,
                &self.scope,
                &self.session_id,
            )?;
            let target = self.revalidate_observed_target().await?;
            if target.pid != observation.process_id || target.window_id != observation.window_handle
            {
                self.invalidate_action_observations();
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "exact minimize target changed after observation",
                ));
            }
            let outcome = dcc_cua_platform_windows::minimize_exact_window(
                dcc_cua_platform_windows::UiaTarget {
                    process_id: target.pid,
                    window_handle: target.window_id,
                },
                instance,
                || windows_platform_window_activation_gate("exact_minimize_pre_dispatch"),
            );
            // Dispatch may have occurred even when readback fails. Never reuse the token.
            self.invalidate_action_observations();
            let state = outcome.map_err(|failure| {
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
            minimize_observation_instance(&observed, requested, &scope, "test").is_ok(),
            valid
        );
    }
}
