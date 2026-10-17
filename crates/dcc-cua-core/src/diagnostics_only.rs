//! Local context reporting without initializing a native execution runtime.

use serde_json::{Value, json};

/// Read project-owned desktop metadata without SDK construction or tool discovery.
/// Completing this report never establishes capture, input, or native acceptance.
pub fn diagnostics_only() -> Value {
    report(local_context())
}

fn local_context() -> Value {
    #[cfg(windows)]
    {
        json!({
            "status": "collected",
            "scope": "local_process_desktop_metadata",
            "data": crate::interactive_desktop::diagnostic(),
            "native_acceptance": "not_run",
        })
    }
    #[cfg(not(windows))]
    {
        // The existing platform-managed placeholder is not native probe evidence.
        not_run("native_desktop_context_requires_the_platform_runtime")
    }
}

fn not_run(reason: &str) -> Value {
    json!({"status": "not_run", "reason": reason})
}

fn report(context: Value) -> Value {
    json!({
        "type": "diagnostics",
        "schema_version": 1,
        "backend": "not_initialized",
        "mode": "diagnostics_only",
        "read_only": true,
        "diagnostic_report_complete": true,
        "ready": false,
        "input_ready": false,
        "native_acceptance": "not_run",
        "limits": {
            "scope": "local_process_desktop_metadata",
            "sdk": "not_initialized",
            "sessions": "not_created",
            "ipc": "not_opened",
            "input_actions": "not_executed",
            "shared_theme": "not_written",
            "renderer": "not_started",
            "completion_is_input_readiness": false,
        },
        "checks": {
            "interactive_desktop": context,
            "sdk_constructor": not_run("outside_diagnostics_only_scope"),
            "sdk_metadata": not_run("sdk_not_initialized"),
            "sdk_inventory": not_run("sdk_not_initialized"),
            "permissions": not_run("outside_diagnostics_only_scope"),
            "health": not_run("outside_diagnostics_only_scope"),
            "window_inventory": not_run("outside_diagnostics_only_scope"),
        },
    })
}

#[cfg(test)]
mod tests;
