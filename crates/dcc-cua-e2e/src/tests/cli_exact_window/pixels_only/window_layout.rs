//! Real native maximize/restore/frame readback in the existing exclusive CI fixture.
use super::{bounded_session, native_fixture::Fixture};
use dcc_cua_core::{
    ComputerUseDriver, ComputerUseErrorCode, ComputerUseInputState, ComputerUseTargetScope,
    ComputerUseWindowFrameRequest,
};
#[allow(unused_imports)]
use rstest::rstest;

fn state(fixture: &Fixture) -> dcc_cua_platform_windows::ExactWindowNativeState {
    dcc_cua_platform_windows::exact_window_native_state(std::process::id(), fixture.target)
        .expect("read disposable fixture's exact native instance")
}

pub(super) async fn verify(fixture: &Fixture, driver: &ComputerUseDriver) {
    let original = state(fixture);
    assert!(!original.maximized && !original.minimized);
    let [x, y, width, height] = original.bounds.unwrap();
    let requested = [x + 8, y + 8, width, height];
    let request = ComputerUseWindowFrameRequest {
        x: f64::from(requested[0]),
        y: f64::from(requested[1]),
        width: f64::from(width),
        height: f64::from(height),
    };
    let mut session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(std::process::id()),
                window_handle: Some(fixture.target),
                window_title: None,
            },
            "window layout fixture",
            "pixels-only-window-layout",
        )
        .unwrap();
    bounded_session(session.start_pixels_only()).await.unwrap();
    let normal_metadata = bounded_session(session.native_window_state_for_frame())
        .await
        .unwrap();
    assert_eq!(normal_metadata["maximized"], false);
    let normal_token = normal_metadata["window_state_id"].as_str().unwrap();
    let key_baseline = fixture.target_key_events();

    fixture.maximize_target();
    let maximized = state(fixture);
    assert!(maximized.maximized && !maximized.minimized);
    assert_eq!(maximized.instance, original.instance);
    let error = bounded_session(session.set_window_frame_from_native_state(normal_token, &request))
        .await
        .expect_err("fresh normal metadata cannot authorize a now-maximized frame");
    assert_eq!(error.code, ComputerUseErrorCode::TargetUnavailable);
    assert!(error.message.contains("restore_activate"), "{error}");
    let details = error.details.unwrap();
    assert_eq!(details.action_attempted, Some(false));
    assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
    assert_eq!(details.effect_unknown, Some(false));
    let still_maximized = state(fixture);
    assert!(still_maximized.maximized);
    assert_eq!(still_maximized.bounds, maximized.bounds);
    let metadata = bounded_session(session.native_window_state_for_frame())
        .await
        .unwrap();
    assert_eq!(metadata["maximized"], true);
    assert!(metadata.get("window_state_id").is_none());

    let restored = bounded_session(session.restore_activate()).await.unwrap();
    assert_eq!(restored["success"], true);
    assert_eq!(restored["fresh_observation_required"], true);
    let normal = state(fixture);
    assert!(!normal.minimized && !normal.maximized && normal.visible && normal.foreground);
    assert_eq!(normal.instance, original.instance);
    // SW_RESTORE from a minimized, previously maximized window can re-enter
    // maximized state. The same explicit operation must normalize both stages.
    fixture.maximize_target();
    assert!(state(fixture).maximized);
    fixture.minimize_target();
    assert!(state(fixture).minimized);
    bounded_session(session.restore_activate()).await.unwrap();
    let normal = state(fixture);
    assert!(!normal.minimized && !normal.maximized && normal.visible && normal.foreground);
    assert_eq!(normal.instance, original.instance);
    let metadata = bounded_session(session.native_window_state_for_frame())
        .await
        .unwrap();
    assert_eq!(metadata["maximized"], false);
    let confirmation = bounded_session(session.set_window_frame_from_native_state(
        metadata["window_state_id"].as_str().unwrap(),
        &request,
    ))
    .await
    .unwrap();
    assert_eq!(confirmation["effect"], "confirmed");
    assert_eq!(confirmation["state"]["maximized"], false);
    let applied = state(fixture);
    assert_eq!(applied.bounds, Some(requested));
    assert_eq!(applied.instance, original.instance);
    assert!(!applied.minimized && !applied.maximized);
    let proof = dcc_cua_platform_windows::native_capture_proof(std::process::id(), fixture.target);
    assert!(!proof.pixels_read && !proof.input_sent && !proof.authorizes_capture_or_input);
    for phase in [proof.before_flush.unwrap(), proof.after_flush.unwrap()] {
        assert!(phase.instance_stable_within_phase);
        assert_eq!(phase.instance, Some(original.instance));
        let layout = phase.target_layout.unwrap();
        assert!(!layout.maximized);
        assert_eq!(layout.client_rect_edges, Some([0, 0, width, height]));
        assert_eq!(
            layout.client_screen_rect_edges,
            Some([
                requested[0],
                requested[1],
                requested[0] + width,
                requested[1] + height,
            ])
        );
        assert!(layout.client_rect_hresult.is_none() && layout.client_screen_win32_error.is_none());
    }
    assert_eq!(
        fixture.target_key_events(),
        key_baseline,
        "window state operations injected keys"
    );
    let stopped = bounded_session(session.stop()).await.unwrap();
    assert!(stopped.success && !stopped.active && !stopped.cleanup_pending);
    eprintln!(
        "native window layout: maximized frame refused before dispatch; explicit restore cleared both states; fresh frame/client physical readback confirmed with zero key events"
    );
}
