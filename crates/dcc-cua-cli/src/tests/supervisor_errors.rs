use std::io::{self, ErrorKind};

use dcc_cua_client::HostClientError;
use rstest::rstest;
use serde_json::{Value, json};

use crate::failure_output::{PUBLIC_FAILURE_MESSAGE, fatal_error_line, fatal_error_value};
use crate::host_lifecycle::{
    HostEnsureError, HostEnsureIoStage, HostProbeFailure, HostStartPollDecision,
    host_start_poll_decision, initial_probe_allows_spawn, validate_supervised_host_version,
};
use crate::{CommandFailure, run_command_boundary};

const PRIVATE_SENTINEL: &str = "REVIEW_PRIVATE_ENDPOINT_PATH_VERSION_SECRET\nsecond line";

#[rstest]
#[case(ErrorKind::NotFound, "not_found", true)]
#[case(ErrorKind::ConnectionRefused, "connection_refused", true)]
#[case(ErrorKind::TimedOut, "timed_out", true)]
#[case(ErrorKind::PermissionDenied, "permission_denied", false)]
#[case(ErrorKind::ConnectionReset, "connection_reset", false)]
#[case(ErrorKind::ConnectionAborted, "connection_aborted", false)]
#[case(ErrorKind::NotConnected, "not_connected", false)]
#[case(ErrorKind::AddrInUse, "address_in_use", false)]
#[case(ErrorKind::AddrNotAvailable, "address_not_available", false)]
#[case(ErrorKind::BrokenPipe, "broken_pipe", false)]
#[case(ErrorKind::AlreadyExists, "already_exists", false)]
#[case(ErrorKind::WouldBlock, "would_block", false)]
#[case(ErrorKind::InvalidInput, "invalid_input", false)]
#[case(ErrorKind::InvalidData, "invalid_data", false)]
#[case(ErrorKind::Interrupted, "interrupted", false)]
#[case(ErrorKind::UnexpectedEof, "unexpected_eof", false)]
#[case(ErrorKind::Unsupported, "unsupported", false)]
#[case(ErrorKind::OutOfMemory, "other", false)]
#[case(ErrorKind::Other, "other", false)]
fn initial_io_probe_preserves_only_allowed_startup_conditions(
    #[case] kind: ErrorKind,
    #[case] public_kind: &str,
    #[case] allows_spawn: bool,
) {
    let failure = HostProbeFailure::from_client_error(HostClientError::Io(io::Error::new(
        kind,
        PRIVATE_SENTINEL,
    )));
    assert_eq!(initial_probe_allows_spawn(failure), allows_spawn);
    let error = HostEnsureError::InitialProbe(failure);
    let code = if kind == ErrorKind::PermissionDenied {
        "host_permission_denied"
    } else {
        "host_transport_failed"
    };
    assert_eq!(error.to_string(), code);
    assert_eq!(
        fatal_error_value(&error),
        json!({
            "success": false,
            "error": {
                "code": code,
                "message": PUBLIC_FAILURE_MESSAGE,
                "details": {
                    "stage": "initial_probe",
                    "io_error_kind": public_kind, "os_error": null,
                },
            },
        })
    );
    assert_safe_line(&error);
}

#[rstest]
#[case(HostClientError::Protocol(PRIVATE_SENTINEL.into()), "host_protocol_failed", false)]
#[case(HostClientError::Remote {
    code: PRIVATE_SENTINEL.into(),
    message: PRIVATE_SENTINEL.into(),
    response: json!({"endpoint": PRIVATE_SENTINEL}),
}, "host_remote_failed", false)]
#[case(HostClientError::Timeout { timeout_ms: u128::MAX }, "host_timeout", true)]
fn typed_probe_rejections_and_timeouts_discard_private_payloads(
    #[case] client_error: HostClientError,
    #[case] code: &str,
    #[case] allows_spawn: bool,
) {
    let failure = HostProbeFailure::from_client_error(client_error);
    assert_eq!(initial_probe_allows_spawn(failure), allows_spawn);
    let error = HostEnsureError::InitialProbe(failure);
    assert_eq!(error.to_string(), code);
    assert_eq!(
        fatal_error_value(&error)["error"]["details"],
        json!({
            "stage": "initial_probe",
        })
    );
    assert_safe_line(&error);
}

#[rstest]
#[case(false, false, false, HostStartPollDecision::Retry)]
#[case(false, true, false, HostStartPollDecision::Retry)]
#[case(false, false, true, HostStartPollDecision::Exhausted)]
#[case(false, true, true, HostStartPollDecision::Exhausted)]
#[case(true, false, false, HostStartPollDecision::Ready)]
#[case(true, true, false, HostStartPollDecision::Ready)]
#[case(true, false, true, HostStartPollDecision::Ready)]
#[case(true, true, true, HostStartPollDecision::Ready)]
fn startup_poll_preserves_ready_priority_and_competing_host_retry(
    #[case] ready: bool,
    #[case] child_exited: bool,
    #[case] deadline: bool,
    #[case] expected: HostStartPollDecision,
) {
    assert_eq!(
        host_start_poll_decision(ready, child_exited, deadline),
        expected
    );
}

#[rstest]
#[case(
    HostEnsureIoStage::PrepareSpawn,
    "prepare_spawn",
    "host_prepare_spawn_failed"
)]
#[case(
    HostEnsureIoStage::ResolveExecutable,
    "resolve_executable",
    "host_executable_unavailable"
)]
#[case(HostEnsureIoStage::Spawn, "spawn", "host_spawn_failed")]
#[case(HostEnsureIoStage::PollChild, "poll_child", "host_child_status_failed")]
fn supervisor_io_stages_preserve_only_stable_metadata(
    #[case] stage: HostEnsureIoStage,
    #[case] public_stage: &str,
    #[case] stage_code: &str,
) {
    for (kind, public_kind, code) in [
        (ErrorKind::InvalidData, "invalid_data", stage_code),
        (
            ErrorKind::PermissionDenied,
            "permission_denied",
            "host_permission_denied",
        ),
    ] {
        let error = HostEnsureError::io(stage, io::Error::new(kind, PRIVATE_SENTINEL));
        assert_eq!(error.to_string(), code);
        assert_eq!(
            fatal_error_value(&error)["error"],
            json!({
                "code": code,
                "message": PUBLIC_FAILURE_MESSAGE,
                "details": {"stage": public_stage, "io_error_kind": public_kind, "os_error": null},
            })
        );
        assert_safe_line(&error);
    }
}

#[rstest]
#[case(json!({}))]
#[case(json!({"host_version": null}))]
#[case(json!({"host_version": 19}))]
fn missing_or_invalid_version_is_typed_without_exposing_payloads(#[case] mut ping: Value) {
    ping["private"] = json!(PRIVATE_SENTINEL);
    let error = validate_supervised_host_version(&ping).unwrap_err();
    assert_eq!(error, HostEnsureError::VersionMissing);
    assert_eq!(
        fatal_error_value(&error)["error"],
        json!({
            "code": "host_protocol_failed", "message": PUBLIC_FAILURE_MESSAGE,
            "details": {"stage": "version_check"},
        })
    );
    assert_safe_line(&error);
}

#[rstest]
fn version_mismatch_is_typed_without_exposing_arbitrary_version_text() {
    let error =
        validate_supervised_host_version(&json!({"host_version": PRIVATE_SENTINEL})).unwrap_err();
    assert_eq!(error, HostEnsureError::VersionMismatch);
    assert_eq!(
        fatal_error_value(&error)["error"],
        json!({
            "code": "host_version_mismatch", "message": PUBLIC_FAILURE_MESSAGE,
            "details": {"stage": "version_check"},
        })
    );
    assert_safe_line(&error);
    assert!(
        validate_supervised_host_version(&json!({"host_version": env!("CARGO_PKG_VERSION")}))
            .is_ok()
    );
}

#[rstest]
fn exhausted_startup_publishes_only_last_probe_classification() {
    for exit_code in [Some(17), None] {
        let error = HostEnsureError::ChildExited {
            exit_code,
            last_probe: HostProbeFailure::Protocol,
        };
        assert_eq!(
            fatal_error_value(&error)["error"],
            json!({
                "code": "host_start_failed", "message": PUBLIC_FAILURE_MESSAGE,
                "details": {
                    "stage": "wait_ready",
                    "last_probe": {"code": "host_protocol_failed"},
                },
            })
        );
    }
    let last_probe = HostProbeFailure::from_client_error(HostClientError::Io(io::Error::new(
        ErrorKind::PermissionDenied,
        PRIVATE_SENTINEL,
    )));
    let error = HostEnsureError::ReadyTimeout { last_probe };
    assert_eq!(
        fatal_error_value(&error)["error"],
        json!({
            "code": "host_start_timeout", "message": PUBLIC_FAILURE_MESSAGE,
            "details": {
                "stage": "wait_ready",
                "last_probe": {"code": "host_permission_denied", "io_error_kind": "permission_denied", "os_error": null},
            },
        })
    );
    assert_safe_line(&error);
}

#[cfg(windows)]
#[rstest]
fn windows_access_denied_preserves_numeric_os_error_and_stops_initial_startup() {
    let failure =
        HostProbeFailure::from_client_error(HostClientError::Io(io::Error::from_raw_os_error(5)));
    assert!(!initial_probe_allows_spawn(failure));
    assert_eq!(
        fatal_error_value(&HostEnsureError::InitialProbe(failure))["error"]["details"],
        json!({
            "stage": "initial_probe", "io_error_kind": "permission_denied", "os_error": 5,
        })
    );
    for stage in [
        HostEnsureIoStage::PrepareSpawn,
        HostEnsureIoStage::ResolveExecutable,
        HostEnsureIoStage::Spawn,
        HostEnsureIoStage::PollChild,
    ] {
        let error = HostEnsureError::io(stage, io::Error::from_raw_os_error(5));
        let value = fatal_error_value(&error);
        assert_eq!(value["error"]["code"], "host_permission_denied");
        assert_eq!(value["error"]["details"]["os_error"], 5);
    }
}

#[rstest]
fn boxed_supervisor_error_retains_identity_through_the_command_boundary() {
    let failure = run_command_boundary(|| {
        Err(Box::new(HostEnsureError::InitialProbe(
            HostProbeFailure::Protocol,
        )))
    })
    .unwrap_err();
    let CommandFailure::Command(line) = failure else {
        panic!("expected typed command failure")
    };
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap()["error"]["code"],
        "host_protocol_failed"
    );
    assert!(!line.contains(PRIVATE_SENTINEL));
}

#[rstest]
fn unrelated_errors_keep_the_original_generic_envelope() {
    assert_eq!(
        fatal_error_value(&io::Error::other(PRIVATE_SENTINEL)),
        json!({
            "success": false,
            "error": {"code": "command_failed", "message": PUBLIC_FAILURE_MESSAGE},
        })
    );
}

fn assert_safe_line(error: &HostEnsureError) {
    let line = fatal_error_line(error);
    assert!(!line.contains("REVIEW_PRIVATE_"));
    assert!(!line.contains('\n'));
    assert!(!error.to_string().contains("REVIEW_PRIVATE_"));
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap()["error"]["message"],
        PUBLIC_FAILURE_MESSAGE
    );
}
