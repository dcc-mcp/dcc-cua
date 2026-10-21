use super::*;
use rstest::rstest;
use serde_json::json;

#[rstest]
#[case("ready", true)]
#[case("pixels_only", false)]
#[case("no_observation", false)]
#[case("no_context", false)]
#[case("different_pid", false)]
#[case("different_hwnd", false)]
#[case("non_uia", false)]
#[case("accessibility_unavailable", false)]
#[case("different_error", false)]
fn explicit_semantic_root_activation_requires_its_existing_exact_uia_context(
    #[case] variant: &str,
    #[case] expected: bool,
) {
    let target = WindowTarget::from_value(&json!({
        "pid":42,"window_id":77,"bounds":{"x":0,"y":0,"width":800,"height":600},
        "is_on_screen":true,"minimized":false,"is_foreground":false
    }))
    .unwrap();
    let mut observation = ComputerUseObservation {
        observation_id: "existing-semantic-observation".into(),
        process_id: target.pid,
        window_handle: target.window_id,
        window_title: String::new(),
        width: 800,
        height: 600,
        source_rect: [0, 0, 800, 600],
        capture_backend: "windows_uia".into(),
        capture_provenance: json!({
            "accessibility_backend":"windows_uia","accessibility_available":true,
            "pixels_captured":false
        }),
        session_id: "same-session".into(),
    };
    match variant {
        "different_pid" => observation.process_id += 1,
        "different_hwnd" => observation.window_handle += 1,
        "non_uia" => observation.capture_provenance["accessibility_backend"] = json!("other"),
        "accessibility_unavailable" => {
            observation.capture_provenance["accessibility_available"] = json!(false);
        }
        _ => {}
    }
    let error = if variant == "different_error" {
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    } else {
        ComputerUseErrorCode::ForegroundActivationRefused
    };
    assert_eq!(
        semantic_root_activation_allowed(
            variant == "pixels_only",
            (variant != "no_observation").then_some(&observation),
            &target,
            variant != "no_context",
            &error,
        ),
        expected
    );
}

#[rstest]
#[case("ready", 1)]
#[case("root_failed", 1)]
#[case("root_interrupted", 1)]
#[case("pixels_only", 0)]
#[case("bootstrap_or_implicit_raw", 0)]
#[case("no_context", 0)]
#[case("different_target", 0)]
#[case("different_error", 0)]
#[tokio::test]
async fn explicit_activation_recovery_dispatches_the_existing_continuation_at_most_once(
    #[case] variant: &str,
    #[case] expected_dispatches: u32,
) {
    let target = WindowTarget::from_value(&json!({
        "pid":42,"window_id":77,"bounds":{"x":0,"y":0,"width":800,"height":600},
        "is_on_screen":true,"minimized":false,"is_foreground":false
    }))
    .unwrap();
    let mut observation = crate::observation::semantic_observation(
        "same-session",
        &target,
        &json!({"backend":"windows_uia", "accessibility_available":true}),
    );
    if variant == "different_target" {
        observation.window_handle += 1;
    }
    let code = if variant == "different_error" {
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    } else {
        ComputerUseErrorCode::ForegroundActivationRefused
    };
    let original = crate::ComputerUseError::new(code, "original native activation evidence");
    let dispatches = std::cell::Cell::new(0u32);
    let result = recover_semantic_root_activation(
        original,
        variant == "pixels_only",
        (variant != "bootstrap_or_implicit_raw").then_some(&observation),
        &target,
        variant != "no_context",
        |error| {
            dispatches.set(dispatches.get() + 1);
            std::future::ready(if variant == "root_interrupted" {
                Err(crate::ComputerUseError::new(
                    ComputerUseErrorCode::UserInterrupted,
                    "stopped",
                ))
            } else if variant == "root_failed" {
                Err(error)
            } else {
                Ok(())
            })
        },
    )
    .await;
    assert_eq!(dispatches.get(), expected_dispatches);
    if variant == "ready" {
        assert!(result.is_ok());
    } else {
        let error = result.expect_err("a refused recovery must not become success");
        if variant == "root_interrupted" {
            assert_eq!(error.code, ComputerUseErrorCode::UserInterrupted);
        } else {
            assert_eq!(error.message, "original native activation evidence");
        }
    }
}

#[rstest]
#[case(
    crate::ComputerUseSemanticRootFocusStatus::DispatchUnknown,
    true,
    false,
    true
)]
#[case(
    crate::ComputerUseSemanticRootFocusStatus::NotFocusable,
    false,
    false,
    false
)]
#[case(
    crate::ComputerUseSemanticRootFocusStatus::ForegroundRefused,
    true,
    true,
    false
)]
fn semantic_root_activation_failure_keeps_native_evidence_and_unknown_dispatch_state(
    #[case] status: crate::ComputerUseSemanticRootFocusStatus,
    #[case] focus_attempted: bool,
    #[case] provider_completed: bool,
    #[case] unknown: bool,
) {
    let mut diagnostic: crate::ComputerUseForegroundActivationDiagnostic =
        serde_json::from_value(json!({
            "caller_process_id":1,"caller_thread_id":2,
            "target":{"process_id":42,"window_handle":77},"target_thread_id":8,
            "target_root_window_handle":77,"target_owner_window_handle":0,
            "initial_foreground":null,"final_foreground":null,
            "attempts":[{"phase":"initial_foreground_request","api_return":0}],
            "foreground_poll_count":20
        }))
        .unwrap();
    let original_attempts = diagnostic.attempts.clone();
    diagnostic.semantic_root_focus = Some(crate::ComputerUseSemanticRootFocusDiagnostic {
        status,
        focus_attempted: Some(focus_attempted),
        provider_completed,
        foreground_observed: None,
    });
    let error = with_semantic_root_activation_diagnostic(
        crate::ComputerUseError::new(
            ComputerUseErrorCode::ForegroundActivationRefused,
            "original native refusal",
        ),
        diagnostic,
    );
    assert_eq!(
        error.code,
        ComputerUseErrorCode::ForegroundActivationRefused
    );
    assert_eq!(error.message, "original native refusal");
    let details = error.details.unwrap();
    assert_eq!(details.effect_unknown, Some(unknown));
    assert_eq!(
        details.completion,
        Some(if unknown {
            crate::ComputerUseCompletionState::Unknown
        } else {
            crate::ComputerUseCompletionState::Known
        })
    );
    assert_eq!(details.focus_mutation_attempted, Some(true));
    assert_eq!(
        details.input_sent,
        Some(crate::ComputerUseInputState::NotSent)
    );
    assert_eq!(details.blind_retry, Some(false));
    assert_eq!(details.automatic_input, Some(false));
    assert_eq!(
        details.foreground_activation.unwrap().attempts,
        original_attempts
    );
}

#[rstest]
#[case(crate::ComputerUseSemanticRootFocusStatus::ProviderRejected, false)]
#[case(crate::ComputerUseSemanticRootFocusStatus::PostconditionFailed, true)]
fn semantic_root_activation_attempted_failure_does_not_claim_known_effects(
    #[case] status: crate::ComputerUseSemanticRootFocusStatus,
    #[case] provider_completed: bool,
) {
    let diagnostic: crate::ComputerUseForegroundActivationDiagnostic =
        serde_json::from_value(json!({
            "caller_process_id":1,"caller_thread_id":2,
            "target":{"process_id":42,"window_handle":77},"target_thread_id":8,
            "target_root_window_handle":77,"target_owner_window_handle":0,
            "initial_foreground":null,"final_foreground":null,
            "attempts":[{"phase":"initial_foreground_request","api_return":0}],
            "foreground_poll_count":20,
            "semantic_root_focus":{
                "status":status,"focus_attempted":true,
                "provider_completed":provider_completed,"foreground_observed":null
            }
        }))
        .unwrap();
    let expected_focus_diagnostic = diagnostic.semantic_root_focus.clone();
    let error = with_semantic_root_activation_diagnostic(
        crate::ComputerUseError::new(
            ComputerUseErrorCode::ForegroundActivationRefused,
            "original native refusal",
        ),
        diagnostic,
    );
    assert_eq!(
        error.code,
        ComputerUseErrorCode::ForegroundActivationRefused
    );
    let details = error.details.unwrap();
    assert_eq!(details.effect_unknown, Some(true));
    assert_eq!(
        details.completion,
        Some(crate::ComputerUseCompletionState::Unknown)
    );
    assert_eq!(
        details.input_sent,
        Some(crate::ComputerUseInputState::NotSent)
    );
    assert_eq!(details.automatic_input, Some(false));
    assert_eq!(details.blind_retry, Some(false));
    assert_eq!(
        details.foreground_activation.unwrap().semantic_root_focus,
        expected_focus_diagnostic
    );
}
