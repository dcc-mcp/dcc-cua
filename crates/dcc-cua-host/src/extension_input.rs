use std::time::{Duration, Instant};

use crate::action_confirmation::{
    ActionConfirmationOutcome, ConfirmationBinding, ConfirmationWindowIdentity,
    TrustedActionConfirmationRequest,
};
use crate::task_authorization::authorize_window_confirmation;
use crate::{HostError, HostSession};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) struct ExtensionObservation {
    pub provider_id: String,
    pub origin: String,
    pub snapshot: Value,
    pub observed_at: Instant,
}

impl ExtensionObservation {
    pub(super) fn new(provider_id: &str, origin: &str, snapshot: Value) -> Result<Self, HostError> {
        let refs = snapshot["refs"]
            .as_array()
            .ok_or_else(|| HostError::Protocol("extension snapshot omitted refs".into()))?;
        if refs.len() > 300
            || snapshot["document"]["origin"].as_str() != Some(origin)
            || snapshot["snapshot_id"]
                .as_str()
                .is_none_or(|id| id.is_empty() || id.len() > 128)
            || snapshot["document"]["id"]
                .as_str()
                .is_none_or(|id| id.is_empty())
        {
            return Err(HostError::Protocol(
                "invalid extension snapshot identity".into(),
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for node in refs {
            let id = node["ref"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 128)
                .ok_or_else(|| HostError::Protocol("invalid extension ref".into()))?;
            if !ids.insert(id) {
                return Err(HostError::Protocol("duplicate extension ref".into()));
            }
        }
        Ok(Self {
            provider_id: provider_id.into(),
            origin: origin.into(),
            snapshot,
            observed_at: Instant::now(),
        })
    }

    pub(super) fn action(
        &self,
        provider: &str,
        origin: &str,
        method: &str,
        params: &Value,
    ) -> Result<Value, HostError> {
        if self.provider_id != provider
            || self.origin != origin
            || self.observed_at.elapsed() > Duration::from_secs(30)
            || params["snapshot_id"] != self.snapshot["snapshot_id"]
        {
            return Err(HostError::Protocol(
                "stale extension observation; take a fresh snapshot".into(),
            ));
        }
        if method == "navigate" {
            let object = params
                .as_object()
                .filter(|object| object.len() == 2)
                .ok_or_else(|| {
                    HostError::Protocol("navigation accepts only snapshot_id and url".into())
                })?;
            let target = object
                .get("url")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty() && value.len() <= 2048)
                .ok_or_else(|| HostError::Protocol("navigation requires a bounded URL".into()))?;
            let url = url::Url::parse(target)
                .map_err(|_| HostError::Protocol("navigation requires an absolute URL".into()))?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.origin().ascii_serialization() != origin
            {
                return Err(HostError::Protocol(
                    "navigation must remain in the exact allowed origin without credentials".into(),
                ));
            }
            return Ok(
                json!({"action":"browser_extension_navigate","input_kind":"browser",
                "authorization_category":"external_effect","browser_origin":origin,
                "snapshot_id":params["snapshot_id"],"provider_id":provider,
                "target_url_sha256":format!("{:x}",Sha256::digest(target.as_bytes()))}),
            );
        }
        let node = self.snapshot["refs"]
            .as_array()
            .and_then(|refs| refs.iter().find(|node| node["ref"] == params["ref"]))
            .ok_or_else(|| {
                HostError::Protocol("extension ref was not observed by this task".into())
            })?;
        if !node["actions"]
            .as_array()
            .is_some_and(|actions| actions.iter().any(|action| action == method))
            || node["visibility"] != "in_viewport"
            || node["states"]["disabled"] != false
        {
            return Err(HostError::Protocol(
                "extension control is not actionable".into(),
            ));
        }
        if method == "type"
            && (node["states"]["sensitive"] != false
                || params["replace"] != true
                || params["text"]
                    .as_str()
                    .is_none_or(|text| text.chars().count() > 4096))
        {
            return Err(HostError::Protocol(
                "extension typing requires an explicitly non-sensitive control and bounded text"
                    .into(),
            ));
        }
        Ok(
            json!({"action":method,"input_kind":"semantic","authorization_category":if method == "click" { "external_effect" } else { "content_change" },"ref":params["ref"],"snapshot_id":params["snapshot_id"],"provider_id":provider,"browser_origin":origin,"text_sha256":params["text"].as_str().map(|text|format!("{:x}",Sha256::digest(text.as_bytes())))}),
        )
    }
}

pub(super) async fn authorize(
    host: &HostSession,
    services: &crate::HostSecurityServices,
    session_id: &str,
    action: Value,
    snapshot_id: &str,
) -> Result<(), HostError> {
    let confirmation = TrustedActionConfirmationRequest::for_bound_window_action_value(
        ConfirmationBinding::window(
            session_id,
            &host.task_grant_id,
            &host.capability,
            ConfirmationWindowIdentity {
                process_id: host.target_process_id,
                window_handle: host.target_window_handle,
            },
            snapshot_id,
            None,
        ),
        "browser_extension_input",
        action,
    )?;
    let outcome = authorize_window_confirmation(services, host, confirmation).await;
    if outcome == ActionConfirmationOutcome::Allowed {
        return Ok(());
    }
    let refusal = crate::request_contract::action_confirmation_refusal(outcome).0;
    Err(HostError::Protocol(
        refusal["error"]
            .as_str()
            .unwrap_or("task_authorization_required")
            .into(),
    ))
}
