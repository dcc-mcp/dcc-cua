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

/// Validate real SDK wire shapes without launching a process.
#[rstest]
#[tokio::test]
async fn launch_mapping_matches_the_actual_native_sdk_schema() {
    let inventory = super::sdk_action_contract::native_sdk_inventory().await;
    let schema = super::tool_schema_from_inventory(&inventory, "launch_app").unwrap();
    let properties = schema["properties"].as_object().unwrap();
    for selector in ["name", "bundle_id", "aumid", "path", "launch_path"] {
        if !properties.contains_key(selector) {
            continue;
        }
        let mut value = serde_json::json!({
            "urls":["https://example.com"],
            "additional_arguments":["--test"],
            "creates_new_application_instance":cfg!(any(windows, target_os = "macos")),
            "start_minimized":cfg!(windows),
        });
        value[selector] = serde_json::json!("FixtureApp");
        let request = serde_json::from_value(value).unwrap();
        for session in [None, Some("launch-schema-session")] {
            let arguments = launch_arguments(&request, session).unwrap();
            for key in arguments
                .as_object()
                .unwrap()
                .keys()
                .filter(|key| *key != "session")
            {
                assert!(
                    properties.contains_key(key),
                    "unknown native launch argument: {key}"
                );
            }
            assert_eq!(
                arguments.get("session").and_then(serde_json::Value::as_str),
                session
            );
            assert_eq!(arguments[selector], "FixtureApp");
            assert_eq!(
                arguments["additional_arguments"],
                serde_json::json!(["--test"])
            );
            assert_eq!(
                arguments["urls"],
                serde_json::json!(["https://example.com"])
            );
        }
    }
}

#[rstest]
#[case("creates_new_application_instance", cfg!(any(windows, target_os = "macos")))]
#[case("start_minimized", cfg!(windows))]
fn launch_never_discards_an_explicit_unsupported_option(
    #[case] key: &str,
    #[case] supported: bool,
) {
    let mut value = serde_json::json!({"name":"FixtureApp"});
    value[key] = serde_json::json!(true);
    let request = serde_json::from_value(value).unwrap();
    let result = launch_arguments(&request, None);
    if supported {
        assert_eq!(result.unwrap()[key], true);
    } else {
        let error = result.unwrap_err();
        assert_eq!(
            error.code,
            crate::contracts::ComputerUseErrorCode::InvalidAction
        );
        assert!(error.message.contains(key));
    }
}
