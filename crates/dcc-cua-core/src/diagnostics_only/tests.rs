use rstest::rstest;
use serde_json::json;

#[rstest]
fn context_readiness_never_promotes_diagnostics_to_native_acceptance() {
    let report = super::report(json!({"success": true, "input_ready": true}));

    assert_eq!(report["diagnostic_report_complete"], true);
    assert_eq!(report["ready"], false);
    assert_eq!(report["input_ready"], false);
    assert_eq!(report["native_acceptance"], "not_run");
    assert_eq!(report["backend"], "not_initialized");
    for check in [
        "sdk_constructor",
        "sdk_metadata",
        "sdk_inventory",
        "permissions",
        "health",
        "window_inventory",
    ] {
        assert_eq!(report["checks"][check]["status"], "not_run");
    }
}

#[cfg(not(windows))]
#[rstest]
fn platform_managed_placeholder_is_not_native_context_evidence() {
    let context = super::local_context();

    assert_eq!(context["status"], "not_run");
    assert!(context.get("success").is_none());
    assert!(context.get("input_ready").is_none());
}
