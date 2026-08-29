use rstest::rstest;
use serde_json::json;

use crate::failure_output::{PUBLIC_FAILURE_MESSAGE, fatal_error_value};

use super::*;

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
    assert_eq!(value["error"]["message"], PUBLIC_FAILURE_MESSAGE);
    assert!(value["error"].get("details").is_none());
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
