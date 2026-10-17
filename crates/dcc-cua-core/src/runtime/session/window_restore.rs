use super::*;

#[cfg(windows)]
fn restoration_failure(mut error: ComputerUseError, attempted: bool) -> ComputerUseError {
    let details = error.details.get_or_insert_default();
    details.phase = Some(if attempted {
        ComputerUseErrorPhase::LocalMutationDispatch
    } else {
        ComputerUseErrorPhase::PreDispatch
    });
    details.action_attempted = Some(attempted);
    details.focus_mutation_attempted = Some(attempted);
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
async fn finalize_restoration<T>(
    attempted: bool,
    readback: impl Future<Output = ComputerUseResult<T>>,
    final_check: impl FnOnce() -> ComputerUseResult<()>,
) -> ComputerUseResult<T> {
    let value = readback
        .await
        .map_err(|error| restoration_failure(error, attempted))?;
    final_check().map_err(|error| restoration_failure(error, attempted))?;
    Ok(value)
}

#[cfg(windows)]
fn validate_restored_native_state(
    expected: &dcc_cua_platform_windows::ExactWindowNativeState,
    state: &dcc_cua_platform_windows::ExactWindowNativeState,
) -> ComputerUseResult<()> {
    if state.instance != expected.instance
        || state.process_id != expected.process_id
        || state.window_handle != expected.window_handle
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetUnavailable,
            "the exact native instance changed during restoration refresh",
        ));
    }
    if state.minimized || state.maximized || !state.visible || !state.foreground {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InputFailed,
            "the refreshed exact native target must remain restored, visible, and foreground",
        ));
    }
    Ok(())
}

#[cfg(windows)]
struct RestoreCancellation {
    generation: u64,
    interrupted: bool,
    presentation: Option<dcc_cua_indicator::BannerInputSuppressionSource>,
}

#[cfg(windows)]
impl RestoreCancellation {
    fn check(&self) -> ComputerUseResult<()> {
        super::super::windows_pixel_input::validate_pixel_interrupt(
            self.generation,
            dcc_cua_interrupt::interrupt_generation(),
            self.interrupted,
        )?;
        if let Some(source) = &self.presentation {
            source.validate_active().map_err(|_| {
                ComputerUseError::new(
                    ComputerUseErrorCode::UserInterrupted,
                    "the owning control presenter stopped during explicit restoration",
                )
            })?;
        }
        Ok(())
    }

    fn gate(
        &self,
        error: &std::cell::RefCell<Option<ComputerUseError>>,
        stage: &'static str,
    ) -> Result<(), dcc_cua_platform_windows::UiaError> {
        let check = || {
            self.check().map_err(|failure| {
                *error.borrow_mut() = Some(failure);
                dcc_cua_platform_windows::UiaError::PermissionDenied(
                    "explicit restoration was interrupted".into(),
                )
            })
        };
        check()?;
        windows_platform_input_gate(stage)?;
        check()
    }
}

impl ComputerUseSession {
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
        let cancellation = RestoreCancellation {
            generation: dcc_cua_interrupt::interrupt_generation(),
            interrupted: self.control_banner_interrupted(),
            presentation: self
                .control_banner
                .as_ref()
                .map(ControlBanner::input_suppression_source),
        };
        #[cfg(windows)]
        let cancellation_error = std::cell::RefCell::new(None);
        #[cfg(windows)]
        let expected = self.revalidate_observed_target().await?;
        #[cfg(windows)]
        let mutation_gate =
            self.finish_observed_input_gate(interactive_desktop::require_input_available());
        #[cfg(windows)]
        let receipt = run_gated_preinvalidated_window_mutation(
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
                    || cancellation.gate(&cancellation_error, "restore_pre_mutation"),
                    || cancellation.gate(&cancellation_error, "activate_pre_mutation"),
                )
                .map_err(|failure| {
                    let (attempted, error) = match failure {
                        dcc_cua_platform_windows::ExactWindowRestoreError::BeforeDispatch(
                            error,
                        ) => (false, error),
                        dcc_cua_platform_windows::ExactWindowRestoreError::AfterDispatch(error) => {
                            (true, error)
                        }
                    };
                    let error = cancellation_error.borrow_mut().take().unwrap_or_else(|| {
                        map_windows_window_mutation_error(
                            "restore and activate the exact Windows target",
                            error,
                        )
                    });
                    restoration_failure(error, attempted)
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
            let target = finalize_restoration(receipt.window_mutation_attempted, async {
                let target = self.require_observed_target_available().await?;
                self.require_observed_input_available()?;
                if target.pid != expected.pid || target.window_id != expected.window_id {
                    return Err(ComputerUseError::new(
                        ComputerUseErrorCode::TargetUnavailable,
                        "the exact target identity changed during restore_activate",
                    ));
                }
                let state = dcc_cua_platform_windows::exact_window_native_state(expected.pid, expected.window_id)
                    .map_err(|error| map_windows_window_mutation_error("validate restored native instance after refresh", error))?;
                validate_restored_native_state(&receipt.state, &state)?;
                if !target.is_foreground {
                    return Err(ComputerUseError::new(
                        ComputerUseErrorCode::InputFailed,
                        "restore_activate did not leave the exact target restored, visible, and foreground",
                    ));
                }
                Ok(target)
            }, || cancellation.check()).await?;
            self.set_banner_activity(BannerActivity::Ready);
            Ok(json!({
                "success": true,
                "automatic_input": false,
                "target": target,
                "fresh_observation_required": true,
            }))
        }
    }
}

#[cfg(test)]
mod tests;
