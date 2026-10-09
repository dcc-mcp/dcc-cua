use std::future::pending;
use std::time::Duration;

use rstest::rstest;
use serde_json::json;

use super::test_window_target;
use crate::policy::{ensure_tool_ok, map_driver_error};
#[cfg(windows)]
use crate::runtime::map_windows_window_mutation_error;
use crate::runtime::{
    action_dispatch_completion_unknown, activation_completion_unknown, await_input_call,
    input_backend_rejection_result,
};
use crate::{
    ComputerUseCompletionState, ComputerUseError, ComputerUseErrorCode, ComputerUseInputState,
    ComputerUseToolStatus,
};

#[rstest]
fn capture_diagnostic_contract_rejects_arbitrary_reason_and_stage_strings() {
    let mut value = json!({
        "stage": "visible_desktop_proof",
        "reason": "root_overlap",
        "target_process_id": 42,
        "target_window_handle": 77,
        "blocker_window_handle": 91,
    });
    let diagnostic: crate::ComputerUseCaptureDiagnostic =
        serde_json::from_value(value.clone()).expect("closed metadata contract");
    assert_eq!(
        diagnostic.reason,
        crate::ComputerUseCaptureReason::RootOverlap
    );
    value["reason"] = json!("PRIVATE_FOREIGN_WINDOW_TEXT");
    assert!(serde_json::from_value::<crate::ComputerUseCaptureDiagnostic>(value.clone()).is_err());
    value["reason"] = json!("root_overlap");
    value["stage"] = json!("PRIVATE_BACKEND_ERROR");
    assert!(serde_json::from_value::<crate::ComputerUseCaptureDiagnostic>(value).is_err());
}

#[cfg(windows)]
#[rstest]
#[case(dcc_cua_platform_windows::RootBoundsClass::Positive, "positive")]
#[case(dcc_cua_platform_windows::RootBoundsClass::ZeroArea, "zero_area")]
#[case(dcc_cua_platform_windows::RootBoundsClass::Inverted, "inverted")]
#[case(dcc_cua_platform_windows::RootBoundsClass::Overflow, "overflow")]
fn root_bounds_failure_native_to_public_mapping_is_lossless(
    #[case] classification: dcc_cua_platform_windows::RootBoundsClass,
    #[case] name: &str,
) {
    use dcc_cua_platform_windows::{RootBoundsFailureDiagnostic, RootBoundsRole};
    let native = RootBoundsFailureDiagnostic {
        root_role: RootBoundsRole::AboveTargetRoot,
        proof_target_root_window_handle: 77,
        dwm_raw_rect_edges: [10, 20, 10, 30],
        dwm_classification: classification,
        visible: true,
        cloaked: Some(0),
        win32_read_after_dwm_rejection: true,
        win32_raw_rect_edges: Some([10, 20, 30, 40]),
        win32_classification: Some(classification),
        win32_os_error: None,
        zero_area_status_mismatch: Some(true),
    };
    let public = crate::runtime::map_root_bounds_failure(native);
    let encoded = serde_json::to_value(public).unwrap();
    assert_eq!(encoded, serde_json::to_value(native).unwrap());
    assert_eq!(encoded["dwm_classification"], name);
    assert_eq!(
        serde_json::from_value::<crate::ComputerUseRootBoundsFailureDiagnostic>(encoded).unwrap(),
        public
    );
}

#[rstest]
#[case("root_role", json!("PRIVATE_TITLE"))]
#[case("dwm_classification", json!("PRIVATE_ERROR"))]
#[case("win32_classification", json!("PRIVATE_ERROR"))]
#[case("dwm_raw_rect_edges", json!([0, 0, 1]))]
#[case("win32_raw_rect_edges", json!([0, 0, 1, 2, 3]))]
#[case("visible", json!("true"))]
fn root_bounds_failure_contract_has_only_closed_types(
    #[case] field: &str,
    #[case] invalid: serde_json::Value,
) {
    let mut value = json!({
        "root_role":"target_root", "proof_target_root_window_handle":77,
        "dwm_raw_rect_edges":[0,0,0,1], "dwm_classification":"zero_area",
        "visible":true, "cloaked":0, "win32_read_after_dwm_rejection":true,
        "win32_raw_rect_edges":null, "win32_classification":null,
        "win32_os_error":-5, "zero_area_status_mismatch":null
    });
    assert!(
        serde_json::from_value::<crate::ComputerUseRootBoundsFailureDiagnostic>(value.clone())
            .is_ok()
    );
    value[field] = invalid;
    assert!(
        serde_json::from_value::<crate::ComputerUseRootBoundsFailureDiagnostic>(value).is_err()
    );
}

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
fn missing_provider_has_a_distinct_non_retryable_error_code() {
    let error = map_driver_error(
        "capture CUA window state",
        cua_driver_sdk::DriverError::Tool {
            tool: "get_window_state".into(),
            message: "the exact window exposes no semantic provider".into(),
            error_code: "no_accessibility_provider".into(),
        },
    );

    assert_eq!(error.code, ComputerUseErrorCode::NoAccessibilityProvider);
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
fn unsupported_input_backend_is_a_structured_non_fallback_result() {
    let result = input_backend_rejection_result(
        "windows.unknown.v1",
        "unsupported input backend",
        &test_window_target(),
    );
    assert_eq!(result.status, ComputerUseToolStatus::Rejected);
    assert_eq!(result.value["success"], false);
    assert_eq!(result.value["route"], "input_backend_selection");
    assert_eq!(
        result.value["delivery"],
        json!({
            "mode": "foreground",
            "backend_id": "windows.unknown.v1",
            "api_accepted": false,
            "consumer_effect_confirmed": false,
            "completion_known": false,
            "verification_required": true,
            "retry_safe": false,
            "fallback_attempted": false,
            "rejection_reason": "unsupported input backend",
            "target_fence": {
                "process_id": 42,
                "window_handle": 7,
                "exact_window": true,
                "foreground_required": true,
                "foreground_verified": true
            }
        })
    );
    assert_eq!(result.value["effect"], "not_attempted");
    assert!(result.degraded);
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
