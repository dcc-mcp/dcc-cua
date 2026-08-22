use super::*;
use rstest::rstest;

fn active_desktop_session() -> ComputerUseDesktopSession {
    let mut session = ComputerUseDesktopSession::new(
        ComputerUseDriver::create().expect("test driver"),
        "Agent".into(),
        "desktop-test".into(),
    )
    .expect("desktop session");
    session.active = true;
    session
}

fn desktop_click(observation_id: Option<&str>) -> ComputerUseAction {
    ComputerUseAction {
        action: "click".into(),
        x: Some(10.0),
        y: Some(20.0),
        observation_id: observation_id.map(str::to_owned),
        ..Default::default()
    }
}

#[rstest]
#[tokio::test]
async fn desktop_action_without_any_snapshot_is_rejected() {
    let mut session = active_desktop_session();

    let error = session
        .perform_action(&desktop_click(None))
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
}

#[rstest]
#[tokio::test]
async fn desktop_action_cannot_reuse_an_observation_after_it_is_consumed() {
    let mut session = active_desktop_session();
    session.latest_observation_id = None;

    let error = session
        .perform_action(&desktop_click(Some("consumed-observation")))
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
}
