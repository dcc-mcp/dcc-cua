use super::*;

pub(super) fn annotate(response: &mut Value, task_id: &str, method: &str, proposal: &TaskProposal) {
    if method == "get_window_state" && proposal.observation_mode == TaskObservationMode::PixelsOnly
    {
        add_frame_recovery_guidance(
            response,
            task_id,
            proposal
                .allowed_methods
                .iter()
                .any(|method| method == "change_window_state"),
            proposal
                .registration
                .allowed_actions
                .iter()
                .any(TrustedTaskActionScope::is_native_menu_dismiss),
        );
    }
}

pub(super) fn add_frame_recovery_guidance(
    response: &mut Value,
    task_id: &str,
    has_restore_method: bool,
    menu_only: bool,
) {
    let state = &response["state"];
    if state.get("window_state_id").is_some() {
        return;
    }
    let reason = if state["minimized"] == true {
        "target_minimized"
    } else if state["maximized"] == true {
        "target_maximized"
    } else {
        return;
    };
    let mut recovery = json!({
        "reason":reason, "fresh_window_state_required_after_restore":true,
        "automatic_input":false,
    });
    if has_restore_method && !menu_only {
        recovery["next_call"] = json!({"tool":"dcc_cua_task_call", "arguments":{
            "task_id":task_id, "method":"change_window_state", "params":{"operation":"restore_activate"},
        }});
    } else {
        recovery["requires_new_task"] = json!(true);
        if !has_restore_method {
            recovery["required_method"] = json!("change_window_state");
        }
        if menu_only {
            recovery["blocking_action_scope"] = json!("dismiss_menu");
            recovery["omit_action_scope"] = json!("dismiss_menu");
        }
    }
    response["frame_recovery"] = recovery;
}
