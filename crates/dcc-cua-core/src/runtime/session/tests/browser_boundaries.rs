use super::super::browser::{BROWSER_TOOL_CALL_TIMEOUT, browser_tool_timeout};
use super::super::ensure_target_available_for_bootstrap_activation;
use crate::ComputerUseErrorCode;
use rstest::rstest;
use serde_json::{Value, json};
use std::time::Duration;

use crate::runtime::INPUT_CALL_TIMEOUT;
use crate::window_target::WindowTarget;

#[rstest]
#[case(true, false, true)]
#[case(false, true, true)]
#[case(false, false, false)]
fn bootstrap_activation_allows_only_the_explicit_minimized_recovery_path(
    #[case] is_minimized: bool,
    #[case] is_on_screen: bool,
    #[case] accepted: bool,
) {
    let target = WindowTarget {
        pid: 42,
        window_id: 77,
        title: "Test DCC".into(),
        app_name: "test.exe".into(),
        bounds: [0, 0, 800, 600],
        is_foreground: false,
        is_minimized,
        is_on_screen,
        z_index: None,
    };

    let result = ensure_target_available_for_bootstrap_activation(&target);
    assert_eq!(result.is_ok(), accepted);
    if let Err(error) = result {
        assert_eq!(error.code, ComputerUseErrorCode::TargetUnavailable);
    }
}

#[rstest]
#[case("browser_prepare", json!({}), BROWSER_TOOL_CALL_TIMEOUT)]
#[case("get_browser_state", json!({}), BROWSER_TOOL_CALL_TIMEOUT)]
#[case(
    "get_browser_state",
    json!({"target_id": "target-1"}),
    BROWSER_TOOL_CALL_TIMEOUT
)]
#[case("browser_click", json!({}), BROWSER_TOOL_CALL_TIMEOUT)]
#[case("browser_type", json!({}), BROWSER_TOOL_CALL_TIMEOUT)]
#[case("browser_snapshot", json!({}), BROWSER_TOOL_CALL_TIMEOUT)]
#[case("list_windows", json!({}), INPUT_CALL_TIMEOUT)]
fn browser_binding_timeout_covers_the_bounded_existing_profile_reconnect(
    #[case] name: &str,
    #[case] arguments: Value,
    #[case] expected: Duration,
) {
    let object = arguments.as_object().expect("browser tool arguments");
    assert_eq!(browser_tool_timeout(name, object), expected);
    assert!(BROWSER_TOOL_CALL_TIMEOUT > Duration::from_secs(32));
}
