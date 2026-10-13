use std::io::{self, ErrorKind};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use dcc_cua_client::{HostClient, HostClientError};
use serde_json::{Value, json};
use tokio::time::Instant;

const HOST_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const HOST_START_TIMEOUT: Duration = Duration::from_secs(15);
const HOST_START_RETRY_MS: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SupervisorIoError {
    kind: ErrorKind,
    os_error: Option<i32>,
}

impl SupervisorIoError {
    pub(super) fn from_io(error: &io::Error) -> Self {
        Self {
            kind: error.kind(),
            os_error: error.raw_os_error(),
        }
    }

    fn details(self) -> Value {
        let kind = match self.kind {
            ErrorKind::NotFound => "not_found",
            ErrorKind::PermissionDenied => "permission_denied",
            ErrorKind::ConnectionRefused => "connection_refused",
            ErrorKind::ConnectionReset => "connection_reset",
            ErrorKind::ConnectionAborted => "connection_aborted",
            ErrorKind::NotConnected => "not_connected",
            ErrorKind::AddrInUse => "address_in_use",
            ErrorKind::AddrNotAvailable => "address_not_available",
            ErrorKind::BrokenPipe => "broken_pipe",
            ErrorKind::AlreadyExists => "already_exists",
            ErrorKind::WouldBlock => "would_block",
            ErrorKind::InvalidInput => "invalid_input",
            ErrorKind::InvalidData => "invalid_data",
            ErrorKind::TimedOut => "timed_out",
            ErrorKind::Interrupted => "interrupted",
            ErrorKind::UnexpectedEof => "unexpected_eof",
            ErrorKind::Unsupported => "unsupported",
            _ => "other",
        };
        json!({"io_error_kind": kind, "os_error": self.os_error})
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HostProbeFailure {
    Io(SupervisorIoError),
    Protocol,
    Timeout,
    Remote,
}

impl HostProbeFailure {
    pub(super) fn from_client_error(error: HostClientError) -> Self {
        match error {
            HostClientError::Io(error) => Self::Io(SupervisorIoError::from_io(&error)),
            HostClientError::Protocol(_) => Self::Protocol,
            HostClientError::Timeout { .. } => Self::Timeout,
            HostClientError::Remote { .. } => Self::Remote,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::Io(error) if error.kind == ErrorKind::PermissionDenied => {
                "host_permission_denied"
            }
            Self::Io(_) => "host_transport_failed",
            Self::Protocol => "host_protocol_failed",
            Self::Timeout => "host_timeout",
            Self::Remote => "host_remote_failed",
        }
    }

    fn details(self) -> Value {
        let mut details = match self {
            Self::Io(error) => error.details(),
            _ => json!({}),
        };
        details["code"] = Value::from(self.code());
        details
    }
}

pub(super) fn initial_probe_allows_spawn(failure: HostProbeFailure) -> bool {
    match failure {
        HostProbeFailure::Io(error) => matches!(
            error.kind,
            ErrorKind::NotFound | ErrorKind::ConnectionRefused | ErrorKind::TimedOut
        ),
        HostProbeFailure::Timeout => true,
        HostProbeFailure::Protocol | HostProbeFailure::Remote => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HostEnsureIoStage {
    PrepareSpawn,
    ResolveExecutable,
    Spawn,
    PollChild,
}

impl HostEnsureIoStage {
    fn name(self) -> &'static str {
        match self {
            Self::PrepareSpawn => "prepare_spawn",
            Self::ResolveExecutable => "resolve_executable",
            Self::Spawn => "spawn",
            Self::PollChild => "poll_child",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HostEnsureError {
    InitialProbe(HostProbeFailure),
    Io {
        stage: HostEnsureIoStage,
        error: SupervisorIoError,
    },
    VersionMissing,
    VersionMismatch,
    ChildExited {
        exit_code: Option<i32>,
        last_probe: HostProbeFailure,
    },
    ReadyTimeout {
        last_probe: HostProbeFailure,
    },
}

impl HostEnsureError {
    pub(super) fn io(stage: HostEnsureIoStage, error: io::Error) -> Self {
        Self::Io {
            stage,
            error: SupervisorIoError::from_io(&error),
        }
    }

    pub(super) fn code(&self) -> &'static str {
        match self {
            Self::InitialProbe(failure) => failure.code(),
            Self::Io { error, .. } if error.kind == ErrorKind::PermissionDenied => {
                "host_permission_denied"
            }
            Self::Io {
                stage: HostEnsureIoStage::PrepareSpawn,
                ..
            } => "host_prepare_spawn_failed",
            Self::Io {
                stage: HostEnsureIoStage::ResolveExecutable,
                ..
            } => "host_executable_unavailable",
            Self::Io {
                stage: HostEnsureIoStage::Spawn,
                ..
            } => "host_spawn_failed",
            Self::Io {
                stage: HostEnsureIoStage::PollChild,
                ..
            } => "host_child_status_failed",
            Self::VersionMissing => "host_protocol_failed",
            Self::VersionMismatch => "host_version_mismatch",
            Self::ChildExited { .. } => "host_start_failed",
            Self::ReadyTimeout { .. } => "host_start_timeout",
        }
    }

    pub(super) fn details(&self) -> Value {
        match *self {
            Self::InitialProbe(failure) => {
                let mut details = match failure {
                    HostProbeFailure::Io(error) => error.details(),
                    _ => json!({}),
                };
                details["stage"] = Value::from("initial_probe");
                details
            }
            Self::Io { stage, error } => {
                let mut details = error.details();
                details["stage"] = Value::from(stage.name());
                details
            }
            Self::VersionMissing | Self::VersionMismatch => json!({"stage": "version_check"}),
            Self::ChildExited { last_probe, .. } => json!({
                "stage": "wait_ready", "last_probe": last_probe.details(),
            }),
            Self::ReadyTimeout { last_probe } => {
                json!({"stage": "wait_ready", "last_probe": last_probe.details()})
            }
        }
    }
}

impl std::fmt::Display for HostEnsureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for HostEnsureError {}

pub(super) fn validate_supervised_host_version(ping: &Value) -> Result<(), HostEnsureError> {
    validate_host_version(ping).map_err(|_| {
        if ping["host_version"].as_str().is_some() {
            HostEnsureError::VersionMismatch
        } else {
            HostEnsureError::VersionMissing
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HostStartPollDecision {
    Ready,
    Retry,
    Exhausted,
}

pub(super) const fn host_start_poll_decision(
    endpoint_ready: bool,
    spawned_child_exited: bool,
    deadline_reached: bool,
) -> HostStartPollDecision {
    match (endpoint_ready, spawned_child_exited, deadline_reached) {
        (true, _, _) => HostStartPollDecision::Ready,
        (false, _, true) => HostStartPollDecision::Exhausted,
        (false, true, false) | (false, false, false) => HostStartPollDecision::Retry,
    }
}

pub(crate) async fn ensure(
    endpoint: String,
    host_args: &[String],
) -> Result<Value, HostEnsureError> {
    let mut last_probe = match ping(&endpoint).await {
        Ok(ping) => {
            validate_supervised_host_version(&ping)?;
            return Ok(ready_response("existing", &endpoint, None, ping));
        }
        Err(failure) if !initial_probe_allows_spawn(failure) => {
            // Peer rejection or unknown I/O is not evidence of an absent Host.
            // Stop before changing standard-handle inheritance or spawning.
            return Err(HostEnsureError::InitialProbe(failure));
        }
        Err(failure) => failure,
    };

    prepare_detached_spawn()
        .map_err(|error| HostEnsureError::io(HostEnsureIoStage::PrepareSpawn, error))?;
    let binary = std::env::current_exe()
        .map_err(|error| HostEnsureError::io(HostEnsureIoStage::ResolveExecutable, error))?;
    let mut child = host_command(&binary, host_args)
        .spawn()
        .map_err(|error| HostEnsureError::io(HostEnsureIoStage::Spawn, error))?;
    let child_pid = child.id();
    let deadline = Instant::now() + HOST_START_TIMEOUT;
    let mut spawned_exit = None;
    loop {
        let ping = match ping(&endpoint).await {
            Ok(ping) => Some(ping),
            Err(failure) => {
                last_probe = failure;
                None
            }
        };
        let child_status = child
            .try_wait()
            .map_err(|error| HostEnsureError::io(HostEnsureIoStage::PollChild, error))?;
        if spawned_exit.is_none() {
            spawned_exit = child_status.map(|status| status.code());
        }
        match host_start_poll_decision(
            ping.is_some(),
            child_status.is_some(),
            Instant::now() >= deadline,
        ) {
            HostStartPollDecision::Ready => {
                let ping = ping.expect("ready decision requires a successful Host probe");
                if let Err(error) = validate_supervised_host_version(&ping) {
                    stop_failed_child(&mut child);
                    return Err(error);
                }
                let running = child_status.is_none();
                return Ok(ready_response(
                    if running { "started" } else { "existing" },
                    &endpoint,
                    running.then_some(child_pid),
                    ping,
                ));
            }
            HostStartPollDecision::Retry => {
                tokio::time::sleep(Duration::from_millis(HOST_START_RETRY_MS)).await;
            }
            HostStartPollDecision::Exhausted => break,
        }
    }

    stop_failed_child(&mut child);
    if let Some(exit_code) = spawned_exit {
        return Err(HostEnsureError::ChildExited {
            exit_code,
            last_probe,
        });
    }
    Err(HostEnsureError::ReadyTimeout { last_probe })
}

pub(super) fn validate_host_version(ping: &Value) -> Result<(), String> {
    let expected = env!("CARGO_PKG_VERSION");
    let actual = ping["host_version"]
        .as_str()
        .ok_or_else(|| "Host ping did not report host_version".to_owned())?;
    if actual != expected {
        return Err(format!(
            "Host version mismatch at the selected endpoint: running {actual}, CLI {expected}; stop the stale Host and run host-ensure again"
        ));
    }
    Ok(())
}

fn host_command(binary: &std::path::Path, host_args: &[String]) -> Command {
    let mut command = Command::new(binary);
    command
        .arg("host")
        .args(host_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
}

#[cfg(windows)]
fn prepare_detached_spawn() -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::{
        HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    for kind in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let handle = unsafe { GetStdHandle(kind) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            continue;
        }
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn prepare_detached_spawn() -> std::io::Result<()> {
    Ok(())
}

async fn ping(endpoint: &str) -> Result<Value, HostProbeFailure> {
    tokio::time::timeout(HOST_PROBE_TIMEOUT, async {
        let mut client = HostClient::connect(endpoint.to_owned(), "dcc-cua-host-ensure").await?;
        Ok::<_, dcc_cua_client::HostClientError>(client.ping().await?.value)
    })
    .await
    .map_err(|_| HostProbeFailure::Timeout)?
    .map_err(HostProbeFailure::from_client_error)
}

fn ready_response(status: &str, endpoint: &str, pid: Option<u32>, ping: Value) -> Value {
    json!({
        "type": "host_ready",
        "status": status,
        "endpoint": endpoint,
        "pid": pid,
        "protocol_version": ping["protocol_version"],
        "host_version": ping["host_version"],
    })
}

fn stop_failed_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}
