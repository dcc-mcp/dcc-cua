use std::time::Duration;

use rstest::rstest;

use crate::contracts::ComputerUseLaunchRequest;
use crate::runtime::application::{launch_arguments, validate_launch_request};
use crate::runtime::driver_call_timeout;

#[rstest]
fn application_launch_has_a_separate_bounded_driver_deadline() {
    let launch_timeout = driver_call_timeout("launch_app");
    let ordinary_timeout = driver_call_timeout("list_apps");

    assert_eq!(ordinary_timeout, Duration::from_secs(15));
    assert_eq!(launch_timeout, Duration::from_secs(35));
    assert!(launch_timeout > ordinary_timeout);
}

#[rstest]
fn launch_requires_one_safe_application_selector() {
    assert!(validate_launch_request(&ComputerUseLaunchRequest::default()).is_err());
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            name: Some("Calculator".into()),
            ..Default::default()
        })
        .is_ok()
    );
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            name: Some("Calculator".into()),
            bundle_id: Some("com.example.Calculator".into()),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            urls: vec!["com.epicgames.launcher://fab/plugins/egl".into()],
            ..Default::default()
        })
        .is_ok()
    );
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            urls: vec!["file:///C:/Windows/System32/cmd.exe".into()],
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            launch_path: Some("powershell.exe".into()),
            ..Default::default()
        })
        .is_err()
    );
    let json = serde_json::to_value(ComputerUseLaunchRequest {
        name: Some("Calculator".into()),
        ..Default::default()
    })
    .expect("launch request should serialize");
    assert!(json.get("bundle_id").is_none());
    let scoped = launch_arguments(
        &ComputerUseLaunchRequest {
            name: Some("Calculator".into()),
            ..Default::default()
        },
        Some("private-runtime-session"),
    )
    .expect("session-scoped launch arguments");
    assert_eq!(scoped["session"], "private-runtime-session");
}

#[rstest]
#[case("consent.exe")]
#[case("wt.exe")]
#[case("wsl.exe")]
#[case("conhost.exe")]
fn launch_denies_sensitive_executable_identities(#[case] executable: &str) {
    let error = validate_launch_request(&ComputerUseLaunchRequest {
        launch_path: Some(executable.into()),
        ..Default::default()
    })
    .expect_err("sensitive executable identities must be denied");

    assert_eq!(
        error.code,
        crate::contracts::ComputerUseErrorCode::InvalidTarget
    );
}

#[rstest]
fn launch_denies_command_interpreters_with_arguments() {
    let error = validate_launch_request(&ComputerUseLaunchRequest {
        launch_path: Some("C:\\Python313\\python.exe".into()),
        additional_arguments: vec!["-c".into(), "print('unsafe')".into()],
        ..Default::default()
    })
    .expect_err("command interpreters must not be launched through arbitrary arguments");

    assert_eq!(
        error.code,
        crate::contracts::ComputerUseErrorCode::InvalidTarget
    );
}
