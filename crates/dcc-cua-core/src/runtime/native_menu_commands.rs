use super::*;

#[cfg(windows)]
pub(super) struct NativeMenuMetadata {
    id: String,
    session_id: String,
    read_at: Instant,
    proof: dcc_cua_platform_windows::NativeMenuProof,
}

#[cfg(windows)]
impl NativeMenuMetadata {
    fn validate_gate_authority(
        &self,
        id: &str,
        scope: &ComputerUseTargetScope,
        session: &str,
        phase: dcc_cua_platform_windows::NativeMenuGatePhase,
        now: Instant,
        authorization_deadline: Instant,
    ) -> ComputerUseResult<()> {
        match phase {
            // The single mutation has already consumed this authority. Its
            // bounded readback still applies current lifecycle/desktop fences.
            dcc_cua_platform_windows::NativeMenuGatePhase::PostDispatch => return Ok(()),
            dcc_cua_platform_windows::NativeMenuGatePhase::PreDispatch => {}
        }
        self.validate(
            id,
            scope,
            session,
            now.saturating_duration_since(self.read_at),
        )?;
        if now >= authorization_deadline {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::SessionRefreshRequired,
                "native menu task authorization expired",
            ));
        }
        Ok(())
    }

    fn validate(
        &self,
        id: &str,
        scope: &ComputerUseTargetScope,
        session: &str,
        age: Duration,
    ) -> ComputerUseResult<()> {
        let popup = &self.proof.popup;
        let owner = &self.proof.owner;
        if self.id != id
            || !id.starts_with("native-menu-state-")
            || self.session_id != session
            || age > Duration::from_secs(5)
            || scope.process_id != Some(popup.process_id)
            || scope.window_handle != Some(popup.window_handle)
            || popup.process_id == 0
            || popup.window_handle == 0
            || !popup.visible
            || popup.minimized
            || popup.instance.process_creation_time_100ns == 0
            || popup.instance.window_thread_id == 0
            || popup.instance.window_class_hash == 0
            || owner.window_handle == 0
            || owner.window_handle == popup.window_handle
            || owner.process_id != popup.process_id
            || owner.instance.process_creation_time_100ns
                != popup.instance.process_creation_time_100ns
            || owner.instance.window_thread_id != popup.instance.window_thread_id
            || owner.instance.window_class_hash == 0
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "dismiss_menu requires its latest purpose-bound native menu state token, at most five seconds old",
            ));
        }
        Ok(())
    }
}

#[cfg(windows)]
fn menu_attempt_error(mut error: ComputerUseError, attempted: bool) -> ComputerUseError {
    if attempted {
        error.code = ComputerUseErrorCode::CompletionUnknown;
    }
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

impl ComputerUseSession {
    /// An explicit read mints distinct frame and menu authorities. A failed menu
    /// proof invalidates both; serialized state can never reconstruct authority.
    pub async fn native_window_state_for_menu_dismiss(&mut self) -> ComputerUseResult<Value> {
        #[cfg(not(windows))]
        {
            Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "native menu metadata is available only on Windows",
            ))
        }
        #[cfg(windows)]
        {
            let result = async {
                let mut state = self.native_window_state_for_frame().await?;
                let proof = dcc_cua_platform_windows::read_exact_native_menu(
                    dcc_cua_platform_windows::UiaTarget {
                        process_id: self.scope.process_id.unwrap_or_default(),
                        window_handle: self.scope.window_handle.unwrap_or_default(),
                    },
                )
                .map_err(|error| {
                    map_windows_window_mutation_error("read exact active native menu", error)
                })?;
                if state["native_instance"] != json!(proof.popup.instance) {
                    return Err(ComputerUseError::new(
                        ComputerUseErrorCode::StaleObservation,
                        "native popup instance changed during explicit menu state read",
                    ));
                }
                let id = format!("native-menu-state-{}", uuid::Uuid::new_v4());
                let metadata = NativeMenuMetadata {
                    id: id.clone(),
                    session_id: self.session_id.clone(),
                    read_at: Instant::now(),
                    proof,
                };
                metadata.validate(&id, &self.scope, &self.session_id, Duration::ZERO)?;
                state["menu_dismiss_window_state_id"] = json!(id);
                state["menu_dismiss_authority"] =
                    json!({"purpose":"dismiss_menu", "max_age_ms":5000, "one_use":true});
                self.native_menu_metadata = Some(metadata);
                Ok(state)
            }
            .await;
            self.finish_observation_sensitive_attempt(result)
        }
    }

    #[cfg(windows)]
    pub fn invalidate_action_observations_preserving_native_menu_state(
        &mut self,
        menu_id: &str,
        frame_id: Option<&str>,
    ) {
        let menu = self.native_menu_metadata.take().filter(|metadata| {
            metadata
                .validate(
                    menu_id,
                    &self.scope,
                    &self.session_id,
                    metadata.read_at.elapsed(),
                )
                .is_ok()
        });
        if let Some(frame_id) = frame_id {
            self.invalidate_action_observations_preserving_native_state(frame_id);
        } else {
            self.invalidate_action_observations();
        }
        self.native_menu_metadata = menu;
    }

    /// Consume one purpose-bound menu proof, including every refused attempt.
    /// The monotonic deadline is derived by Host from its trusted task lease.
    pub async fn dismiss_native_menu_from_state(
        &mut self,
        state_id: &str,
        authorization_deadline: Instant,
    ) -> ComputerUseResult<Value> {
        #[cfg(not(windows))]
        {
            let _ = (state_id, authorization_deadline);
            self.invalidate_action_observations();
            Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "native menu dismissal is available only on Windows",
            ))
        }
        #[cfg(windows)]
        {
            let started_generation = dcc_cua_interrupt::interrupt_generation();
            let metadata = self.native_menu_metadata.take();
            self.invalidate_action_observations();
            let result: ComputerUseResult<Value> = async {
                self.ensure_active()?;
                #[cfg(feature = "test-support")]
                if self.synthetic_test_session { return Err(ComputerUseError::new(ComputerUseErrorCode::BackendUnavailable, "synthetic sessions cannot dismiss native menus")); }
                if self.pixel_observation_route != Some(PixelObservationRoute::ExplicitPixelsOnly) {
                    return Err(ComputerUseError::new(ComputerUseErrorCode::InvalidAction, "native menu dismissal requires explicit pixels_only mode"));
                }
                let metadata = metadata.ok_or_else(|| ComputerUseError::new(ComputerUseErrorCode::StaleObservation, "read fresh native menu state before dismiss_menu"))?;
                metadata.validate(state_id, &self.scope, &self.session_id, metadata.read_at.elapsed())?;
                let target = self.revalidate_observed_target().await?;
                if target.pid != metadata.proof.popup.process_id || target.window_id != metadata.proof.popup.window_handle {
                    return Err(ComputerUseError::new(ComputerUseErrorCode::StaleObservation, "native menu target changed before dismissal"));
                }
                let lifecycle = self.control_banner.as_ref().map(ControlBanner::input_suppression_source)
                    .ok_or_else(|| ComputerUseError::new(ComputerUseErrorCode::UserInterrupted, "native menu dismissal requires its active control session"))?;
                let scope = &self.scope;
                let session_id = &self.session_id;
                let mut gate_error = None;
                let outcome = dcc_cua_platform_windows::dismiss_exact_native_menu(&metadata.proof, |phase| {
                    let result = (|| {
                        metadata.validate_gate_authority(state_id, scope, session_id, phase, Instant::now(), authorization_deadline)?;
                        // This source is used only as a shared read-only lifecycle
                        // fence. No input-suppression lease is acquired.
                        lifecycle.validate_active().map_err(|_| ComputerUseError::new(ComputerUseErrorCode::UserInterrupted, "native menu control session stopped"))?;
                        super::windows_pixel_input::validate_pixel_interrupt(started_generation, dcc_cua_interrupt::interrupt_generation(), false)?;
                        windows_platform_window_activation_gate("native_menu_dismiss").map_err(|error| map_windows_window_mutation_error("validate native menu desktop", error))?;
                        lifecycle.validate_active().map_err(|_| ComputerUseError::new(ComputerUseErrorCode::UserInterrupted, "native menu control session stopped after desktop validation"))?;
                        super::windows_pixel_input::validate_pixel_interrupt(started_generation, dcc_cua_interrupt::interrupt_generation(), false)
                    })();
                    result.map_err(|error| {
                        gate_error = Some(error);
                        dcc_cua_platform_windows::UiaError::PermissionDenied("native menu authority or desktop gate refused".into())
                    })
                });
                let outcome = outcome.map_err(|failure| {
                    let (attempted, source) = match failure {
                        dcc_cua_platform_windows::NativeMenuDismissError::BeforeDispatch(error) => (false, error),
                        dcc_cua_platform_windows::NativeMenuDismissError::AfterDispatch(error) => (true, error),
                    };
                    menu_attempt_error(gate_error.unwrap_or_else(|| map_windows_window_mutation_error("dismiss exact native menu", source)), attempted)
                })?;
                Ok(json!({"success":true,"effect":"confirmed","operation":"dismiss_menu",
                    "popup_hidden_or_gone":outcome.popup_hidden_or_gone,"menu_inactive":outcome.menu_inactive,"owner_alive":outcome.owner_alive,
                    "message_attempted":outcome.message_attempted,"input_sent":outcome.input_sent,"automatic_input":false,"process_terminated":false,
                    "window_state_id":state_id,"target":{"process_id":outcome.proof.popup.process_id,"window_handle":outcome.proof.popup.window_handle},
                    "native_instance":outcome.proof.popup.instance,"owner":{"process_id":outcome.proof.owner.process_id,"window_handle":outcome.proof.owner.window_handle,"native_instance":outcome.proof.owner.instance},
                    "fresh_observation_required":true,"cua":{"path":"windows_exact_native_menu_cancelmode"}}))
            }.await;
            result.map_err(|error| {
                let attempted = error
                    .details
                    .as_ref()
                    .and_then(|details| details.action_attempted)
                    .unwrap_or(false);
                menu_attempt_error(error, attempted)
            })
        }
    }
}

#[cfg(all(test, windows))]
mod tests;
