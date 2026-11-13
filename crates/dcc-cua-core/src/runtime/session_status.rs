use super::*;

/// Driver 0.49 reports lifecycle through get_session. DCC's capture policy is
/// fixed to window when it starts that exact session; it never grants desktop
/// scope. Keep those Host facts distinct from the upstream lifecycle receipt.
pub(super) fn window_session_state(
    mut lifecycle: Value,
    session: &str,
) -> ComputerUseResult<Value> {
    let state = lifecycle["structuredContent"]
        .as_object_mut()
        .ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "CUA get_session did not return a structured lifecycle receipt",
            )
        })?;
    if state.get("session").and_then(Value::as_str) != Some(session)
        || state.get("state").and_then(Value::as_str) != Some("active")
        || state
            .get("active")
            .is_some_and(|value| value.as_bool() != Some(true))
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetUnavailable,
            "CUA lifecycle receipt does not prove this exact active session",
        ));
    }
    for key in ["capture_scope", "effective_scope"] {
        if state
            .get(key)
            .is_some_and(|value| value.as_str() != Some("window"))
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::TargetUnavailable,
                "CUA lifecycle receipt contradicts the Host's exact-window policy",
            ));
        }
    }
    for key in ["desktop_capture_authorized", "desktop_unlocked"] {
        if state
            .get(key)
            .is_some_and(|value| value.as_bool() != Some(false))
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::TargetUnavailable,
                "CUA lifecycle receipt contradicts the Host's desktop-scope refusal",
            ));
        }
    }
    state.insert("active".into(), json!(true));
    state.insert("capture_scope".into(), json!("window"));
    state.insert("effective_scope".into(), json!("window"));
    state.insert("desktop_capture_authorized".into(), json!(false));
    state.insert("desktop_unlocked".into(), json!(false));
    state.insert(
        "capture_scope_source".into(),
        json!("host_exact_window_binding"),
    );
    Ok(lifecycle)
}

impl ComputerUseSession {
    pub fn status(&self) -> Value {
        json!({
            "active": self.active,
            "escalated": self.escalated,
            "session_id": self.session_id,
            "target": self.target,
            "banner": self.banner_status(),
            "marker": self.marker,
            "latest_observation_id": self.observation.as_ref().map(|value| &value.observation_id),
            "backend": "cua-driver-sdk",
            "upstream_session": self.upstream_session_status(),
        })
    }

    pub(super) fn upstream_session_status(&self) -> Value {
        match &self.upstream_session_state {
            UpstreamSessionState::Inactive => json!({"state": "inactive", "degraded": false}),
            UpstreamSessionState::Active => json!({"state": "active", "degraded": false}),
            #[cfg(target_os = "linux")]
            UpstreamSessionState::LinuxPixelsOnly => json!({
                "state": "pixels_only", "platform": "linux_x11", "read_only": true,
                "degraded": false, "accessibility_provider_started": false,
                "requires_explicit_escalation": false, "scope": "exact_window",
            }),
            #[cfg(windows)]
            UpstreamSessionState::VisualOnly { reason } => json!({
                "state": "visual_only",
                "degraded": true,
                "reason": reason,
                "requires_explicit_escalation": true,
                "scope": "exact_window",
            }),
        }
    }
}
