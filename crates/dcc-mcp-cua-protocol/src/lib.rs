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
pub const MAX_PARALLEL_DISCOVERY_REQUESTS: usize = 32;

#[cfg(unix)]
const UNIX_SOCKET_NAME: &str = "dcc-mcp-cua-v1.sock";

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
            return format!(r"\\.\pipe\dcc-mcp-cua-v1-session-{session_id}");
        }
        r"\\.\pipe\dcc-mcp-cua-v1".to_owned()
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
        "dcc-mcp-cua-v1".to_owned()
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
        .unwrap_or_else(|| temp_dir.join(format!("dcc-mcp-cua-{user_id}")));
    runtime_dir.join(UNIX_SOCKET_NAME)
}

#[cfg(test)]
mod tests;
