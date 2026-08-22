use super::*;

pub(super) fn parse_bound_tool_value(
    name: &str,
    result: &cua_driver_sdk::ToolResult,
) -> ComputerUseResult<Value> {
    serde_json::from_str(&result.raw_json).map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            format!("CUA {name} returned invalid JSON: {error}"),
        )
    })
}

pub(super) fn mutation_pre_dispatch_failure(
    context: &str,
    error: cua_driver_sdk::DriverError,
) -> ComputerUseError {
    pre_dispatch_failure(map_driver_error(context, error))
}

pub(super) fn pre_dispatch_failure(error: ComputerUseError) -> ComputerUseError {
    ComputerUseError::new(error.code, error.message).with_details(ComputerUseErrorDetails {
        phase: Some(ComputerUseErrorPhase::PreDispatch),
        action_attempted: Some(false),
        input_sent: Some(ComputerUseInputState::NotSent),
        completion: Some(ComputerUseCompletionState::Known),
        local_session_invalidated: Some(false),
        session_remains_active: Some(true),
        automatic_input: Some(false),
        blind_retry: Some(false),
        fresh_observation_required: Some(false),
        ..Default::default()
    })
}

pub(super) fn mutation_known_failure(
    context: &str,
    error: cua_driver_sdk::DriverError,
) -> ComputerUseError {
    let error = map_driver_error(context, error);
    ComputerUseError::new(error.code, error.message).with_details(ComputerUseErrorDetails {
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
    })
}

#[cfg(any(windows, test))]
pub(super) fn local_mutation_attempt_failure(error: ComputerUseError) -> ComputerUseError {
    ComputerUseError::new(error.code, error.message).with_details(ComputerUseErrorDetails {
        phase: Some(ComputerUseErrorPhase::LocalMutationDispatch),
        action_attempted: Some(true),
        input_sent: Some(ComputerUseInputState::Unknown),
        completion: Some(ComputerUseCompletionState::Known),
        effect_unknown: Some(true),
        local_session_invalidated: Some(false),
        session_remains_active: Some(true),
        automatic_input: Some(false),
        blind_retry: Some(false),
        fresh_observation_required: Some(true),
        ..Default::default()
    })
}

#[cfg(any(windows, test))]
pub(super) fn local_activation_attempt_failure(error: ComputerUseError) -> ComputerUseError {
    ComputerUseError::new(error.code, error.message).with_details(ComputerUseErrorDetails {
        phase: Some(ComputerUseErrorPhase::ActivationDispatch),
        focus_mutation_attempted: Some(true),
        action_attempted: Some(false),
        input_sent: Some(ComputerUseInputState::NotSent),
        completion: Some(ComputerUseCompletionState::Known),
        effect_unknown: Some(false),
        local_session_invalidated: Some(false),
        session_remains_active: Some(true),
        automatic_input: Some(false),
        blind_retry: Some(false),
        fresh_observation_required: Some(true),
        ..Default::default()
    })
}

#[cfg(any(windows, test))]
pub(super) fn local_activation_validation_failure(
    code: ComputerUseErrorCode,
    message: impl Into<String>,
) -> ComputerUseError {
    ComputerUseError::new(code, message).with_details(ComputerUseErrorDetails {
        phase: Some(ComputerUseErrorPhase::ActivationDispatch),
        focus_mutation_attempted: Some(true),
        action_attempted: Some(false),
        input_sent: Some(ComputerUseInputState::NotSent),
        completion: Some(ComputerUseCompletionState::Known),
        effect_unknown: Some(false),
        automatic_input: Some(false),
        blind_retry: Some(false),
        fresh_observation_required: Some(true),
        ..Default::default()
    })
}
