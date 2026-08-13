use super::*;
use rstest::rstest;

#[cfg(windows)]
#[rstest]
#[tokio::test]
async fn approved_pixel_fallback_does_not_retry_the_timed_out_uia_provider() {
    let (mut session, _calls) = counting_session();
    session.uia_timeout_escalated = true;
    session.windows_uia = None;
    let target = session.target.clone().expect("exact test target");

    let accessibility = session
        .visual_fallback_accessibility(&target, 5_000, 64, "exact_window_wgc")
        .await;

    assert_eq!(accessibility["accessibility_available"], false);
    assert_eq!(accessibility["degraded"], true);
    assert_eq!(accessibility["fallback"], "exact_window_wgc");
    assert!(session.windows_uia.is_none());
}

#[rstest]
#[tokio::test]
async fn vanished_target_skips_upstream_stop_for_the_dead_window() {
    let (mut session, calls) = counting_session();

    let error = session
        .finish_observation_sensitive_attempt::<()>(Err(ComputerUseError::new(
            ComputerUseErrorCode::MissingWindow,
            "simulated closed window",
        )))
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::MissingWindow);
    assert!(session.target.is_none());
    session.stop().await.expect("local stop completes");
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    assert!(!session.active);
}
