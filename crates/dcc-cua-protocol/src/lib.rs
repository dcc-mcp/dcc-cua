//! Shared Host wire limits and platform-default local endpoint identity.

#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
#[cfg(unix)]
use std::path::{Path, PathBuf};

pub const HOST_PROTOCOL_VERSION: u32 = 1;
pub const MAX_JSON_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_BINARY_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_REQUEST_ID_CHARS: usize = 128;
pub const MAX_HOST_CONNECTIONS: usize = 32;
pub const MAX_SESSIONS_PER_CONNECTION: usize = 16;
pub const MAX_PARALLEL_DISCOVERY_REQUESTS: usize = 32;
pub const DEFAULT_SESSION_IDLE_TIMEOUT_MS: u64 = 15 * 60 * 1_000;
pub const MIN_SESSION_IDLE_TIMEOUT_MS: u64 = 1_000;
pub const MAX_SESSION_IDLE_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq)]
pub struct RequestEnvelope {
    pub request_id: Option<String>,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RequestEnvelopeError {
    #[error("request envelope must be an object")]
    NotObject,
    #[error("request_id must be a string")]
    RequestIdNotString,
    #[error("request_id must contain 1..{MAX_REQUEST_ID_CHARS} characters")]
    InvalidRequestId,
    #[error("request envelope requires a non-empty method")]
    MissingMethod,
    #[error("request params must be an object")]
    ParamsNotObject,
}

impl RequestEnvelope {
    pub fn from_value(value: &serde_json::Value) -> Result<Self, RequestEnvelopeError> {
        let object = value.as_object().ok_or(RequestEnvelopeError::NotObject)?;
        let request_id = object
            .get("request_id")
            .map(|value| {
                value
                    .as_str()
                    .ok_or(RequestEnvelopeError::RequestIdNotString)
                    .and_then(|request_id| {
                        validate_request_id(request_id)?;
                        Ok(request_id.to_owned())
                    })
            })
            .transpose()?;
        let method = object
            .get("method")
            .and_then(serde_json::Value::as_str)
            .filter(|method| !method.is_empty())
            .ok_or(RequestEnvelopeError::MissingMethod)?
            .to_owned();
        let params = object
            .get("params")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if !params.is_object() {
            return Err(RequestEnvelopeError::ParamsNotObject);
        }
        Ok(Self {
            request_id,
            method,
            params,
        })
    }
}

pub fn validate_request_id(request_id: &str) -> Result<(), RequestEnvelopeError> {
    if request_id.is_empty() || request_id.chars().count() > MAX_REQUEST_ID_CHARS {
        return Err(RequestEnvelopeError::InvalidRequestId);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostMethodTraits {
    pub action: bool,
    pub standalone_snapshot: bool,
    pub semantic_observation: bool,
    pub pipeline_safe: bool,
    pub parallel_discovery: bool,
}

#[must_use]
pub fn host_method_traits(method: &str) -> HostMethodTraits {
    let action = matches!(
        method,
        "execute_action"
            | "execute_desktop_action"
            | "browser_click"
            | "browser_type"
            | "browser_pointer"
            | "browser_navigate"
            | "browser_set_input_files"
            | "browser_dialog"
    );
    let standalone_snapshot = matches!(
        method,
        "snapshot" | "desktop_snapshot" | "desktop_session_snapshot" | "browser_snapshot"
    );
    let semantic_observation = matches!(
        method,
        "accessibility_snapshot"
            | "find"
            | "get_session_state"
            | "get_input_state"
            | "session_health"
            | "poll_session_events"
            | "get_window_state"
            | "verify_state"
            | "wait_for"
    );
    let parallel_discovery = matches!(
        method,
        "ping" | "list_apps" | "list_tools" | "list_windows" | "screen_size" | "cursor_position"
    );
    let pipeline_safe = parallel_discovery
        || matches!(
            method,
            "desktop_snapshot"
                | "get_window_state"
                | "snapshot"
                | "accessibility_snapshot"
                | "verify_state"
                | "get_session_state"
                | "get_input_state"
                | "session_health"
                | "find"
                | "browser_snapshot"
                | "recording_state"
                | "live_observation_state"
                | "clipboard_read"
                | "desktop_session_snapshot"
        );
    HostMethodTraits {
        action,
        standalone_snapshot,
        semantic_observation,
        pipeline_safe,
        parallel_discovery,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame transport failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame protocol failed: {0}")]
    Protocol(String),
}

pub async fn read_frame<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> Result<Option<Vec<u8>>, FrameError> {
    use tokio::io::AsyncReadExt;

    let mut prefix = [0_u8; 4];
    match reader.read_exact(&mut prefix).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > max {
        return Err(FrameError::Protocol(format!(
            "frame length {length} exceeds the configured limit"
        )));
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).await?;
    Ok(Some(body))
}

pub async fn write_frame<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    body: &[u8],
    max: usize,
) -> Result<(), FrameError> {
    use tokio::io::AsyncWriteExt;

    write_frame_unflushed(writer, body, max).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn write_frame_unflushed<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    body: &[u8],
    max: usize,
) -> Result<(), FrameError> {
    use tokio::io::AsyncWriteExt;

    if body.is_empty() || body.len() > max || body.len() > u32::MAX as usize {
        return Err(FrameError::Protocol(
            "frame payload is outside the configured limit".into(),
        ));
    }
    writer.write_all(&(body.len() as u32).to_be_bytes()).await?;
    writer.write_all(body).await?;
    Ok(())
}

#[cfg(unix)]
const UNIX_SOCKET_NAME: &str = "dcc-cua-v1.sock";

/// Return the stable local endpoint shared by the Host and reusable client.
#[must_use]
pub fn default_endpoint() -> String {
    #[cfg(windows)]
    {
        let mut session_id = 0;
        let resolved = unsafe {
            windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(
                windows_sys::Win32::System::Threading::GetCurrentProcessId(),
                &mut session_id,
            ) != 0
        };
        if resolved {
            return format!(r"\\.\pipe\dcc-cua-v1-session-{session_id}");
        }
        r"\\.\pipe\dcc-cua-v1".to_owned()
    }
    #[cfg(unix)]
    {
        default_unix_endpoint_from(
            std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
            &std::env::temp_dir(),
            effective_user_id(),
        )
        .to_string_lossy()
        .into_owned()
    }
    #[cfg(not(any(windows, unix)))]
    {
        "dcc-cua-v1".to_owned()
    }
}

/// Return the effective Unix user that owns the local control endpoint.
#[cfg(unix)]
#[must_use]
pub fn effective_user_id() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

/// Verify the XDG ownership and mode contract used for control sockets.
#[cfg(unix)]
#[must_use]
pub fn is_private_runtime_directory(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    metadata.file_type().is_dir()
        && metadata.uid() == effective_user_id()
        && metadata.permissions().mode() & 0o777 == 0o700
}

#[cfg(unix)]
fn default_unix_endpoint_from(
    xdg_runtime_dir: Option<&OsStr>,
    temp_dir: &Path,
    user_id: u32,
) -> PathBuf {
    let runtime_dir = xdg_runtime_dir
        .map(Path::new)
        .filter(|path| path.is_absolute() && is_private_runtime_directory(path))
        .map(Path::to_path_buf)
        .unwrap_or_else(|| temp_dir.join(format!("dcc-cua-{user_id}")));
    runtime_dir.join(UNIX_SOCKET_NAME)
}

#[cfg(test)]
mod tests;
