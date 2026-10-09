use rstest::rstest;

use super::*;

#[rstest]
#[case(false)]
#[case(true)]
fn passive_prepared_frame_never_becomes_actionable(#[case] actual_foreground: bool) {
    let proof = dcc_cua_showcase::PreparedCaptureFrameProvenance {
        preparation_id: [7; 16],
        actual_foreground,
        captured_at_ms: 42,
    };
    let error = require_actionable_native_frame(Some(&proof)).unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::InvalidAction);
    let details = error.details.unwrap();
    assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
    assert_eq!(details.action_attempted, Some(false));
    assert_eq!(details.automatic_rebind, Some(false));
}

#[rstest]
fn ordinary_frame_has_no_passive_preparation_authority() {
    assert!(require_actionable_native_frame(None).is_ok());
}
