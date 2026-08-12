use super::*;

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
