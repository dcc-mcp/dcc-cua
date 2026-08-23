use dcc_cua_core::{COMPUTER_USE_ESCALATION_REASONS, MAX_ESCALATION_DETAIL_CHARS};
use dcc_cua_host::{
    DEFAULT_SESSION_IDLE_TIMEOUT_MS, HOST_HELLO_TIMEOUT_MS, HOST_PROTOCOL_VERSION, HostTransport,
    MAX_APPLICATION_LABEL_CHARS, MAX_BINARY_FRAME_BYTES, MAX_HOST_CONNECTIONS,
    MAX_JSON_FRAME_BYTES, MAX_PARALLEL_DISCOVERY_REQUESTS, MAX_SECRET_HANDLE_CHARS,
    MAX_SESSION_EVENT_POLL_TIMEOUT_MS, MAX_SESSION_IDLE_TIMEOUT_MS, MAX_SESSION_INPUT_EVENTS,
    MAX_SESSIONS_PER_CONNECTION, MAX_TASK_GRANT_ID_CHARS, MIN_SESSION_IDLE_TIMEOUT_MS,
    host_capabilities,
};
use serde_json::{Value, json};

pub(crate) fn document() -> Value {
    let session_events = json!({
        "state_method": "get_input_state",
        "poll_method": "poll_session_events",
        "max_poll_timeout_ms": MAX_SESSION_EVENT_POLL_TIMEOUT_MS,
        "queue_capacity": MAX_SESSION_INPUT_EVENTS,
        "observed_at": "unix_epoch_milliseconds",
        "sequence_scope": "per_session",
        "cursor_field": "latest_sequence",
        "component_sequence": "last_transition",
        "state_components": ["interactive_input", "target_window"],
        "overflow_contract": "resync_required_with_current_state",
        "recovery_notifies_only": true,
        "automatic_input": false,
        "resume_requirements": [
            "exact_target_revalidation",
            "fresh_observation",
            "foreground_or_explicit_activation",
            "explicit_upstream_session_refresh_when_required"
        ],
        "target_event_types": [
            "target_minimized",
            "target_restored",
            "target_unavailable",
            "target_available"
        ],
        "target_recovery": {
            "operation": "restore_activate",
            "request_method": "change_window_state",
            "exact_grant_binding_required": true,
            "explicit_request_required": true,
            "automatic_input": false,
            "fresh_observation_required": true,
            "blind_retry": false,
        },
    });
    let session_health = json!({
        "method": "session_health",
        "components": [
            "interactive_input",
            "exact_target_window",
            "recording",
            "action_evidence_epoch",
            "transition_sequence"
        ],
        "policy_defaults": {
            "require_recording_healthy": false,
            "require_recording_progress": false,
        },
        "recording_progress_fingerprint": [
            "lane",
            "trajectory_turn",
            "finalized_segments",
            "current_partial_size_bytes",
            "current_partial_modified_at_unix_ms"
        ],
        "recording_progress_authority": {
            "video_present": "video",
            "otherwise": "trajectory",
        },
        "consistency_fence": ["action_evidence_epoch", "transition_sequence"],
        "state_changed_during_probe_blocker": "state_changed_during_probe",
        "safe_to_input_authority": "preflight_only",
        "automatic_activation": false,
        "automatic_input": false,
        "fresh_observation_required": true,
        "replaces_execute_action_gate": false,
    });
    let session_escalation = json!({
        "method": "escalate_session",
        "requires_explicit_grant": true,
        "reason": {
            "type": "string",
            "enum": COMPUTER_USE_ESCALATION_REASONS
                .iter()
                .map(|reason| reason.value)
                .collect::<Vec<_>>(),
            "meanings": COMPUTER_USE_ESCALATION_REASONS
                .iter()
                .map(|reason| {
                    (
                        reason.value.to_owned(),
                        Value::String(reason.meaning.to_owned()),
                    )
                })
                .collect::<serde_json::Map<_, _>>(),
            "recommended": {
                "exact_window_uia_timeout": "uia_timeout",
                "background_delivery_failure": "background_delivery_failed",
            },
        },
        "detail": {
            "type": "string",
            "required": false,
            "max_chars": MAX_ESCALATION_DETAIL_CHARS,
        },
        "desktop_control_widened": false,
        "fresh_observation_required_after_escalation": true,
    });
    json!({
        "schema_version": 1,
        "name": "dcc-cua",
        "version": env!("CARGO_PKG_VERSION"),
        "rust_version": env!("CARGO_PKG_RUST_VERSION"),
        "target": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "host": {
            "stdio_command": ["host", "--stdio"],
            "endpoint_command": ["host"],
            "ensure_command": ["host-ensure"],
            "default_endpoint": HostTransport::default_endpoint(),
            "protocol_version": HOST_PROTOCOL_VERSION,
            "snapshot_transports": ["binary_frame", "shared_memory"],
            "max_json_frame_bytes": MAX_JSON_FRAME_BYTES,
            "max_binary_frame_bytes": MAX_BINARY_FRAME_BYTES,
            "max_connections": MAX_HOST_CONNECTIONS,
            "session_concurrency": {
                "model": "one_connection_per_logical_task",
                "max_sessions_per_connection": MAX_SESSIONS_PER_CONNECTION,
                "session_ownership": "connection_scoped",
                "same_public_session_id_across_connections": true,
                "capabilities_are_connection_private": true,
                "raw_input_arbitration": "host_global_fifo",
                "background_actions_may_run_concurrently": true,
                "disconnect_cleanup": "own_sessions_only",
                "logical_task_session": {
                    "client_type": "dcc_cua_client::LogicalTaskSession",
                    "one_connection": true,
                    "one_window_session": true,
                    "activity_renews_lease": true,
                    "default_idle_timeout_ms": DEFAULT_SESSION_IDLE_TIMEOUT_MS,
                    "min_idle_timeout_ms": MIN_SESSION_IDLE_TIMEOUT_MS,
                    "max_idle_timeout_ms": MAX_SESSION_IDLE_TIMEOUT_MS,
                    "idle_expiry": "stop_and_require_fresh_session",
                },
            },
            "hello_timeout_ms": HOST_HELLO_TIMEOUT_MS,
            "max_parallel_discovery_requests": MAX_PARALLEL_DISCOVERY_REQUESTS,
            "grant_limits": {
                "task_grant_id_max_chars": MAX_TASK_GRANT_ID_CHARS,
                "application_label_max_chars": MAX_APPLICATION_LABEL_CHARS,
            },
            "trusted_confirmation": {
                "request_schema": dcc_cua_host::TRUSTED_ACTION_CONFIRMATION_SCHEMA,
                "mode": if cfg!(windows) { "native_user_prompt" } else { "embedding_callback" },
                "action_scoped": true,
                "exact_window_identity": true,
                "default_decision": "deny",
                "input_text_echoed": false,
            },
            "task_authorization": {
                "request_schema": dcc_cua_host::TRUSTED_TASK_AUTHORIZATION_SCHEMA,
                "mode": "split_constructor_capability_broker",
                "issuer_owner": "authenticated_embedding_user_input",
                "embedding": "mcp_apps_inline_card",
                "task_scoped": true,
                "modal": false,
                "registration_single_use": true,
                "max_ttl_ms": 86_400_000,
                "exact_window_identity": true,
                "action_risk_category_bound": true,
                "browser_origin_bound": true,
                "expiry_and_revocation_checked_per_action": true,
                "ipc_can_mint_or_widen": false,
                "cli_arguments_can_authorize": false,
                "environment_can_authorize": false,
                "stdin_can_authorize": false,
                "input_text_echoed": false,
                "cli_fallback": "per_action_confirmation",
            },
            "secret_vault": {
                "backend": "platform_keyring",
                "service": "dcc-cua",
                "handle_max_chars": MAX_SECRET_HANDLE_CHARS,
                "plaintext_accepted_over_host_ipc": false,
                "resolve_after_exact_confirmation": true,
                "clipboard_capture_method": "clipboard_capture_secret",
                "clipboard_cleared_after_store": true,
            },
            "session_events": session_events,
            "session_health": session_health,
            "session_escalation": session_escalation,
            "capabilities": host_capabilities(cfg!(any(windows, target_os = "linux", target_os = "macos"))),
        },
        "core_bridge": {
            "command": ["host-jsonl"],
            "preferred_snapshot_transport": "shared_memory",
            "response_formats": ["host", "mcp"],
            "mcp_response_flag": ["--response-format", "mcp"],
            "mcp_native_image_content": true,
            "rust_crate": "dcc-cua-client",
        },
        "runtime": {
            "backend": "cua-driver-sdk",
            "separate_driver_required": false,
            "browser_prepare_existing_profile": {
                "exact_window_required": true,
                "control_discovery": "native_topology",
                "localized_labels": "opaque",
                "renderer_controls_rejected": true,
                "state_confirmation": "native_tab_count_delta",
            },
            "browser_providers": {
                "default": "cdp",
                "selection_command": ["browser-extension", "plan"],
                "extension": {
                    "when": "cdp_unavailable_and_explicit_tab_pairing_required",
                    "management_commands": ["plan", "status", "install-native-host"],
                    "host_status_method": "browser_extension_status",
                    "host_call_method": "browser_extension_call",
                    "native_messaging_host": "com.dcc_mcp.dcc_cua",
                    "ordinary_install": "signed_browser_store_with_user_authorization",
                    "silent_sideload": false,
                    "pairing": "explicit_action_click_in_exact_tab",
                },
            },
        },
    })
}
