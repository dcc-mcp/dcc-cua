use super::*;

pub(super) fn window_state_changed_response(
    session_id: &str,
    operation: &str,
    state: Value,
    result: Value,
) -> Value {
    json!({
        "type": "window_state_changed",
        "session_id": session_id,
        "operation": operation,
        "state": state,
        "result": result,
    })
}

pub(super) fn take_connection_session<T>(
    sessions: &mut std::collections::HashMap<String, T>,
    session_id: &str,
) -> Result<T, HostError> {
    sessions
        .remove(session_id)
        .ok_or_else(|| HostError::Protocol("session not found".into()))
}

pub(super) fn input_target_from_cua(
    session_id: &str,
    target: &Value,
) -> Result<dcc_cua_core::ComputerUseInputTarget, HostError> {
    let process_id = target["pid"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| HostError::Protocol("CUA target PID is invalid".into()))?;
    let window_handle = target["window_id"]
        .as_u64()
        .ok_or_else(|| HostError::Protocol("CUA target window handle is invalid".into()))?;
    Ok(dcc_cua_core::ComputerUseInputTarget {
        session_id: session_id.to_owned(),
        process_id,
        window_handle,
    })
}

pub(super) fn session_events_response(
    session_id: &str,
    page: dcc_cua_core::ComputerUseSessionEventsPage,
) -> Result<Value, HostError> {
    let mut response =
        serde_json::to_value(page).map_err(|error| HostError::Protocol(error.to_string()))?;
    response["type"] = Value::String("session_events".into());
    response["session_id"] = Value::String(session_id.to_owned());
    Ok(response)
}

pub(super) const fn cursor_render_backend(upstream_cursor_renderer_enabled: bool) -> &'static str {
    if upstream_cursor_renderer_enabled {
        "cua-driver-sdk"
    } else {
        "unavailable"
    }
}

pub(super) fn post_snapshot_delay(
    capture_after: bool,
    delay_ms: u64,
) -> Result<std::time::Duration, HostError> {
    if delay_ms > MAX_POST_SNAPSHOT_DELAY_MS {
        return Err(HostError::ComputerUse(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            format!("post_snapshot_delay_ms must be at most {MAX_POST_SNAPSHOT_DELAY_MS}"),
        )));
    }
    if !capture_after && delay_ms != 0 {
        return Err(HostError::ComputerUse(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "post_snapshot_delay_ms requires capture_after",
        )));
    }
    Ok(std::time::Duration::from_millis(delay_ms))
}

pub(super) fn poll_session_events_timeout(
    timeout_ms: u64,
) -> Result<std::time::Duration, HostError> {
    if timeout_ms > MAX_SESSION_EVENT_POLL_TIMEOUT_MS {
        return Err(HostError::Protocol(format!(
            "timeout_ms must be at most {MAX_SESSION_EVENT_POLL_TIMEOUT_MS}"
        )));
    }
    Ok(std::time::Duration::from_millis(timeout_ms))
}
