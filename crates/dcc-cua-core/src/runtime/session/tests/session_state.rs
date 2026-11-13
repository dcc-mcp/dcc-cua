use super::*;
use crate::runtime::session_status::window_session_state;
use rstest::rstest;

#[rstest]
fn lifecycle_state_retains_the_host_window_contract_and_its_source() {
    let receipt = json!({
        "content": [{"type": "text", "text": "Session is active."}],
        "structuredContent": {
            "session": "owned-state", "state": "active", "cursor_visible": false,
            "recording_active": false, "expires_in_seconds": 123
        }
    });
    let state = window_session_state(receipt.clone(), "owned-state").unwrap();
    let fields = &state["structuredContent"];
    assert_eq!(fields["effective_scope"], "window");
    assert_eq!(fields["desktop_capture_authorized"], false);
    assert_eq!(fields["desktop_unlocked"], false);
    assert_eq!(fields["capture_scope_source"], "host_exact_window_binding");
    assert_eq!(fields["cursor_visible"], false);
    assert_eq!(fields["expires_in_seconds"], 123);
    assert_eq!(state["content"], receipt["content"]);
}

#[rstest]
#[case(json!({}), ComputerUseErrorCode::TargetUnavailable)]
#[case(json!({"session": "foreign", "state": "active"}), ComputerUseErrorCode::TargetUnavailable)]
#[case(json!({"session": "owned-state", "state": "ending"}), ComputerUseErrorCode::TargetUnavailable)]
#[case(json!({"session": "owned-state", "state": "active", "active": false}), ComputerUseErrorCode::TargetUnavailable)]
#[case(json!({"session": "owned-state", "state": "active", "capture_scope": "auto"}), ComputerUseErrorCode::TargetUnavailable)]
#[case(json!({"session": "owned-state", "state": "active", "effective_scope": "desktop"}), ComputerUseErrorCode::TargetUnavailable)]
#[case(json!({"session": "owned-state", "state": "active", "desktop_capture_authorized": true}), ComputerUseErrorCode::TargetUnavailable)]
#[case(json!({"session": "owned-state", "state": "active", "desktop_unlocked": "false"}), ComputerUseErrorCode::TargetUnavailable)]
#[case(Value::Null, ComputerUseErrorCode::BackendUnavailable)]
fn malformed_foreign_or_broader_lifecycle_receipts_cannot_publish_window_state(
    #[case] receipt: Value,
    #[case] expected: ComputerUseErrorCode,
) {
    let error =
        window_session_state(json!({"structuredContent": receipt}), "owned-state").unwrap_err();
    assert_eq!(error.code, expected);
}
