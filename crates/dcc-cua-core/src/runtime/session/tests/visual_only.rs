use super::*;
use rstest::rstest;

#[cfg(windows)]
#[rstest]
fn upstream_start_timeout_is_the_only_failure_allowed_to_enter_visual_only_mode() {
    let timeout = ComputerUseError::new(
        ComputerUseErrorCode::InputFailed,
        "CUA start CUA session timed out after 15000 ms",
    );
    let backend_failure = ComputerUseError::new(
        ComputerUseErrorCode::BackendUnavailable,
        "CUA start CUA session failed before dispatch",
    );

    assert!(visual_only_start_degradation(&timeout).is_some());
    assert!(visual_only_start_degradation(&backend_failure).is_none());
}

#[cfg(windows)]
#[rstest]
#[tokio::test]
async fn visual_only_session_never_reenters_the_unresponsive_upstream_driver() {
    let (mut session, calls) = counting_session();
    session.upstream_session_state = UpstreamSessionState::VisualOnly {
        reason: "simulated upstream timeout".into(),
    };

    session
        .refresh_upstream_session_before_observation_if_needed()
        .await
        .expect("visual fallback skips upstream refresh");
    let error = session
        .require_current_upstream_session_for_evidence()
        .unwrap_err();

    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
    assert_eq!(session.status()["upstream_session"]["state"], "visual_only");
    assert_eq!(
        session.status()["upstream_session"]["requires_explicit_escalation"],
        true
    );
}
