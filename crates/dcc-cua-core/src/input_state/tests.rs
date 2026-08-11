use rstest::rstest;
use serde_json::json;

use super::*;

#[rstest]
fn diagnostic_maps_to_typed_ready_or_suspended_input_readiness() {
    let ready = ComputerUseInputReadiness::from_diagnostic(&json!({
        "success": true,
        "code": "interactive_desktop_ready",
        "message": "ready",
        "input_ready": true
    }));
    assert_eq!(ready.status, ComputerUseInputStatus::Ready);
    assert_eq!(ready.code, "interactive_desktop_ready");
    assert_eq!(ready.reason, None);

    let suspended = ComputerUseInputReadiness::from_diagnostic(&json!({
        "success": false,
        "code": "interactive_desktop_unknown",
        "message": "Windows input desktop could not be read",
        "input_ready": false,
        "input_code": "interactive_input_surface_unavailable",
        "input_message": "OpenInputDesktop: access denied"
    }));
    assert_eq!(suspended.status, ComputerUseInputStatus::Suspended);
    assert_eq!(suspended.code, "interactive_input_surface_unavailable");
    assert_eq!(
        suspended.reason.as_deref(),
        Some("OpenInputDesktop: access denied")
    );
}

#[rstest]
fn initial_session_input_state_binds_exact_target_and_sequence() {
    let state = ComputerUseSessionInputState::initial(
        ComputerUseInputTarget {
            session_id: "session-1".into(),
            process_id: 42,
            window_handle: 77,
        },
        ComputerUseInputReadiness {
            status: ComputerUseInputStatus::Ready,
            code: "interactive_desktop_ready".into(),
            reason: None,
        },
        1_723_000,
    );

    assert_eq!(state.sequence, 1);
    assert_eq!(state.observed_at, 1_723_000);
    assert_eq!(state.target.session_id, "session-1");
    assert_eq!(state.target.process_id, 42);
    assert_eq!(state.target.window_handle, 77);
    assert_eq!(state.status, ComputerUseInputStatus::Ready);
}

#[rstest]
fn minimized_target_state_is_orthogonal_to_interactive_input_readiness() {
    let target = ComputerUseInputTarget {
        session_id: "session-1".into(),
        process_id: 42,
        window_handle: 77,
    };
    let input = ComputerUseSessionInputState::initial(
        target.clone(),
        ComputerUseInputReadiness {
            status: ComputerUseInputStatus::Ready,
            code: "interactive_desktop_ready".into(),
            reason: None,
        },
        100,
    );
    let target_state = ComputerUseSessionTargetState::initial(
        target,
        ComputerUseTargetAvailability {
            status: ComputerUseTargetStatus::Minimized,
            code: "target_minimized".into(),
            visible: false,
            minimized: true,
            foreground: false,
        },
        100,
    );

    assert_eq!(input.status, ComputerUseInputStatus::Ready);
    assert_eq!(target_state.status, ComputerUseTargetStatus::Minimized);
    assert!(!target_state.visible);
    assert!(target_state.minimized);
}

#[rstest]
fn minimized_target_recovery_is_explicit_and_never_retries_input_automatically() {
    let requirements = ComputerUseTargetRecoveryRequirements::explicit_restore_activate();

    assert!(!requirements.automatic_input);
    assert!(requirements.explicit_request_required);
    assert_eq!(requirements.operation, "restore_activate");
    assert!(requirements.exact_target_revalidation);
    assert!(requirements.fresh_observation);
    assert!(requirements.foreground_validation);
    assert!(!requirements.blind_retry);
}

#[rstest]
fn hidden_non_minimized_target_is_not_reported_available() {
    let state = ComputerUseTargetAvailability::from_window_state(&json!({
        "visible": false,
        "minimized": false,
        "foreground": false
    }));

    assert_eq!(state.status, ComputerUseTargetStatus::Unavailable);
    assert_eq!(state.code, "target_unavailable");
}

#[rstest]
fn missing_visibility_evidence_fails_closed_as_target_unavailable() {
    let state = ComputerUseTargetAvailability::from_window_state(&json!({
        "minimized": false,
        "foreground": false
    }));

    assert_eq!(state.status, ComputerUseTargetStatus::Unavailable);
    assert!(!state.visible);
}
