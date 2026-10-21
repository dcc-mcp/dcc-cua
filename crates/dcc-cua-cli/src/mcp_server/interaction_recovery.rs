use super::{TaskProposal, project_task_remote_error, remote_error_field, tool_error, tool_result};
use dcc_cua_client::HostClientError;
use dcc_cua_core::ComputerUseForegroundActivationDiagnostic;
use dcc_cua_host::{TaskObservationMode, TrustedTaskActionScope, TrustedTaskAuthorizationTarget};
use serde_json::{Value, json};

pub(super) fn task_remote_error(
    error: &HostClientError,
    method: &str,
    task_id: &str,
    proposal: &TaskProposal,
) -> Value {
    let Some(projected) = project_task_remote_error(error, method) else {
        return tool_error(error.to_string());
    };
    let session = proposal
        .session
        .as_ref()
        .expect("task session was initialized");
    let mut result = tool_result(
        serde_json::to_value(projected).expect("typed remote error projection must serialize"),
    );
    let payload = &mut result["structuredContent"];
    annotate(payload, method, proposal);
    payload["task_context"] = json!({
        "provider": "dcc-cua",
        "runtime_version": env!("CARGO_PKG_VERSION"),
        "task_id": task_id,
        "target": {
            "process_id": session.target()["process_id"],
            "window_handle": session.target()["window_handle"],
        },
        "native_action_popups": false,
    });
    result["content"] = json!([{"type":"text", "text": result["structuredContent"].to_string()}]);
    result
}

pub(super) fn annotate(host: &mut Value, method: &str, proposal: &TaskProposal) {
    if proposal.observation_mode != TaskObservationMode::PixelsOnly {
        return;
    }
    let TrustedTaskAuthorizationTarget::ExactWindow {
        process_id,
        window_handle,
    } = proposal.registration.target
    else {
        return;
    };
    let (foreground, refused) = match method {
        "get_window_state" => {
            let state = &host["state"];
            if host["type"] != "window_state"
                || state["process_id"].as_u64() != Some(u64::from(process_id))
                || state["window_handle"].as_u64() != Some(window_handle)
                || state["exists"].as_bool() != Some(true)
                || state["visible"].as_bool() != Some(true)
                || state["minimized"].as_bool() != Some(false)
            {
                return;
            }
            let Some(foreground) = state["foreground"].as_bool() else {
                return;
            };
            (Some(foreground), false)
        }
        "change_window_state"
            if host["code"] == "foreground_activation_refused" && host["ok"] == false =>
        {
            let Some(diagnostic) = remote_error_field::<ComputerUseForegroundActivationDiagnostic>(
                &host["details"],
                "foreground_activation",
            ) else {
                return;
            };
            if diagnostic.target.process_id != process_id
                || diagnostic.target.window_handle != window_handle
            {
                return;
            }
            (
                diagnostic.final_foreground.map(|target| {
                    target.process_id == process_id && target.window_handle == window_handle
                }),
                true,
            )
        }
        _ => return,
    };
    let authorized = |method| {
        proposal
            .allowed_methods
            .iter()
            .any(|allowed| allowed == method)
    };
    let input = authorized("execute_action")
        && proposal
            .registration
            .allowed_actions
            .iter()
            .any(TrustedTaskActionScope::is_pixels_input);
    let activation = authorized("change_window_state")
        && !proposal
            .registration
            .allowed_actions
            .iter()
            .any(TrustedTaskActionScope::is_native_menu_dismiss);
    // Core viability hints describe other routes too; this closed bridge only delivers foreground input.
    host["interaction_recovery"] = json!({
        "observation": {
            "snapshot_authorized": authorized("snapshot"),
            "foreground_required": false,
            "fresh_capture_proof_required": true,
        },
        "input": {
            "execute_action_authorized": input,
            "foreground_observed": foreground,
            "foreground_required": true,
            "background_delivery_supported": false,
            "fresh_observation_required": true,
            "readiness_attested": false,
        },
        "activation": {
            "method_authorized": activation,
            "status": if refused { "refused" } else { "history_unknown" },
            "prior_refusal_overridden": false,
            "automatic_retry": false,
            "blind_retry": false,
            "next_step": if refused { "diagnose_native_refusal" }
                else if !input { "observation_only" }
                else if foreground == Some(true) { "fresh_observation_before_input" }
                else if !activation { "activation_scope_required" }
                else { "assess_input_handoff" },
        },
    });
}
