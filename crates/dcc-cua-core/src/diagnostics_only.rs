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
mod tests {
    use super::*;

    #[test]
    fn diagnostics_only_factory_disables_preparation_renderer_and_host_hooks() {
        let options = driver_factory::diagnostics_only_host_options();
        assert!(!options.cursor.enabled);
        assert!(!options.prepare_desktop_environment);
        assert!(options.host_owns_permission_ux);
        assert!(!options.claude_code_compatibility);
        assert!(options.host_bundle_id.is_none());
        assert!(options.register_host_tools.is_none());
        assert!(options.authorization_host.is_none());
        assert!(options.activity_observer.is_none());
    }

    #[test]
    fn constructor_failure_keeps_context_and_does_not_claim_input_readiness() {
        let context = json!({"execution_context": {"thread_desktop": "fixture"}});
        let value = report(
            context.clone(),
            failed("diagnostic_runtime_constructor", "refused".into()),
            not_run("driver_constructor_failed"),
        );
        assert_eq!(value["checks"]["interactive_desktop"], context);
        assert_eq!(
            value["checks"]["driver"]["stage"],
            "diagnostic_runtime_constructor"
        );
        assert!(value["checks"]["driver"]["io_kind"].is_null());
        assert!(value["checks"]["driver"]["os_error"].is_null());
        assert_eq!(
            value["checks"]["static_capabilities"]["reason"],
            "driver_constructor_failed"
        );
        assert_eq!(
            value["checks"]["permissions"]["reason"],
            "unbounded_helper_deferred"
        );
        for name in ["health", "window_inventory"] {
            assert_eq!(value["checks"][name]["status"], "not_run");
            assert_eq!(
                value["checks"][name]["reason"],
                "outside_diagnostics_only_scope"
            );
        }
        assert_eq!(value["ready"], false);
        assert_eq!(value["input_ready"], false);
        assert_eq!(value["diagnostic_report_complete"], true);
    }

    #[test]
    fn successful_static_inventory_does_not_claim_input_readiness() {
        let value = report(
            json!({"input_ready": true}),
            json!({"success": true}),
            static_capabilities(
                r#"{"tools":[{"name":"second","inputSchema":{}},{"name":"first"}]}"#,
            ),
        );
        assert_eq!(value["ready"], false);
        assert_eq!(value["input_ready"], false);
        assert_eq!(value["read_only"], true);
        assert_eq!(
            value["checks"]["static_capabilities"],
            json!({"success":true,"status":"ok","count":2,"names":["first","second"]})
        );
    }

    #[test]
    fn malformed_or_ambiguous_static_inventory_fails_closed() {
        for raw in [
            "not JSON",
            "{}",
            r#"{"tools":[{}]}"#,
            r#"{"tools":[{"name":""}]}"#,
        ] {
            assert_eq!(
                static_capabilities(raw)["message"],
                "invalid_static_tool_inventory"
            );
        }
        assert_eq!(
            static_capabilities(r#"{"tools":[{"name":"same"},{"name":"same"}]}"#)["message"],
            "duplicate_static_tool_name"
        );
    }
}
