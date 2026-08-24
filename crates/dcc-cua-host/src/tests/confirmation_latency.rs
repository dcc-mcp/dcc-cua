use dcc_cua_core::ComputerUseObservation;
use rstest::rstest;
use serde_json::json;

use crate::request_handler::{
    ConfirmedActionEvidenceRefresh, confirmed_action_evidence_refresh,
    rebind_confirmed_action_evidence,
};
use crate::{ConfirmationWindowIdentity, HostAction};

fn type_chars_action() -> HostAction {
    serde_json::from_value(json!({
        "action": "type_chars",
        "input_kind": "raw_input",
        "intent": "ordinary_edit",
        "text": "hidden from confirmation"
    }))
    .unwrap()
}

fn confirmed_action_value(action: &HostAction) -> serde_json::Value {
    let mut value = serde_json::to_value(action).unwrap();
    value["authorization_category"] = json!("raw_input");
    value
}

fn observation(id: &str, process_id: u32, window_handle: u64) -> ComputerUseObservation {
    ComputerUseObservation {
        observation_id: id.into(),
        window_handle,
        process_id,
        window_title: "Exact target".into(),
        width: 800,
        height: 600,
        source_rect: [10, 20, 800, 600],
        capture_backend: "test".into(),
        capture_provenance: json!({"accessibility_backend": "test"}),
        session_id: "session-1".into(),
    }
}

#[rstest]
fn type_chars_renews_evidence_only_when_confirmation_crosses_the_refresh_interval() {
    let action = type_chars_action();

    assert_eq!(
        confirmed_action_evidence_refresh(&action, false),
        ConfirmedActionEvidenceRefresh::NotRequired
    );
    assert_eq!(
        confirmed_action_evidence_refresh(&action, true),
        ConfirmedActionEvidenceRefresh::AccessibilityObservation
    );
}

#[rstest]
fn approved_type_chars_rebinds_to_equivalent_fresh_exact_target_evidence() {
    let action = type_chars_action();
    let previous = observation("observation-before-confirmation", 42, 77);
    let refreshed = observation("observation-after-confirmation", 42, 77);
    let expected_target = ConfirmationWindowIdentity {
        process_id: 42,
        window_handle: 77,
    };

    let rebound = rebind_confirmed_action_evidence(
        &action,
        &confirmed_action_value(&action),
        expected_target,
        &previous,
        &refreshed,
        &json!({"elements": []}),
    )
    .expect("equivalent exact-target evidence");

    assert_eq!(rebound, "observation-after-confirmation");
}

#[rstest]
#[case(43, 77, [10, 20, 800, 600])]
#[case(42, 78, [10, 20, 800, 600])]
#[case(42, 77, [10, 20, 801, 600])]
fn approved_type_chars_never_rebinds_after_target_or_geometry_changes(
    #[case] process_id: u32,
    #[case] window_handle: u64,
    #[case] source_rect: [i32; 4],
) {
    let action = type_chars_action();
    let previous = observation("observation-before-confirmation", 42, 77);
    let mut refreshed = observation("observation-after-confirmation", process_id, window_handle);
    refreshed.source_rect = source_rect;
    refreshed.width = source_rect[2] as u32;
    refreshed.height = source_rect[3] as u32;

    let error = rebind_confirmed_action_evidence(
        &action,
        &confirmed_action_value(&action),
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 77,
        },
        &previous,
        &refreshed,
        &json!({"elements": []}),
    )
    .unwrap_err();

    assert_eq!(
        error.code,
        dcc_cua_core::ComputerUseErrorCode::StaleObservation
    );
    let details = error.details.expect("structured refusal details");
    assert_eq!(details.action_attempted, Some(false));
    assert_eq!(
        details.input_sent,
        Some(dcc_cua_core::ComputerUseInputState::NotSent)
    );
    assert_eq!(details.automatic_input, Some(false));
    assert_eq!(details.blind_retry, Some(false));
}

#[rstest]
fn approved_type_chars_never_rebinds_if_the_action_identity_changes() {
    let action = type_chars_action();
    let mut changed_action = type_chars_action();
    changed_action.type_chars_only = true;
    let previous = observation("observation-before-confirmation", 42, 77);
    let refreshed = observation("observation-after-confirmation", 42, 77);

    let error = rebind_confirmed_action_evidence(
        &changed_action,
        &confirmed_action_value(&action),
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 77,
        },
        &previous,
        &refreshed,
        &json!({"elements": []}),
    )
    .unwrap_err();

    assert_eq!(
        error.code,
        dcc_cua_core::ComputerUseErrorCode::StaleObservation
    );
    assert_eq!(
        error.details.and_then(|details| details.action_attempted),
        Some(false)
    );
}
