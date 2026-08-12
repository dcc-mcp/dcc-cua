use rstest::rstest;
use serde_json::json;

use super::*;

#[rstest]
fn global_uia_enumeration_timeout_keeps_exact_window_semantics_degraded() {
    let permissions = json!({"result": {"uia": true}});
    let health = json!({
        "result": {
            "checks": [{
                "name": "ax_capability",
                "status": "fail",
                "data": {
                    "error_detail": "UI Automation desktop enumeration exceeded 2000ms; a UIA provider may be hung."
                }
            }]
        }
    });

    let route = diagnostic_semantic_route(true, &permissions, &health, true);
    assert_eq!(route["ready"], true);
    assert_eq!(route["degraded"], true);
    assert_eq!(route["mode"], "exact_window_uia_fallback");
    assert_eq!(route["global_enumeration_ready"], false);

    assert_eq!(
        diagnostic_semantic_route(true, &permissions, &health, false)["ready"],
        false
    );
    assert_eq!(
        diagnostic_semantic_route(true, &json!({"result": {"uia": false}}), &health, true)["ready"],
        false
    );
}

#[rstest]
fn desktop_actions_accept_signed_virtual_desktop_coordinates() {
    let action = ComputerUseAction {
        action: "click".into(),
        x: Some(-2_400.0),
        y: Some(80.0),
        ..Default::default()
    };

    validate_action(&action).unwrap();
    let args = desktop_action_arguments(&action, "desktop-session");
    assert_eq!(args["scope"], "desktop");
    assert_eq!(args["x"], -2_400.0);
    assert_eq!(args["y"], 80.0);
}

#[rstest]
fn window_actions_reject_negative_screenshot_local_coordinates() {
    let action = ComputerUseAction {
        action: "click".into(),
        x: Some(-10.0),
        y: Some(20.0),
        ..Default::default()
    };

    validate_action(&action).unwrap();
    let error = validate_window_action_coordinates(&action).unwrap_err();
    assert!(error.message.contains("screenshot-local"));
}
