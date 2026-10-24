//! Current-executor metadata without opening or reading the input surface.
use serde_json::{Value, json};

#[cfg(windows)]
mod windows;

/// This does not inspect an existing MCP server, acquire a desktop, or attest readiness.
pub fn context_only_diagnostics() -> Value {
    #[cfg(windows)]
    let context = collect(&windows::NativeContext);
    #[cfg(not(windows))]
    let context = json!({
        "status":"not_run", "reason":"windows_context_metadata_not_supported",
        "data":{"process_id":std::process::id()},
    });
    report(context)
}

// Deliberately capability-limited: no cursor, input desktop, foreground,
// permissions, capture, SDK or runtime operation can be requested through this seam.
#[cfg(any(windows, test))]
trait ContextProbe {
    fn process_id(&self) -> u32;
    fn process_birth(&self) -> Result<String, String>;
    fn thread_id(&self) -> u32;
    fn session_id(&self) -> Result<u32, String>;
    fn window_station(&self) -> Result<Option<String>, String>;
    fn thread_desktop(&self) -> Result<Option<String>, String>;
    fn receives_input(&self) -> Result<bool, String>;
}

#[cfg(any(windows, test))]
fn collect(probe: &impl ContextProbe) -> Value {
    let birth = probe.process_birth();
    let session = probe.session_id();
    let station = probe.window_station();
    let desktop = probe.thread_desktop();
    let receives = probe.receives_input();
    let complete = birth.is_ok()
        && session.is_ok()
        && matches!(&station, Ok(Some(_)))
        && matches!(&desktop, Ok(Some(_)))
        && receives.is_ok();
    json!({
        "status":if complete { "collected" } else { "partial" },
        "data":{
            "source":"current_process_and_thread", "process_id":probe.process_id(),
            // Full FILETIME ticks are a string, preserving precision in JSON clients.
            "process_birth":{"windows_filetime_100ns":birth.as_ref().ok(),"error":birth.as_ref().err()},
            "thread_id":probe.thread_id(),
            "session_id":{"value":session.as_ref().ok(),"error":session.as_ref().err()},
            "window_station":{"name":station.as_ref().ok().and_then(|v|v.as_deref()),"error":station.as_ref().err()},
            "thread_desktop":{
                "name":desktop.as_ref().ok().and_then(|v|v.as_deref()),"error":desktop.as_ref().err(),
                "receives_input":receives.as_ref().ok(),"receives_input_error":receives.as_ref().err(),
            },
        },
    })
}

fn report(context: Value) -> Value {
    let mut checks = json!({"execution_context":context});
    for check in [
        "input_desktop",
        "input_surface",
        "foreground_window",
        "sdk_constructor",
        "sdk_inventory",
        "permissions",
        "health",
        "window_inventory",
        "capture",
        "recording",
    ] {
        checks[check] = json!({"status":"not_run","reason":"outside_context_only_scope"});
    }
    json!({
        "type":"diagnostics", "schema_version":1, "mode":"context_only",
        "read_only":true, "diagnostic_report_complete":true,
        "scope":"current_executor_borrowed_metadata", "backend":"not_initialized",
        "ready":false, "input_ready":false, "readiness_assessed":false,
        "native_acceptance":"not_run", "checks":checks,
        "limits":{
            "sdk":"not_initialized", "sessions":"not_created", "ipc":"not_opened",
            "renderer":"not_started", "shared_theme":"not_written", "input_actions":"not_executed",
            "input_desktop":"not_opened", "input_surface":"not_read", "foreground_window":"not_read",
            "desktop_or_permissions_changed":false, "completion_is_input_readiness":false,
        },
        "agent_host_binding":{
            "status":"not_established", "existing_mcp_process_inspected":false,
            "missing_evidence":[
                "agent_host_product_version_and_supported_launch_mode",
                "parent_and_child_full_process_identity",
                "launched_executable_hash_and_source_revision",
                "startup_desktop_policy_and_inherited_handles",
                "actual_mcp_execution_thread_context",
                "original_native_access_failure_security_context",
            ],
        },
    })
}

#[cfg(test)]
mod tests;
