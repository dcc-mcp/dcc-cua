use rstest::rstest;
use serde_json::json;

use super::*;

#[rstest]
fn one_shot_failures_have_a_single_json_error_envelope() {
    let error = ComputerUseError::new(
        ComputerUseErrorCode::StaleObservation,
        "take a fresh snapshot",
    );
    let line = fatal_error_line(&error);
    let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON error line");

    assert!(!line.contains('\n'));
    assert_eq!(value["success"], false);
    assert_eq!(value["error"]["code"], "stale_observation");
    assert_eq!(value["error"]["message"], "take a fresh snapshot");
}

#[rstest]
fn panic_boundary_returns_a_fixed_safe_machine_envelope() {
    let line = run_command_boundary(|| panic!("private panic payload"))
        .expect_err("panic should fail closed");
    let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON error line");

    assert_eq!(value["success"], false);
    assert_eq!(value["error"]["code"], "internal_failure");
    assert!(!line.contains("private panic payload"));
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
fn remote_host_failure_preserves_its_machine_code_without_the_raw_response() {
    let error = HostClientError::Remote {
        code: "approval_required".into(),
        message: "confirmation required".into(),
        response: json!({"type": "error", "private": "not forwarded"}),
    };
    let value = fatal_error_value(&error);

    assert_eq!(value["error"]["code"], "approval_required");
    assert_eq!(value["error"]["message"], "confirmation required");
    assert!(value["error"].get("response").is_none());
}
