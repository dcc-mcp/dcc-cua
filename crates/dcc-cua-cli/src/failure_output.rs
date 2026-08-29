//! Fixed, allowlisted one-shot terminal failure envelopes.

use dcc_cua_client::HostClientError;
use dcc_cua_core::{ComputerUseError, ComputerUseErrorCode};
use serde_json::{Value, json};

pub(super) const PUBLIC_FAILURE_MESSAGE: &str = "dcc-cua could not complete the command";
const NO_ACCESSIBILITY_PROVIDER_MESSAGE: &str =
    "The exact window has no usable accessibility provider.";

pub(super) fn fatal_error_line(error: &(dyn std::error::Error + 'static)) -> String {
    serde_json::to_string(&fatal_error_value(error)).unwrap_or_else(|_| {
        r#"{"success":false,"error":{"code":"command_failed","message":"dcc-cua could not complete the command"}}"#.into()
    })
}

pub(super) fn internal_failure_line() -> String {
    r#"{"success":false,"error":{"code":"internal_failure","message":"dcc-cua could not complete the command"}}"#.into()
}

pub(super) fn fatal_error_value(error: &(dyn std::error::Error + 'static)) -> Value {
    if let Some(error) = error.downcast_ref::<ComputerUseError>() {
        if error.code == ComputerUseErrorCode::NoAccessibilityProvider {
            return json!({
                "success": false,
                "error": {
                    "code": error.code,
                    "message": NO_ACCESSIBILITY_PROVIDER_MESSAGE,
                    "details": {
                        "retryable": false,
                        "permanent_for_window_class": true,
                        "fallback_command": "snapshot --pixels-only",
                        "fallback_requires": "ocr_or_another_perception_layer",
                    }
                }
            });
        }
        return json!({
            "success": false,
            "error": {
                "code": error.code,
                "message": PUBLIC_FAILURE_MESSAGE,
            }
        });
    }
    if let Some(error) = error.downcast_ref::<HostClientError>() {
        let code = match error {
            HostClientError::Io(_) => "host_transport_failed",
            HostClientError::Protocol(_) => "host_protocol_failed",
            HostClientError::Timeout { .. } => "host_timeout",
            HostClientError::Remote { .. } => "host_remote_failed",
        };
        return json!({
            "success": false,
            "error": {"code": code, "message": PUBLIC_FAILURE_MESSAGE},
        });
    }
    json!({
        "success": false,
        "error": {"code": "command_failed", "message": PUBLIC_FAILURE_MESSAGE},
    })
}
