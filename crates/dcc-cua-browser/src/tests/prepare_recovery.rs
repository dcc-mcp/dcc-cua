use rstest::rstest;
use serde_json::{Value, json};

use crate::prepare_recovery::annotate;

fn refusal() -> Value {
    json!({"structuredContent": {
        "status": "refused",
        "refusal": {
            "code": "browser_wrong_target_refused",
            "message": "the setup page did not become ready",
            "detail": {"setup_side_effects": {
                "opened_setup_page": true,
                "closed_setup_page": false,
                "enabled_remote_debugging": false
            }}
        }
    }})
}

fn target(app_name: &str) -> Value {
    json!({"pid": 42, "window_id": 77, "app_name": app_name})
}

#[rstest]
#[case("msedge.exe", Some("edge://inspect/#remote-debugging"))]
#[case(
    "C:\\Program Files\\Microsoft\\Edge\\MSedge.exe",
    Some("edge://inspect/#remote-debugging")
)]
#[case("Google Chrome", Some("chrome://inspect/#remote-debugging"))]
#[case("chromium", Some("chrome://inspect/#remote-debugging"))]
#[case("firefox.exe", None)]
#[case("pretend-msedge.exe", None)]
fn failed_setup_reports_manual_recovery_without_inventing_tab_authority(
    #[case] app_name: &str,
    #[case] url: Option<&str>,
) {
    let mut value = refusal();
    let original = value.clone();
    annotate(&mut value, true, Some(&target(app_name)));
    let recovery = &value["structuredContent"]["recovery"];
    assert_eq!(recovery["kind"], "existing_profile_setup");
    assert_eq!(recovery["target"], json!({"pid": 42, "window_id": 77}));
    assert_eq!(recovery["setup"]["url"].as_str(), url);
    assert_eq!(recovery["setup"]["setting_state"], "unverified");
    assert_eq!(recovery["cleanup"]["status"], "manual_cleanup_required");
    assert_eq!(recovery["automatic_cleanup"], false);
    assert_eq!(recovery["automatic_retry"], false);
    assert!(recovery.get("cleanup_handle").is_none());
    value["structuredContent"]
        .as_object_mut()
        .unwrap()
        .remove("recovery");
    assert_eq!(value, original);
}

#[rstest]
#[case("/structuredContent/status", json!("ok"))]
#[case("/structuredContent/prepared", json!(true))]
#[case("/structuredContent/refusal/code", json!("browser_consent_revoked"))]
#[case("/structuredContent/refusal/detail/setup_side_effects", Value::Null)]
#[case("/structuredContent/refusal/detail/setup_side_effects/opened_setup_page", json!("true"))]
#[case(
    "/structuredContent/refusal/detail/setup_side_effects/closed_setup_page",
    Value::Null
)]
fn unrelated_or_malformed_receipts_do_not_gain_recovery(#[case] path: &str, #[case] field: Value) {
    let mut value = refusal();
    if path.ends_with("/prepared") {
        value["structuredContent"]["prepared"] = field;
    } else {
        *value.pointer_mut(path).unwrap() = field;
    }
    let original = value.clone();
    annotate(&mut value, true, Some(&target("msedge.exe")));
    assert_eq!(value, original);
}

#[rstest]
#[case(false, Some(json!({"pid": 42, "window_id": 77})))]
#[case(true, None)]
#[case(true, Some(json!({"pid": 0, "window_id": 77})))]
#[case(true, Some(json!({"pid": 42, "window_id": 0})))]
#[case(true, Some(json!({"pid": "42", "window_id": 77})))]
fn recovery_requires_existing_profile_and_exact_native_target(
    #[case] existing_profile: bool,
    #[case] target: Option<Value>,
) {
    let mut value = refusal();
    let original = value.clone();
    annotate(&mut value, existing_profile, target.as_ref());
    assert_eq!(value, original);
}

#[rstest]
#[case(false, false, "not_opened")]
#[case(true, true, "closed_by_driver")]
fn reported_cleanup_states_remain_distinct(
    #[case] opened: bool,
    #[case] closed: bool,
    #[case] status: &str,
) {
    let mut value = refusal();
    let effects = &mut value["structuredContent"]["refusal"]["detail"]["setup_side_effects"];
    effects["opened_setup_page"] = json!(opened);
    effects["closed_setup_page"] = json!(closed);
    annotate(&mut value, true, Some(&target("msedge.exe")));
    assert_eq!(
        value["structuredContent"]["recovery"]["cleanup"]["status"],
        status
    );
}
