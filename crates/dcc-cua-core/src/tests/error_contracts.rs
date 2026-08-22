use std::future::pending;
use std::time::Duration;

use rstest::rstest;

use crate::policy::{ensure_tool_ok, map_driver_error};
#[cfg(windows)]
use crate::runtime::map_windows_window_mutation_error;
use crate::runtime::{
    action_dispatch_completion_unknown, activation_completion_unknown, await_input_call,
};
use crate::{
    ComputerUseCompletionState, ComputerUseError, ComputerUseErrorCode, ComputerUseInputState,
};

#[rstest]
#[tokio::test]
async fn input_calls_have_a_hard_timeout() {
    let error = await_input_call(
        pending::<()>(),
        Duration::from_millis(1),
        "window activation",
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::InputFailed);
    assert_eq!(
        error.details.as_ref().and_then(|details| details.timed_out),
        Some(true)
    );
    assert!(error.message.contains("window activation timed out"));
}

#[rstest]
fn activation_timeout_is_typed_completion_unknown_without_blind_retry() {
    let error = activation_completion_unknown(ComputerUseError::new(
        ComputerUseErrorCode::InputFailed,
        "window activation timed out",
    ));

    assert_eq!(error.code, ComputerUseErrorCode::CompletionUnknown);
    let details = error.details.expect("activation failure details");
    assert_eq!(
        details.completion,
        Some(ComputerUseCompletionState::Unknown)
    );
    assert_eq!(details.automatic_input, Some(false));
    assert_eq!(details.blind_retry, Some(false));
}

#[rstest]
fn action_dispatch_timeout_reports_attempted_input_and_real_session_invalidation() {
    let error = action_dispatch_completion_unknown(ComputerUseError::new(
        ComputerUseErrorCode::InputFailed,
        "CUA action timed out after 15000 ms",
    ));

    assert_eq!(error.code, ComputerUseErrorCode::CompletionUnknown);
    let details = error.details.expect("dispatch failure details");
    assert_eq!(details.action_attempted, Some(true));
    assert_eq!(details.input_sent, Some(ComputerUseInputState::Unknown));
    assert_eq!(
        details.completion,
        Some(ComputerUseCompletionState::Unknown)
    );
    assert_eq!(details.local_session_invalidated, Some(true));
    assert_eq!(details.blind_retry, Some(false));
}

#[cfg(windows)]
#[rstest]
fn foreground_activation_refusal_is_typed_and_suggests_safe_background_delivery() {
    let error = map_windows_window_mutation_error(
        "activate exact target",
        dcc_cua_platform_windows::UiaError::ForegroundActivationRefused {
            reason: "Windows rejected foreground activation".into(),
            background_delivery_viable: true,
            suggested_delivery_mode: Some("background".into()),
        },
    );

    assert_eq!(
        error.code,
        ComputerUseErrorCode::ForegroundActivationRefused
    );
    let details = error
        .details
        .expect("foreground refusal must carry actionable structured details");
    assert_eq!(details.background_delivery_viable, Some(true));
    assert_eq!(
        details.suggested_delivery_mode.as_deref(),
        Some("background")
    );
    assert!(!error.message.contains("suggested_delivery_mode="));
}

#[rstest]
fn native_provider_timeout_is_backend_unavailable() {
    let error = map_driver_error(
        "capture CUA window state",
        cua_driver_sdk::DriverError::Tool {
            tool: "get_window_state".into(),
            message: "provider unavailable".into(),
            error_code: "backend_unavailable".into(),
        },
    );
    assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
}

#[rstest]
fn tool_provider_timeout_is_backend_unavailable() {
    let result = cua_driver_sdk::ToolResult {
        is_error: true,
        error_code: Some("backend_unavailable".into()),
        raw_json: "{}".into(),
        text: "get_window_state timed out: UIA provider unresponsive".into(),
        structured_json: None,
        images: Vec::new(),
        degraded: false,
        action: None,
        verification: None,
    };
    assert_eq!(
        ensure_tool_ok("capture CUA window", &result)
            .unwrap_err()
            .code,
        ComputerUseErrorCode::BackendUnavailable
    );
}

#[rstest]
fn driver_failure_classification_uses_only_the_typed_error_code() {
    let result = cua_driver_sdk::ToolResult {
        is_error: true,
        error_code: Some("input_failed".into()),
        raw_json: "{}".into(),
        text: "window title: Browser recording clipboard settings".into(),
        structured_json: None,
        images: Vec::new(),
        degraded: false,
        action: None,
        verification: None,
    };

    assert_eq!(
        ensure_tool_ok("perform input", &result).unwrap_err().code,
        ComputerUseErrorCode::InputFailed
    );
}

#[rstest]
#[case("target_minimized", ComputerUseErrorCode::TargetMinimized)]
#[case("target_unavailable", ComputerUseErrorCode::TargetUnavailable)]
#[case("missing_window", ComputerUseErrorCode::TargetUnavailable)]
#[case(
    "interactive_desktop_unavailable",
    ComputerUseErrorCode::InteractiveDesktopUnavailable
)]
#[case(
    "input_gate_stage=foreground_dispatch",
    ComputerUseErrorCode::InteractiveDesktopUnavailable
)]
fn exact_status_driver_markers_override_browser_and_uia_classification(
    #[case] marker: &str,
    #[case] expected: ComputerUseErrorCode,
) {
    let result = cua_driver_sdk::ToolResult {
        is_error: true,
        error_code: Some(marker.into()),
        raw_json: "{}".into(),
        text: "browser UIA operation rejected the exact target".into(),
        structured_json: None,
        images: Vec::new(),
        degraded: false,
        action: None,
        verification: None,
    };
    assert_eq!(
        ensure_tool_ok("perform browser operation", &result)
            .unwrap_err()
            .code,
        expected
    );
    assert_eq!(
        map_driver_error(
            "perform browser operation",
            cua_driver_sdk::DriverError::Tool {
                tool: "test".into(),
                message: "browser UIA operation rejected the exact target".into(),
                error_code: marker.into(),
            }
        )
        .code,
        expected
    );
}
