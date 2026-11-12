use rstest::rstest;
use serde_json::json;

use crate::failure_output::{OVERLAY_EXCLUSION_MESSAGE, PUBLIC_FAILURE_MESSAGE, fatal_error_value};

#[rstest]
fn incomplete_window_discovery_has_a_structured_non_absence_failure() {
    let error = dcc_cua_core::ComputerUseError::new(
        dcc_cua_core::ComputerUseErrorCode::TargetUnavailable,
        "private arbitrary error",
    )
    .with_details(dcc_cua_core::ComputerUseErrorDetails {
        window_inventory: Some(dcc_cua_core::ComputerUseWindowInventoryDiagnostic {
            complete: false,
            reason: "EnumWindows: os error 5".into(),
            partial_candidates: vec![dcc_cua_core::ComputerUseWindowInventoryCandidate {
                process_id: Some(42),
                window_handle: 7,
            }],
        }),
        ..Default::default()
    });
    let value = fatal_error_value(&error);
    assert_eq!(value["error"]["code"], "window_discovery_unknown");
    assert_eq!(
        value["error"]["details"]["window_inventory"]["complete"],
        false
    );
    assert_eq!(
        value["error"]["details"]["window_inventory"]["partial_candidates"][0]["process_id"],
        42
    );
    assert_eq!(value["error"]["details"]["automatic_relaunch"], false);
    assert!(!value.to_string().contains("private arbitrary error"));
}

use super::*;

#[rstest]
#[case(std::io::ErrorKind::NotFound, true)]
#[case(std::io::ErrorKind::ConnectionRefused, true)]
#[case(std::io::ErrorKind::PermissionDenied, false)]
#[case(std::io::ErrorKind::TimedOut, false)]
#[case(std::io::ErrorKind::WouldBlock, false)]
#[case(std::io::ErrorKind::ConnectionReset, false)]
#[case(std::io::ErrorKind::BrokenPipe, false)]
#[case(std::io::ErrorKind::InvalidInput, false)]
#[case(std::io::ErrorKind::Other, false)]
fn missing_host_guidance_requires_an_absent_listener_at_connect_time(
    #[case] kind: std::io::ErrorKind,
    #[case] absent: bool,
) {
    for connecting in [false, true] {
        let io = std::io::Error::new(kind, "REVIEW_PRIVATE_ENDPOINT_AND_OS_MESSAGE");
        let error = if connecting {
            HostClientError::Connect(io)
        } else {
            HostClientError::Io(io)
        };
        let value = fatal_error_value(&error);
        if connecting && absent {
            assert_eq!(value["error"]["code"], "host_not_running");
            assert_eq!(value["error"]["hint"], "dcc-cua host-ensure");
        } else {
            assert_eq!(value["error"]["code"], "host_transport_failed");
            assert_eq!(value["error"]["message"], PUBLIC_FAILURE_MESSAGE);
            assert!(value["error"].get("hint").is_none());
        }
        assert!(!value.to_string().contains("REVIEW_PRIVATE_"));
    }
}

#[rstest]
fn doctor_failures_preserve_the_diagnostics_native_stdout_boundary() {
    assert!(
        terminal_error_output(&[OsString::from("doctor")]) == TerminalErrorOutput::ProtocolNative
    );
}

#[rstest]
fn profile_state_watch_failures_preserve_the_streaming_stdout_boundary() {
    assert!(
        terminal_error_output(&[
            OsString::from("profile-state"),
            OsString::from("--profile-file"),
            OsString::from("fixture.json"),
            OsString::from("--watch"),
        ]) == TerminalErrorOutput::ProtocolNative
    );
}

#[rstest]
fn profile_state_single_read_failures_keep_the_one_shot_envelope() {
    assert!(
        terminal_error_output(&[
            OsString::from("profile-state"),
            OsString::from("--profile-file"),
            OsString::from("fixture.json"),
        ]) == TerminalErrorOutput::OneShotEnvelope
    );
}

#[rstest]
fn typed_one_shot_failures_keep_only_allowlisted_identity() {
    let error = ComputerUseError::new(
        ComputerUseErrorCode::StaleObservation,
        "REVIEW_PRIVATE_MESSAGE_41c9a0",
    )
    .with_details(dcc_cua_core::ComputerUseErrorDetails {
        suggested_delivery_mode: Some("REVIEW_PRIVATE_DETAIL_7a321e".into()),
        ..Default::default()
    });
    let line = fatal_error_line(&error);
    let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON error line");

    assert!(!line.contains('\n'));
    assert_eq!(value["success"], false);
    assert_eq!(value["error"]["code"], "stale_observation");
    let guidance = value["error"]["message"].as_str().unwrap();
    assert!(guidance.contains("persistent MCP task"));
    assert!(guidance.contains("same live task"));
    assert!(guidance.contains("one-shot CLI"));
    assert!(guidance.contains("without reusing the old opaque token"));
    assert!(guidance.contains("separate snapshot invocation cannot renew it"));
    assert!(value["error"]["details"].is_null());
    assert!(!line.contains("REVIEW_PRIVATE_"));
}

#[rstest]
fn missing_provider_failure_publishes_only_fixed_fallback_guidance() {
    let error = ComputerUseError::new(
        ComputerUseErrorCode::NoAccessibilityProvider,
        "REVIEW_PRIVATE_PROVIDER_DETAIL_42f97a",
    );
    let value = fatal_error_value(&error);

    assert_eq!(value["error"]["code"], "no_accessibility_provider");
    assert_eq!(
        value["error"]["message"],
        "The exact window has no usable accessibility provider."
    );
    assert_eq!(value["error"]["details"]["retryable"], false);
    assert_eq!(
        value["error"]["details"]["permanent_for_window_class"],
        true
    );
    assert_eq!(
        value["error"]["details"]["fallback_command"],
        "snapshot --pixels-only"
    );
    assert_eq!(
        value["error"]["details"]["fallback_requires"],
        "ocr_or_another_perception_layer"
    );
    assert!(!value.to_string().contains("REVIEW_PRIVATE_"));
}

#[rstest]
fn overlay_exclusion_failure_publishes_stable_retry_guidance() {
    let error = ComputerUseError::new(
        ComputerUseErrorCode::OverlayExclusionUnavailable,
        "REVIEW_PRIVATE_OVERLAY_OWNER_92af10",
    );
    let value = fatal_error_value(&error);

    assert_eq!(value["error"]["code"], "overlay_exclusion_unavailable");
    assert_eq!(
        value["error"]["message"],
        "Another DCC-CUA cursor overlay did not acknowledge capture exclusion."
    );
    assert_eq!(value["error"]["details"]["retryable"], true);
    assert_eq!(value["error"]["details"]["scope_preserved"], "exact_window");
    assert_eq!(value["error"]["details"]["whole_desktop_fallback"], false);
    assert!(!value.to_string().contains("REVIEW_PRIVATE_"));
}

#[rstest]
fn remote_overlay_exclusion_failure_keeps_the_same_allowlisted_identity() {
    let error = HostClientError::Remote {
        code: "overlay_exclusion_unavailable".into(),
        message: "REVIEW_PRIVATE_REMOTE_OVERLAY_3c64b1".into(),
        response: json!({"private": "REVIEW_PRIVATE_REMOTE_RESPONSE_a72c8f"}),
    };
    let value = fatal_error_value(&error);

    assert_eq!(value["error"]["code"], "overlay_exclusion_unavailable");
    assert_eq!(value["error"]["message"], OVERLAY_EXCLUSION_MESSAGE);
    assert!(!value.to_string().contains("REVIEW_PRIVATE_"));
}

#[rstest]
fn panic_boundary_returns_a_fixed_safe_machine_envelope() {
    let failure = run_command_boundary(|| panic!("private panic payload"))
        .expect_err("panic should fail closed");
    let CommandFailure::Panic(line) = failure else {
        panic!("panic should retain its terminal failure category");
    };
    let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON error line");

    assert_eq!(value["success"], false);
    assert_eq!(value["error"]["code"], "internal_failure");
    assert!(!line.contains("private panic payload"));
}

#[rstest]
fn generic_error_boundary_does_not_publish_private_error_text() {
    let failure =
        run_command_boundary(|| Err(std::io::Error::other("REVIEW_PRIVATE_ERROR_74c291").into()))
            .expect_err("ordinary error should fail closed");
    let CommandFailure::Command(line) = failure else {
        panic!("ordinary errors should retain their terminal failure category");
    };
    let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON error line");

    assert_eq!(value["error"]["code"], "command_failed");
    assert_eq!(
        value["error"]["message"],
        "dcc-cua could not complete the command"
    );
    assert!(
        !line.contains("REVIEW_PRIVATE_ERROR_74c291"),
        "generic internal error text leaked into the machine envelope: {line}"
    );
}

#[rstest]
fn pixels_only_escalation_conflict_publishes_safe_typed_guidance() {
    let value = fatal_error_value(&CliUsageError::PixelsOnlyConflictsWithEscalation);

    assert_eq!(value["error"]["code"], "invalid_argument_combination");
    assert_eq!(
        value["error"]["message"],
        "snapshot --pixels-only skips accessibility and cannot be combined with --escalate"
    );
    assert_eq!(
        value["error"]["details"]["incompatible_options"],
        json!(["--escalate"])
    );
}

#[rstest]
fn native_capture_failure_preserves_only_typed_content_free_proof() {
    use dcc_cua_core::{
        ComputerUseCaptureDiagnostic, ComputerUseCaptureReason, ComputerUseCaptureStage,
    };
    let error = ComputerUseError::new(
        ComputerUseErrorCode::InvalidTarget,
        "PRIVATE_FOREIGN_TITLE_AND_BACKEND_MESSAGE",
    )
    .with_details(dcc_cua_core::ComputerUseErrorDetails {
        capture: Some(ComputerUseCaptureDiagnostic {
            stage: ComputerUseCaptureStage::VisibleDesktopProof,
            reason: ComputerUseCaptureReason::RootOverlap,
            target_process_id: 42,
            target_window_handle: 77,
            target_bounds: Some([0, 0, 100, 100]),
            blocker_process_id: Some(91),
            blocker_window_handle: Some(92),
            blocker_bounds: Some([1, 1, 1, 1]),
            cloaked: Some(0),
            os_error: None,
            root_bounds_failure: None,
        }),
        suggested_delivery_mode: Some("PRIVATE_SECRET_DETAIL".into()),
        ..Default::default()
    });
    let value = fatal_error_value(&error);
    let capture = &value["error"]["details"]["capture"];
    assert_eq!(
        value["error"]["message"],
        crate::failure_output::CAPTURE_PROOF_MESSAGE
    );
    assert_eq!(capture["stage"], "visible_desktop_proof");
    assert_eq!(capture["reason"], "root_overlap");
    assert_eq!(capture["target_process_id"], 42);
    assert_eq!(capture["target_window_handle"], 77);
    assert_eq!(capture["blocker_window_handle"], 92);
    assert_eq!(capture["blocker_bounds"], json!([1, 1, 1, 1]));
    assert_eq!(value["error"]["details"]["whole_desktop_fallback"], false);
    assert!(!value.to_string().contains("PRIVATE_"));
    assert!(capture.get("title").is_none());
    assert!(capture.get("os_error").is_none());
}

#[rstest]
fn broken_stdout_is_reported_without_panicking_or_retrying_the_envelope() {
    struct BrokenStdout;

    impl std::io::Write for BrokenStdout {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "closed pipe",
            ))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let error = write_error_line(&mut BrokenStdout, &internal_failure_line())
        .expect_err("closed stdout should be surfaced");
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
}

#[rstest]
fn remote_host_failure_uses_a_local_allowlisted_identity() {
    let error = HostClientError::Remote {
        code: "REVIEW_PRIVATE_CODE_aa4904".into(),
        message: "REVIEW_PRIVATE_MESSAGE_950b33".into(),
        response: json!({"type": "error", "private": "REVIEW_PRIVATE_RESPONSE_b327ca"}),
    };
    let value = fatal_error_value(&error);

    assert_eq!(value["error"]["code"], "host_remote_failed");
    assert_eq!(value["error"]["message"], PUBLIC_FAILURE_MESSAGE);
    assert!(value["error"].get("response").is_none());
    assert!(!value.to_string().contains("REVIEW_PRIVATE_"));
}

#[rstest]
#[case("completion_unknown", "Do not blindly retry input")]
#[case("stale_observation", "same live task")]
#[case("session_not_found", "Host connection")]
#[case("invalid_request", "start_session is not a Host IPC method")]
#[case("task_authorization_required", "host-ensure only starts")]
#[case("task_authorization_denied", "outside its task authorization")]
#[case("task_authorization_expired", "expired")]
#[case("task_authorization_revoked", "revoked")]
fn remote_lifetime_failures_publish_only_fixed_local_guidance(
    #[case] code: &str,
    #[case] guidance: &str,
) {
    let error = HostClientError::Remote {
        code: code.into(),
        message: "PRIVATE_REMOTE_MESSAGE".into(),
        response: json!({"code":code,"details":{"token":"PRIVATE_REMOTE_TOKEN"}}),
    };
    let value = fatal_error_value(&error);
    assert_eq!(value["success"], false);
    assert_eq!(value["error"]["code"], code);
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains(guidance)
    );
    assert!(!value.to_string().contains("PRIVATE_"));
    assert!(value["error"].get("response").is_none());
    assert!(value["error"].get("details").is_none());
}

#[rstest]
#[case(dcc_cua_core::ComputerUseInputState::NotSent, "not_sent")]
#[case(dcc_cua_core::ComputerUseInputState::Sent, "sent")]
#[case(dcc_cua_core::ComputerUseInputState::Unknown, "unknown")]
fn completion_unknown_preserves_only_closed_typed_facts(
    #[case] input: dcc_cua_core::ComputerUseInputState,
    #[case] serialized: &str,
) {
    use dcc_cua_core::{
        ComputerUseCompletionState, ComputerUseErrorDetails, ComputerUseErrorPhase,
    };
    let error = ComputerUseError::new(
        ComputerUseErrorCode::CompletionUnknown,
        "PRIVATE_COMPLETION_MESSAGE\nPRIVATE_SECOND_LINE",
    )
    .with_details(ComputerUseErrorDetails {
        phase: Some(ComputerUseErrorPhase::ActionDispatch),
        action_attempted: Some(true),
        focus_mutation_attempted: Some(false),
        input_sent: Some(input),
        completion: Some(ComputerUseCompletionState::Unknown),
        effect_unknown: Some(true),
        suggested_delivery_mode: Some("PRIVATE_COMPLETION_DETAIL".into()),
        session_remains_active: Some(true),
        ..Default::default()
    });
    let failure = run_command_boundary(|| Err(error.into())).unwrap_err();
    let CommandFailure::Command(line) = failure else {
        panic!("expected command failure")
    };
    let mut output = Vec::new();
    write_error_line(&mut output, &line).unwrap();
    assert_eq!(output.iter().filter(|byte| **byte == b'\n').count(), 1);
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_completion_unknown_guidance(&value);
    assert_eq!(
        value["error"]["details"],
        json!({
            "phase":"action_dispatch", "action_attempted":true,
            "focus_mutation_attempted":false, "input_sent":serialized,
            "completion":"unknown", "effect_unknown":true
        })
    );
    assert!(!line.contains("PRIVATE_"));
}

fn assert_completion_unknown_guidance(value: &serde_json::Value) {
    assert_eq!(value["success"], false);
    assert_eq!(value["error"]["code"], "completion_unknown");
    let guidance = value["error"]["message"].as_str().unwrap();
    for required in [
        "Do not blindly retry input",
        "fresh observation",
        "same exact",
        "actual result",
        "authorized",
        "stale observations",
        "timeout alone",
    ] {
        assert!(
            guidance.contains(required),
            "missing recovery guidance: {required}"
        );
    }
}

#[rstest]
fn completion_unknown_without_facts_matches_remote_guidance_without_invented_state() {
    let typed = ComputerUseError::new(ComputerUseErrorCode::CompletionUnknown, "PRIVATE_TYPED");
    let remote = HostClientError::Remote {
        code: "completion_unknown".into(),
        message: "PRIVATE_REMOTE".into(),
        response: json!({"success":true, "message":"PRIVATE_RESPONSE", "details":{
            "input_sent":"not_sent", "completion":"known", "blind_retry":true,
            "session_remains_active":true, "private":"PRIVATE_DETAIL"
        }}),
    };
    let value = fatal_error_value(&typed);
    assert_completion_unknown_guidance(&value);
    assert_eq!(value, fatal_error_value(&remote));
    assert_eq!(value.as_object().unwrap().len(), 2);
    assert_eq!(value["error"].as_object().unwrap().len(), 2);
    assert!(value["error"].get("details").is_none());
    assert!(!value.to_string().contains("PRIVATE_"));
}

#[rstest]
fn completion_unknown_ignores_unrelated_typed_details_without_filling_missing_facts() {
    let error = ComputerUseError::new(ComputerUseErrorCode::CompletionUnknown, "PRIVATE_TYPED")
        .with_details(dcc_cua_core::ComputerUseErrorDetails {
            suggested_delivery_mode: Some("PRIVATE_DETAIL".into()),
            timed_out: Some(true),
            session_remains_active: Some(false),
            ..Default::default()
        });
    let value = fatal_error_value(&error);
    assert_completion_unknown_guidance(&value);
    assert!(value["error"].get("details").is_none());
    assert!(!value.to_string().contains("PRIVATE_"));
}

#[rstest]
#[case("CompletionUnknown")]
#[case("completion_unknown.extra")]
#[case("completion_unknown\nPRIVATE_CODE")]
fn near_match_completion_codes_remain_generic_remote_failures(#[case] code: &str) {
    let error = HostClientError::Remote {
        code: code.into(),
        message: "PRIVATE_MESSAGE".into(),
        response: json!({"code":"completion_unknown","details":"PRIVATE_DETAIL"}),
    };
    let value = fatal_error_value(&error);
    assert_eq!(
        value,
        json!({"success":false,"error":{"code":"host_remote_failed","message":PUBLIC_FAILURE_MESSAGE}})
    );
}

#[rstest]
fn transport_timeout_and_known_input_failure_are_not_completion_unknown() {
    let remote = HostClientError::Timeout { timeout_ms: 1 };
    assert_eq!(fatal_error_value(&remote)["error"]["code"], "host_timeout");
    let typed = ComputerUseError::new(ComputerUseErrorCode::InputFailed, "PRIVATE_MESSAGE")
        .with_details(dcc_cua_core::ComputerUseErrorDetails {
            phase: Some(dcc_cua_core::ComputerUseErrorPhase::PreDispatch),
            input_sent: Some(dcc_cua_core::ComputerUseInputState::NotSent),
            timed_out: Some(true),
            ..Default::default()
        });
    assert_eq!(
        fatal_error_value(&typed),
        json!({"success":false,"error":{"code":"input_failed","message":PUBLIC_FAILURE_MESSAGE}})
    );
}

#[rstest]
#[case("mcp-server")]
#[case("host-jsonl")]
#[case("host")]
fn completion_guidance_does_not_change_persistent_protocol_output_boundaries(
    #[case] command: &str,
) {
    assert!(
        terminal_error_output(&[OsString::from(command)]) == TerminalErrorOutput::ProtocolNative
    );
}

#[rstest]
fn completion_unknown_guidance_precedes_unrelated_capture_proof() {
    use dcc_cua_core::{
        ComputerUseCaptureDiagnostic, ComputerUseCaptureReason, ComputerUseCaptureStage,
        ComputerUseCompletionState, ComputerUseErrorDetails, ComputerUseErrorPhase,
        ComputerUseInputState,
    };
    let error = ComputerUseError::new(ComputerUseErrorCode::CompletionUnknown, "PRIVATE_MESSAGE")
        .with_details(ComputerUseErrorDetails {
            phase: Some(ComputerUseErrorPhase::ActivationDispatch),
            action_attempted: Some(false),
            focus_mutation_attempted: Some(true),
            input_sent: Some(ComputerUseInputState::NotSent),
            completion: Some(ComputerUseCompletionState::Unknown),
            effect_unknown: Some(true),
            capture: Some(ComputerUseCaptureDiagnostic {
                stage: ComputerUseCaptureStage::VisibleDesktopProof,
                reason: ComputerUseCaptureReason::RootOverlap,
                target_process_id: 42,
                target_window_handle: 77,
                target_bounds: None,
                blocker_process_id: None,
                blocker_window_handle: None,
                blocker_bounds: None,
                cloaked: None,
                os_error: None,
                root_bounds_failure: None,
            }),
            suggested_delivery_mode: Some("PRIVATE_DETAIL".into()),
            ..Default::default()
        });
    let value = fatal_error_value(&error);
    assert_completion_unknown_guidance(&value);
    assert_eq!(
        value["error"]["details"],
        json!({
            "phase":"activation_dispatch", "action_attempted":false,
            "focus_mutation_attempted":true, "input_sent":"not_sent",
            "completion":"unknown", "effect_unknown":true
        })
    );
    assert!(!value.to_string().contains("PRIVATE_"));
}
