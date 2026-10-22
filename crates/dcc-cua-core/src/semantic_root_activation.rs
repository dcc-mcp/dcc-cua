use crate::{ComputerUseErrorCode, ComputerUseObservation, window_target::WindowTarget};

pub(crate) fn semantic_root_activation_allowed(
    pixels_only: bool,
    observation: Option<&ComputerUseObservation>,
    target: &WindowTarget,
    context_matches: bool,
    native_error: &ComputerUseErrorCode,
) -> bool {
    !pixels_only
        && context_matches
        && *native_error == ComputerUseErrorCode::ForegroundActivationRefused
        && observation.is_some_and(|observation| {
            observation.process_id == target.pid
                && observation.window_handle == target.window_id
                && observation.capture_provenance["accessibility_backend"] == "windows_uia"
                && observation.capture_provenance["accessibility_available"] == true
        })
}

pub(crate) async fn recover_semantic_root_activation<T, Recover, Output>(
    error: crate::ComputerUseError,
    pixels_only: bool,
    observation: Option<&ComputerUseObservation>,
    target: &WindowTarget,
    context_matches: bool,
    recover: Recover,
) -> crate::ComputerUseResult<T>
where
    Recover: FnOnce(crate::ComputerUseError) -> Output,
    Output: std::future::Future<Output = crate::ComputerUseResult<T>>,
{
    if semantic_root_activation_allowed(
        pixels_only,
        observation,
        target,
        context_matches,
        &error.code,
    ) {
        recover(error).await
    } else {
        Err(error)
    }
}

pub(crate) fn with_semantic_root_activation_diagnostic(
    mut error: crate::ComputerUseError,
    diagnostic: crate::ComputerUseForegroundActivationDiagnostic,
) -> crate::ComputerUseError {
    let unknown = diagnostic
        .semantic_root_focus
        .as_ref()
        .is_some_and(|focus| {
            matches!(
                focus.status,
                crate::ComputerUseSemanticRootFocusStatus::DispatchUnknown
                    | crate::ComputerUseSemanticRootFocusStatus::PostconditionFailed
            ) || (focus.focus_attempted == Some(true) && !focus.provider_completed)
        });
    let details = error.details.get_or_insert_with(Default::default);
    details.foreground_activation = Some(diagnostic);
    details.phase = Some(crate::ComputerUseErrorPhase::ActivationDispatch);
    details.focus_mutation_attempted = Some(true);
    details.action_attempted = Some(false);
    details.input_sent = Some(crate::ComputerUseInputState::NotSent);
    details.completion = Some(if unknown {
        crate::ComputerUseCompletionState::Unknown
    } else {
        crate::ComputerUseCompletionState::Known
    });
    details.effect_unknown = Some(unknown);
    details.automatic_input = Some(false);
    details.blind_retry = Some(false);
    details.fresh_observation_required = Some(true);
    error
}

#[cfg(test)]
mod tests;
