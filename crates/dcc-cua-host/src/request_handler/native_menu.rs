use super::*;

pub(crate) fn validate_window_operation_scope(
    scopes: &[TrustedTaskActionScope],
    operation: &WindowOperation,
) -> Result<(), HostError> {
    if scopes
        .iter()
        .any(TrustedTaskActionScope::is_native_menu_dismiss)
        && !matches!(operation, WindowOperation::DismissMenu)
    {
        return Err(HostError::coded_protocol(
            HostProtocolErrorCode::TaskAuthorizationDenied,
            "a dismiss_menu action scope cannot authorize activate, restore_activate or close",
        ));
    }
    Ok(())
}

pub(super) fn validate_other_operation(
    host: &mut HostSession,
    operation: &WindowOperation,
    state_id: Option<&str>,
) -> Result<(), HostError> {
    if let Err(error) = validate_window_operation_scope(
        host.task_authorization
            .as_ref()
            .map(|lease| lease.allowed_actions.as_slice())
            .unwrap_or_default(),
        operation,
    ) {
        host.session.invalidate_action_observations();
        host.invalidate_observations();
        return Err(error);
    }
    if state_id.is_some() {
        return Err(HostError::Protocol(
            "window_state_id is accepted only for dismiss_menu in change_window_state".into(),
        ));
    }
    Ok(())
}

pub(super) async fn dismiss_menu(
    host: &mut HostSession,
    session_id: &str,
    state_id: Option<&str>,
    mutation_generation: u64,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let prior_epoch = host.session.action_evidence_epoch();
    let result = async {
        host.require_native_menu_dismiss_grant(session_id)?;
        let id = state_id
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && id.trim() == *id
                    && !id.chars().any(char::is_control)
            })
            .ok_or_else(|| {
                HostError::ComputerUse(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "dismiss_menu requires a fresh purpose-bound menu window_state_id",
                ))
            })?;
        let _input_turn = RAW_INPUT_QUEUE.lock().await;
        revalidate_queued_window_mutation(host).await?;
        ensure_session_not_interrupted(host).await?;
        if interrupt_generation() != mutation_generation {
            return Err(HostError::ComputerUse(ComputerUseError::new(
                ComputerUseErrorCode::UserInterrupted,
                "native menu dismissal was interrupted while awaiting its trusted queue or lease",
            )));
        }
        let expires_at = host.require_native_menu_dismiss_grant(session_id)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let remaining = u128::from(expires_at)
            .checked_sub(now)
            .filter(|remaining| *remaining > 0)
            .and_then(|remaining| u64::try_from(remaining).ok())
            .ok_or_else(|| {
                HostError::coded_protocol(
                    HostProtocolErrorCode::TaskAuthorizationDenied,
                    "native menu task authorization expired",
                )
            })?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(remaining);
        let result = host
            .session
            .dismiss_native_menu_from_state(id, deadline)
            .await;
        host.finish_observation_sensitive_attempt(result)
            .map_err(HostError::from)
    }
    .await
    .map_err(native_frame_pre_dispatch_failure);
    if host.session.action_evidence_epoch() == prior_epoch {
        host.session.invalidate_action_observations();
    }
    host.invalidate_observations();
    let result = result?;
    // The granted target is the popup. Its absence proves no application close;
    // Core returns only the independent native menu and owner readback.
    Ok((
        json!({"type":"window_state_changed","session_id":session_id,"operation":"dismiss_menu","result":result}),
        None,
    ))
}
