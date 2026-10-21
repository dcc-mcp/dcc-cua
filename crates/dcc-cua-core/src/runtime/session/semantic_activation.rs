use super::*;

impl ComputerUseSession {
    pub(super) async fn activate_windows_with_semantic_root(
        &mut self,
        target: &WindowTarget,
    ) -> ComputerUseResult<Option<ComputerUseSemanticRootFocusDiagnostic>> {
        let expected_pid = target.pid;
        let expected_window = target.window_id;
        let mutation_gate = self
            .finish_observed_input_gate(interactive_desktop::require_window_activation_available());
        mutation_gate?;
        let activation_generation = dcc_cua_interrupt::interrupt_generation();
        // Keep only a one-use private continuation; never restore either
        // context after invalidating the public observation epoch below.
        let activation_observation = self.observation.take();
        let root_context = self.windows_uia.take();
        let native_before = root_context.as_ref().and_then(|context| {
            context
                .matches(expected_pid, expected_window)
                .then(|| {
                    dcc_cua_platform_windows::exact_window_native_state(
                        expected_pid,
                        expected_window,
                    )
                    .ok()
                })
                .flatten()
        });
        let pixels_only =
            self.pixel_observation_route == Some(PixelObservationRoute::ExplicitPixelsOnly);
        self.invalidate_action_observations();
        let activation = dcc_cua_platform_windows::activate_window(
            dcc_cua_platform_windows::UiaTarget {
                process_id: expected_pid,
                window_handle: expected_window,
            },
            || windows_platform_window_activation_gate("explicit_activation"),
        );
        let root_focus_diagnostic = if let Err(native_error) = activation {
            let native_diagnostic = match &native_error {
                dcc_cua_platform_windows::UiaError::ForegroundActivationRefused {
                    diagnostic,
                    ..
                } => diagnostic.as_deref().cloned(),
                _ => None,
            };
            let error = map_windows_window_mutation_error(
                "activate the exact Windows target",
                native_error,
            );
            let context_matches = root_context
                .as_ref()
                .is_some_and(|context| context.matches(expected_pid, expected_window))
                && native_before.is_some()
                && native_diagnostic.is_some();
            crate::semantic_root_activation::recover_semantic_root_activation(
                error,
                pixels_only,
                activation_observation.as_ref(),
                target,
                context_matches,
                |error| async move {
                    let diagnostic = root_context.expect("validated UIA context")
                        .focus_root_after_activation_refusal(
                            native_before.expect("validated native instance"),
                            native_diagnostic.expect("validated native refusal diagnostic"),
                            activation_generation,
                        ).await;
                    let diagnostic = match diagnostic {
                        Ok(outcome) => {
                            let diagnostic = outcome.diagnostic;
                            let activated = diagnostic.semantic_root_focus.as_ref().is_some_and(|focus| {
                                focus.status == dcc_cua_platform_windows::WindowsSemanticRootFocusStatus::Activated
                            });
                            let public_diagnostic = map_foreground_activation_diagnostic(&diagnostic);
                            if dcc_cua_interrupt::interrupt_generation() != activation_generation {
                                let interrupted = ComputerUseError::new(ComputerUseErrorCode::UserInterrupted, "root activation was interrupted");
                                return Err(crate::semantic_root_activation::with_semantic_root_activation_diagnostic(interrupted, public_diagnostic));
                            }
                            if let Some(prerequisite_error) = outcome.prerequisite_error {
                                let prerequisite_error = map_windows_window_mutation_error("semantic root activation prerequisite", prerequisite_error);
                                return Err(crate::semantic_root_activation::with_semantic_root_activation_diagnostic(prerequisite_error, public_diagnostic));
                            }
                            if activated {
                                return Ok(public_diagnostic.semantic_root_focus);
                            }
                            public_diagnostic
                        }
                        Err(_) => {
                            let mut diagnostic = error.details.as_ref()
                                .and_then(|details| details.foreground_activation.clone())
                                .expect("retained native foreground refusal");
                            diagnostic.semantic_root_focus = Some(ComputerUseSemanticRootFocusDiagnostic {
                                    status: ComputerUseSemanticRootFocusStatus::DispatchUnknown,
                                    focus_attempted: Some(true),
                                    provider_completed: false,
                                    foreground_observed: None,
                                });
                            diagnostic
                        }
                    };
                    let error = if dcc_cua_interrupt::interrupt_generation() != activation_generation {
                        ComputerUseError::new(ComputerUseErrorCode::UserInterrupted, "root activation was interrupted")
                    } else {
                        error
                    };
                    Err(crate::semantic_root_activation::with_semantic_root_activation_diagnostic(error, diagnostic))
                },
            ).await?
        } else {
            None
        };
        Ok(root_focus_diagnostic)
    }
}
