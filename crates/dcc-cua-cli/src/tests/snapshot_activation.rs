use dcc_cua_core::{ComputerUseErrorDetails, ComputerUseResult};
use rstest::rstest;
use serde_json::json;

use crate::actions::snapshot_activation_result;

use super::*;

fn activation_error(
    code: ComputerUseErrorCode,
    background_delivery_viable: Option<bool>,
) -> ComputerUseError {
    ComputerUseError::new(code, "activation failed").with_details(ComputerUseErrorDetails {
        background_delivery_viable,
        suggested_delivery_mode: Some("background".into()),
        ..Default::default()
    })
}

#[rstest]
fn snapshot_activation_refusal_falls_back_only_with_explicit_background_proof() {
    let activation = snapshot_activation_result(Err(activation_error(
        ComputerUseErrorCode::ForegroundActivationRefused,
        Some(true),
    )))
    .expect("explicit background viability should preserve the snapshot");

    assert_eq!(activation["status"], "refused_fallback_background");
    assert_eq!(activation["code"], "foreground_activation_refused");
    assert_eq!(activation["background_delivery_viable"], true);
    assert_eq!(activation["suggested_delivery_mode"], "background");
    assert_eq!(activation["degraded"], true);
}

#[rstest]
#[case(None)]
#[case(Some(false))]
fn snapshot_activation_refusal_without_background_proof_remains_an_error(
    #[case] background_delivery_viable: Option<bool>,
) {
    let error = activation_error(
        ComputerUseErrorCode::ForegroundActivationRefused,
        background_delivery_viable,
    );
    assert_eq!(snapshot_activation_result(Err(error.clone())), Err(error));
}

#[rstest]
fn snapshot_activation_never_falls_back_for_an_unrelated_error() {
    let error = activation_error(ComputerUseErrorCode::InputFailed, Some(true));
    assert_eq!(snapshot_activation_result(Err(error.clone())), Err(error));
}

#[rstest]
fn successful_snapshot_activation_is_preserved() {
    let activation = json!({"success": true, "target": {"pid": 42, "window_id": 7}});
    let result: ComputerUseResult<_> = Ok(activation.clone());
    assert_eq!(snapshot_activation_result(result).unwrap(), activation);
}
