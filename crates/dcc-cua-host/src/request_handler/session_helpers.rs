use super::*;

pub(crate) async fn acquire_raw_input_turn(
    enabled: bool,
) -> Option<tokio::sync::MutexGuard<'static, ()>> {
    if enabled {
        Some(RAW_INPUT_QUEUE.lock().await)
    } else {
        None
    }
}

pub(crate) fn ensure_connection_session_capacity(
    active_session_count: usize,
) -> Result<(), HostError> {
    if active_session_count < MAX_SESSIONS_PER_CONNECTION {
        return Ok(());
    }
    Err(HostError::coded_protocol(
        HostProtocolErrorCode::SessionLimitReached,
        format!(
            "connection session limit reached (maximum {})",
            MAX_SESSIONS_PER_CONNECTION
        ),
    ))
}

pub(crate) fn denied(code: HostProtocolErrorCode, capability: &'static str) -> HostError {
    HostError::coded_protocol(code, format!("{capability} is not granted"))
}

pub(crate) fn finish_window_mutation_attempt<T, E>(
    result: Result<T, E>,
    invalidate: impl FnOnce(),
) -> Result<T, E> {
    invalidate();
    result
}

pub(crate) fn session_stopped_response(
    session_id: &str,
    result: ComputerUseSessionStopResult,
) -> Value {
    json!({
        "type": "session_stopped",
        "session_id": session_id,
        "success": result.success,
        "active": result.active,
        "cleanup_pending": result.cleanup_pending,
        "cleanup_issues": result.cleanup_issues,
        "marker": result.marker,
    })
}

pub(crate) fn observed_window_state_response(
    host: &mut HostSession,
    session_id: &str,
    state: Value,
) -> Value {
    host.observe_target_state(&state);
    json!({"type":"window_state", "session_id":session_id, "state":state})
}

pub(crate) fn finish_target_sensitive_cached_read(
    host: &mut HostSession,
    availability: ComputerUseResult<dcc_cua_core::ComputerUseTargetAvailability>,
) -> ComputerUseResult<()> {
    let availability = host.finish_observation_sensitive_attempt(availability)?;
    let status = availability.status;
    host.observe_target_availability(availability);
    if host.input_events.current().status == dcc_cua_core::ComputerUseInputStatus::Suspended {
        return Err(cached_target_pre_dispatch_refusal(
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
            host.input_events
                .current()
                .reason
                .clone()
                .unwrap_or_else(|| "interactive desktop is unavailable".into()),
        ));
    }
    match status {
        dcc_cua_core::ComputerUseTargetStatus::Available => Ok(()),
        dcc_cua_core::ComputerUseTargetStatus::Minimized => {
            Err(cached_target_pre_dispatch_refusal(
                ComputerUseErrorCode::TargetMinimized,
                "the exact target is minimized and cached accessibility evidence is stale",
            ))
        }
        dcc_cua_core::ComputerUseTargetStatus::Unavailable => {
            Err(cached_target_pre_dispatch_refusal(
                ComputerUseErrorCode::TargetUnavailable,
                "the exact target is unavailable and cached accessibility evidence is stale",
            ))
        }
    }
}

pub(crate) fn cached_target_pre_dispatch_refusal(
    code: ComputerUseErrorCode,
    message: impl Into<String>,
) -> ComputerUseError {
    ComputerUseError::new(code, message).with_details(dcc_cua_core::ComputerUseErrorDetails {
        phase: Some(dcc_cua_core::ComputerUseErrorPhase::PreDispatch),
        action_attempted: Some(false),
        input_sent: Some(dcc_cua_core::ComputerUseInputState::NotSent),
        completion: Some(dcc_cua_core::ComputerUseCompletionState::Known),
        effect_unknown: Some(false),
        local_session_invalidated: Some(false),
        session_remains_active: Some(true),
        automatic_input: Some(false),
        blind_retry: Some(false),
        fresh_observation_required: Some(true),
        ..Default::default()
    })
}

pub(crate) fn bind_launched_process(
    launched: &HostLaunchSession,
    grant: &mut TaskGrant,
) -> Result<(), HostError> {
    if grant.task_grant_id != launched.task_grant_id
        || grant.application_label != launched.application_label
    {
        return Err(HostError::Protocol(
            "launch and window session grants do not match".into(),
        ));
    }
    if grant
        .process_id
        .is_some_and(|process_id| process_id != launched.process_id)
    {
        return Err(HostError::Protocol(
            "window session does not target the launched process".into(),
        ));
    }
    grant.process_id = Some(launched.process_id);
    Ok(())
}

pub(crate) fn restore_activate_available(grant: &TaskGrant) -> bool {
    cfg!(windows) && grant.process_id.is_some() && grant.window_handle.is_some()
}

pub(crate) fn session_health_state_changed(
    evidence_epoch_before: dcc_cua_core::ActionEvidenceEpoch,
    transition_sequence_before: u64,
    action_evidence_epoch: dcc_cua_core::ActionEvidenceEpoch,
    transition_sequence: u64,
) -> bool {
    evidence_epoch_before != action_evidence_epoch
        || transition_sequence_before != transition_sequence
}

pub(crate) async fn refresh_session_health_input_and_target(host: &mut HostSession) -> bool {
    host.refresh_input_readiness();
    let availability = host.session.target_availability().await;
    let target_probe_failed = availability.is_err();
    if let Ok(availability) = host.finish_observation_sensitive_attempt(availability) {
        host.observe_target_availability(availability);
    }
    target_probe_failed
}
