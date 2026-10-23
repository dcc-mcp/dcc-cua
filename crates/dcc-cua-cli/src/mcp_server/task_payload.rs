//! Public task attestation, including independent owner ceilings.
use super::*;

pub(super) fn proposal_payload(proposal_id: &str, proposal: &TaskProposal, status: &str) -> Value {
    let target = match proposal.registration.target {
        TrustedTaskAuthorizationTarget::ExactWindow {
            process_id,
            window_handle,
        } => json!({
            "kind": "exact_window",
            "process_id": process_id,
            "window_handle": window_handle,
        }),
        TrustedTaskAuthorizationTarget::OwnedBrowser(launch) => json!({
            "kind": "owned_browser",
            "browser": launch.browser,
            "profile": launch.profile,
            "process_id": Value::Null,
            "window_handle": Value::Null,
            "derived_on_start": true,
        }),
    };
    json!({
        "ok": !matches!(status, "cleanup_failed" | "cleanup_unknown"),
        "provider": "dcc-cua",
        "runtime_version": env!("CARGO_PKG_VERSION"),
        "task_id": proposal_id,
        "status": status,
        "application_label": proposal.registration.application_label,
        "surface": proposal.surface.as_str(),
        "observation_mode": proposal.observation_mode,
        "target": target,
        "allowed_methods": proposal.allowed_methods,
        "allowed_actions": proposal.registration.allowed_actions,
        "allowed_browser_origins": proposal.registration.allowed_browser_origins,
        "expires_at_unix_ms": proposal.registration.expires_at_unix_ms,
        "confirmation_required": false,
        "allow_native_capture_popup_keyboard": proposal.registration.allowed_actions.iter().any(TrustedTaskActionScope::is_native_capture_popup_keyboard),
        "native_action_popups": false,
        "secrets_accepted": false,
        "recording_output_dir": proposal.registration.recording_output_dir,
        "cleanup": proposal.cleanup,
    })
}
