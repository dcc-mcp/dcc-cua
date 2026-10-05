use serde_json::{Value, json};

use crate::{driver_factory, interactive_desktop};

/// Local desktop context and static SDK inventory; never an input-readiness test.
pub async fn diagnostics_only() -> Value {
    // Retain this evidence even when the SDK rejects runtime construction.
    let context = interactive_desktop::diagnostic_read_only();
    let (metadata, capabilities) = match driver_factory::create_diagnostics_only() {
        Ok(driver) => {
            // Neither method invokes a tool or starts permission/window probes.
            let (metadata, tools) = tokio::join!(driver.metadata(), driver.list_tools_json());
            let metadata = match metadata {
                Ok(data) => json!({"success": true, "status": "ok", "data": data}),
                Err(error) => failed("sdk_metadata", error.to_string()),
            };
            let capabilities = match tools {
                Ok(raw) => static_capabilities(&raw),
                Err(error) => failed("sdk_static_tool_inventory", error.to_string()),
            };
            (metadata, capabilities)
        }
        Err(error) => (
            failed("diagnostic_runtime_constructor", error.to_string()),
            not_run("driver_constructor_failed"),
        ),
    };
    report(context, metadata, capabilities)
}

fn failed(stage: &str, message: String) -> Value {
    json!({"success": false, "status": "failed", "stage": stage,
        "message": message, "io_kind": null, "os_error": null})
}

fn not_run(reason: &str) -> Value {
    json!({"success": false, "status": "not_run", "reason": reason})
}

fn static_capabilities(raw: &str) -> Value {
    let parsed = serde_json::from_str::<Value>(raw);
    let Some(tools) = parsed
        .as_ref()
        .ok()
        .and_then(|value| value["tools"].as_array())
    else {
        return failed(
            "sdk_static_tool_inventory",
            "invalid_static_tool_inventory".into(),
        );
    };
    let mut names = Vec::with_capacity(tools.len());
    for tool in tools {
        let Some(name) = tool["name"].as_str().filter(|name| !name.is_empty()) else {
            return failed(
                "sdk_static_tool_inventory",
                "invalid_static_tool_inventory".into(),
            );
        };
        names.push(name.to_owned());
    }
    names.sort();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return failed(
            "sdk_static_tool_inventory",
            "duplicate_static_tool_name".into(),
        );
    }
    json!({"success": true, "status": "ok", "count": names.len(), "names": names})
}

fn report(context: Value, metadata: Value, capabilities: Value) -> Value {
    json!({
        "type": "diagnostics", "schema_version": 1, "backend": "SDK",
        "mode": "diagnostics_only", "read_only": true,
        "diagnostic_report_complete": true, "ready": false, "input_ready": false,
        "native_acceptance": "not_run",
        "limits": {
            "scope": "local_process_desktop_context_and_static_inventory",
            "sessions": "not_created", "ipc": "not_opened", "input_actions": "not_executed",
            "shared_theme": "not_written", "renderer": "disabled",
            "existing_configuration": "may_be_read", "sdk_maintenance": "process_local",
            "completion_is_input_readiness": false
        },
        "checks": {
            "interactive_desktop": context, "driver": metadata, "static_capabilities": capabilities,
            "permissions": not_run("unbounded_helper_deferred"),
            "health": not_run("outside_diagnostics_only_scope"),
            "window_inventory": not_run("outside_diagnostics_only_scope")
        }
    })
}

#[cfg(test)]
mod tests;
