use super::*;
use rstest::rstest;

#[rstest]
#[tokio::test]
async fn unknown_action_transport_completion_invalidates_the_exact_session() {
    let (mut session, calls) = counting_session();
    let evidence_epoch_before_dispatch = session.action_evidence_epoch();

    let error = session
        .finish_typed_dispatch_result::<()>(
            "execute CUA click",
            Ok(Err(DriverError::ActionInterrupted {
                completion: ActionCompletion::Unknown,
                reason: "simulated action response loss".into(),
            })),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::CompletionUnknown);
    let details = error
        .details
        .as_ref()
        .expect("unknown completion must expose structured action metadata");
    assert_eq!(details.phase, Some(ComputerUseErrorPhase::ActionDispatch));
    assert_eq!(details.action_attempted, Some(true));
    assert_eq!(details.input_sent, Some(ComputerUseInputState::Unknown));
    assert_eq!(
        details.completion,
        Some(ComputerUseCompletionState::Unknown)
    );
    assert_eq!(details.local_session_invalidated, Some(true));
    assert_eq!(details.blind_retry, Some(false));
    assert_eq!(details.fresh_observation_required, Some(true));
    assert!(!error.message.contains("completion_unknown="));
    assert!(!session.active);
    assert!(session.target.is_none());
    assert!(session.action_evidence_epoch() > evidence_epoch_before_dispatch);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn outer_action_transport_timeout_invalidates_the_exact_session() {
    let (mut session, calls) = counting_session();

    let error = session
        .finish_typed_dispatch_result::<()>(
            "execute CUA click",
            Err(ComputerUseError::new(
                ComputerUseErrorCode::InputFailed,
                "execute CUA click timed out",
            )),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::CompletionUnknown);
    let details = structured_error_details(&error);
    assert_eq!(details.action_attempted, Some(true));
    assert_eq!(details.blind_retry, Some(false));
    assert!(!session.active);
    assert!(session.target.is_none());
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn not_started_action_transport_failure_preserves_the_exact_session() {
    let (mut session, calls) = counting_session();
    let evidence_epoch_before_dispatch = session.action_evidence_epoch();

    let error = session
        .finish_typed_dispatch_result::<()>(
            "execute CUA click",
            Ok(Err(DriverError::ActionInterrupted {
                completion: ActionCompletion::NotStarted,
                reason: "simulated pre-dispatch rejection".into(),
            })),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::InputFailed);
    let details = error
        .details
        .as_ref()
        .expect("pre-dispatch failure must expose structured action metadata");
    assert_eq!(details.phase, Some(ComputerUseErrorPhase::PreDispatch));
    assert_eq!(details.action_attempted, Some(false));
    assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
    assert_eq!(details.completion, Some(ComputerUseCompletionState::Known));
    assert_eq!(details.blind_retry, Some(false));
    assert_eq!(details.fresh_observation_required, Some(false));
    assert!(!error.message.contains("phase="));
    assert!(!error.message.contains("action_attempted="));
    assert!(session.active);
    assert_eq!(session.target.as_ref().map(|target| target.pid), Some(42));
    assert!(session.observation.is_some());
    assert_eq!(
        session.action_evidence_epoch(),
        evidence_epoch_before_dispatch
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}
