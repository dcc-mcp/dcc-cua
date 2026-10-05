use super::*;
use rstest::rstest;

#[rstest]
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

#[rstest]
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

#[rstest]
fn successful_static_inventory_does_not_claim_input_readiness() {
    let value = report(
        json!({"input_ready": true}),
        json!({"success": true}),
        static_capabilities(r#"{"tools":[{"name":"second","inputSchema":{}},{"name":"first"}]}"#),
    );
    assert_eq!(value["ready"], false);
    assert_eq!(value["input_ready"], false);
    assert_eq!(value["read_only"], true);
    assert_eq!(
        value["checks"]["static_capabilities"],
        json!({"success":true,"status":"ok","count":2,"names":["first","second"]})
    );
}

#[rstest]
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
