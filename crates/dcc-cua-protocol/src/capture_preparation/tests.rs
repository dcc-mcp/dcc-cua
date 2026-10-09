use rstest::rstest;

use super::*;
use serde_json::json;

#[rstest]
#[case(0)]
#[case(MAX_PREPARATION_LIFETIME_MS + 1)]
#[case(u64::MAX)]
fn preparation_request_rejects_unbounded_lifetime(#[case] lifetime_ms: u64) {
    assert!(
        CapturePreparationBeginRequest {
            window_state_id: "fresh-state".into(),
            lifetime_ms,
        }
        .validate()
        .is_err()
    );
}

#[rstest]
#[case("journal_directory", json!("C:/arbitrary"))]
#[case("allowed_affected_scope", json!([]))]
#[case("input_authorized", json!(true))]
fn request_cannot_supply_constructor_permission(
    #[case] key: &str,
    #[case] value: serde_json::Value,
) {
    let mut request = json!({"window_state_id":"fresh-state", "lifetime_ms":1000});
    request[key] = value;
    assert!(serde_json::from_value::<CapturePreparationBeginRequest>(request).is_err());
}

#[rstest]
fn preparation_request_requires_fresh_metadata_reference() {
    for window_state_id in [String::new(), "x".repeat(129)] {
        assert!(
            CapturePreparationBeginRequest {
                window_state_id,
                lifetime_ms: 1000
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        CapturePreparationBeginRequest {
            window_state_id: "fresh-state".into(),
            lifetime_ms: MAX_PREPARATION_LIFETIME_MS,
        }
        .validate()
        .is_ok()
    );
}
