//! Fixed, content-free lifetime guidance; never infer ownership on another connection.

use serde_json::{Value, json};

pub(crate) const TASK_NOT_FOUND: &str = "Task not found on this MCP connection. Keep the same mcp-server stdio connection open from start_task through stop_task. Task IDs cannot be resumed on another connection; if the owning connection closed, start a new task and take a fresh observation.";
pub(crate) const TASK_STOPPED: &str = "Task was stopped. Start a new task on this MCP connection and take a fresh observation; stopped task IDs and references cannot be reused.";
pub(crate) const TASK_EXPIRED: &str = "Task expired. Start a new task on this MCP connection and take a fresh observation; expired task IDs and references cannot be reused.";

pub(crate) fn task_failure(message: &str) -> Option<Value> {
    let code = match message {
        TASK_NOT_FOUND => "task_not_found",
        TASK_STOPPED => "task_stopped",
        TASK_EXPIRED => "task_expired",
        _ => return None,
    };
    Some(json!({
        "ok": false,
        "error": message,
        "message": message,
        "code": code,
        "error_code": code,
        "details": {"connection_scoped": true, "cross_connection_resume": false},
    }))
}

/// Match only closed codes and replace all remote text/details with local guidance.
pub(crate) fn one_shot_failure(code: &str) -> Option<Value> {
    let (code, message) = match code {
        "stale_observation" => (
            "stale_observation",
            "The observation or reference is no longer valid for this session. Take a fresh snapshot in the same live task before acting. Opaque element tokens cannot be reused across CLI invocations or reconnected MCP sessions; do not blindly retry input.",
        ),
        "session_not_found" => (
            "session_not_found",
            "No session with this ID exists on this Host connection. Sessions and capabilities belong to the connection that opened them. Keep one mcp-server connection open for start_task and dcc_cua_task_call, or keep an authorized Host client connection open for open_session and subsequent calls. A running resident Host does not preserve sessions across host-call invocations.",
        ),
        "invalid_request" => (
            "invalid_request",
            "The Host rejected the request method or parameters. host-call uses Host IPC schemas, not the driver tools listed by dcc-cua tools: start_session is not a Host IPC method, and get_window_state requires session_id, task_grant_id and window_capability from open_session on the same connection. For agent automation use start_task and dcc_cua_task_call on one persistent mcp-server connection.",
        ),
        "task_authorization_required" => (
            "task_authorization_required",
            "This request requires task authorization from the embedding host. host-ensure only starts a resident Host; it does not issue task grants. Use start_task and dcc_cua_task_call on one persistent mcp-server connection, or the trusted embedding authorization API.",
        ),
        "task_authorization_denied" => (
            "task_authorization_denied",
            "The request is outside its task authorization. Check the exact target, allowed methods and actions on the owning connection; do not widen scope or retry with another session's capability.",
        ),
        "task_authorization_expired" => (
            "task_authorization_expired",
            "Task authorization expired. Start a new authorized task and take a fresh observation on the same live connection before acting.",
        ),
        "task_authorization_revoked" => (
            "task_authorization_revoked",
            "Task authorization was revoked. Start a new authorized task and take a fresh observation before acting; the revoked lease cannot be resumed.",
        ),
        _ => return None,
    };
    Some(json!({"success": false, "error": {"code": code, "message": message}}))
}

pub(crate) fn manifest_contract() -> Value {
    json!({
        "mcp_command": ["mcp-server"],
        "task_id_scope": "mcp_stdio_connection",
        "host_session_scope": "host_connection",
        "observation_scope": "owning_session_latest_valid_observation",
        "opaque_references_portable": false,
        "cross_connection_resume": false,
        "disconnect": "cleanup_owned_tasks_and_sessions",
        "reconnect": "start_new_task_then_take_fresh_observation",
        "one_shot_cli": {
            "invocations_share_session": false,
            "act_captures_fresh_observation": true,
            "element_index_resolved_against": "fresh_in_command_snapshot",
            "opaque_tokens_from_previous_invocation_accepted": false,
        },
        "host_call": {
            "connection": "new_per_invocation",
            "method_namespace": "host_ipc_not_driver_tools",
            "host_ensure_issues_task_grants": false,
        },
        "recommended_loop": ["start_task", "dcc_cua_task_call(snapshot)", "dcc_cua_task_call(execute_action)", "stop_task"],
        "guidance": "Keep one mcp-server stdio connection open for the entire task, including observations, actions and stop_task. Reconnection requires a new task and fresh observation; a resident Host or repeated session label does not resume old state.",
    })
}
