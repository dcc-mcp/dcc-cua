use super::*;
use rstest::rstest;

#[rstest]
fn windows_local_click_maps_snapshot_pixels_to_native_window_pixels() {
    let observation = ComputerUseObservation {
        session_id: "session".into(),
        observation_id: "observation".into(),
        process_id: 7,
        window_handle: 11,
        window_title: "scaled window".into(),
        width: 1568,
        height: 852,
        source_rect: [-7691, -6, 3862, 2110],
        capture_backend: "cua-driver-sdk".into(),
        capture_provenance: json!({
            "accessibility_available": true,
            "backend": "cua-driver-sdk",
            "pixels_captured": true,
            "scope": "window"
        }),
    };
    let action = ComputerUseAction {
        action: "click".into(),
        delivery_mode: Some("foreground".into()),
        x: Some(590.0),
        y: Some(250.0),
        ..Default::default()
    };

    assert!(uses_windows_local_foreground_path(&action));
    let mapped = action_for_window_visual_fallback(&action, &observation).unwrap();
    assert!((mapped.x.unwrap() - 1_453.176_020_408_163_4).abs() < 0.001);
    assert!((mapped.y.unwrap() - 619.131_455_399_061).abs() < 0.001);
}

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

    assert_exact_target_revalidation(&mut session, calls, "click", None).await;
}

#[cfg(windows)]
#[rstest]
#[tokio::test]
async fn due_upstream_refresh_does_not_fence_a_foreground_coordinate_action() {
    let (mut session, calls) = counting_session();

    assert_exact_target_revalidation(&mut session, calls, "click", Some("foreground")).await;
}

#[rstest]
#[tokio::test]
async fn due_upstream_refresh_does_not_fence_a_foreground_cursor_move() {
    let (mut session, calls) = counting_session();

    assert_exact_target_revalidation(&mut session, calls, "move", Some("foreground")).await;
}

#[cfg(windows)]
async fn assert_exact_target_revalidation(
    session: &mut ComputerUseSession,
    calls: Arc<AtomicUsize>,
    action: &str,
    delivery_mode: Option<&str>,
) {
    let error = session
        .perform_action(&ComputerUseAction {
            action: action.into(),
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
