use super::*;
use rstest::rstest;

#[cfg(windows)]
#[rstest]
#[tokio::test]
async fn due_upstream_refresh_does_not_fence_a_local_visual_action() {
    let (mut session, calls) = counting_session();
    session
        .observation
        .as_mut()
        .expect("seeded observation")
        .capture_provenance = json!({
        "accessibility_available": false,
        "backend": "windows_graphics_capture",
        "scope": "window",
    });

    assert_exact_target_revalidation(&mut session, calls, None).await;
}

#[cfg(windows)]
#[rstest]
#[tokio::test]
async fn due_upstream_refresh_does_not_fence_a_foreground_coordinate_action() {
    let (mut session, calls) = counting_session();

    assert_exact_target_revalidation(&mut session, calls, Some("foreground")).await;
}

#[cfg(windows)]
async fn assert_exact_target_revalidation(
    session: &mut ComputerUseSession,
    calls: Arc<AtomicUsize>,
    delivery_mode: Option<&str>,
) {
    let error = session
        .perform_action(&ComputerUseAction {
            action: "click".into(),
            observation_id: Some("observation-before-refresh".into()),
            delivery_mode: delivery_mode.map(str::to_owned),
            x: Some(10.0),
            y: Some(10.0),
            ..Default::default()
        })
        .await
        .expect_err("the synthetic HWND must still fail exact-target revalidation");

    assert_eq!(error.code, ComputerUseErrorCode::MissingWindow);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    assert!(session.observation.is_none());
}
