#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
#[cfg(unix)]
use std::path::Path;
use std::sync::Arc;

use tokio::sync::Semaphore;

#[cfg(any(windows, unix))]
use super::process_connection;
use super::{ComputerUseDriver, HostError, MAX_HOST_CONNECTIONS};

pub(crate) fn connection_limiter() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(MAX_HOST_CONNECTIONS))
}

pub(crate) async fn serve(driver: ComputerUseDriver, endpoint: String) -> Result<(), HostError> {
    #[cfg(windows)]
    {
        serve_named_pipe(driver, endpoint).await
    }
    #[cfg(unix)]
    {
        serve_unix_socket(driver, endpoint).await
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = driver;
        let _ = endpoint;
        Err(HostError::Protocol(
            "local endpoint transport is unsupported on this platform".into(),
        ))
    }
}

#[cfg(windows)]
async fn serve_named_pipe(driver: ComputerUseDriver, endpoint: String) -> Result<(), HostError> {
    let _singleton = acquire_endpoint_singleton(&endpoint)?;
    let limiter = connection_limiter();
    loop {
        let permit = limiter
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| HostError::Protocol("Host connection limiter closed".into()))?;
        let server = create_secure_named_pipe(&endpoint)?;
        server.connect().await?;
        let next_driver = driver.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _ = process_connection(next_driver, server).await;
        });
    }
}

#[cfg(windows)]
fn acquire_endpoint_singleton(
    endpoint: &str,
) -> Result<std::os::windows::io::OwnedHandle, HostError> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let name = endpoint_singleton_name(endpoint);
    let wide = name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let raw = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
    if raw.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    if already_exists {
        return Err(HostError::Protocol(format!(
            "endpoint is already in use: {endpoint}"
        )));
    }
    Ok(handle)
}

pub(crate) fn endpoint_singleton_name(endpoint: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in endpoint.to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!(r"Local\dcc-mcp-cua-host-{hash:016x}")
}

#[cfg(windows)]
fn create_secure_named_pipe(
    endpoint: &str,
) -> Result<tokio::net::windows::named_pipe::NamedPipeServer, HostError> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let mut security = WindowsPipeSecurity::for_current_logon()?;
    // SAFETY: attributes points to a valid SECURITY_ATTRIBUTES and descriptor
    // for the duration of CreateNamedPipeW; Tokio does not retain either pointer.
    Ok(unsafe {
        ServerOptions::new()
            .create_with_security_attributes_raw(endpoint, security.as_raw_attributes())?
    })
}

#[cfg(windows)]
struct WindowsPipeSecurity {
    _descriptor: LocalAllocation,
    attributes: windows_sys::Win32::Security::SECURITY_ATTRIBUTES,
}

#[cfg(windows)]
impl WindowsPipeSecurity {
    fn for_current_logon() -> Result<Self, HostError> {
        use windows_sys::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };
        use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

        let sddl: Vec<u16> = windows_pipe_sddl(&current_logon_sid_string()?)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = std::ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let descriptor = LocalAllocation(descriptor);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        Ok(Self {
            _descriptor: descriptor,
            attributes,
        })
    }

    fn as_raw_attributes(&mut self) -> *mut std::ffi::c_void {
        (&mut self.attributes as *mut windows_sys::Win32::Security::SECURITY_ATTRIBUTES).cast()
    }
}

#[cfg(windows)]
struct LocalAllocation(*mut std::ffi::c_void);

#[cfg(windows)]
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            let _ = unsafe { windows_sys::Win32::Foundation::LocalFree(self.0) };
        }
    }
}

#[cfg(windows)]
pub(crate) fn windows_pipe_sddl(logon_sid: &str) -> String {
    format!("D:P(A;;GA;;;SY)(A;;GA;;;{logon_sid})")
}

#[cfg(windows)]
pub(crate) fn current_logon_sid_string() -> Result<String, HostError> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_GROUPS, TOKEN_QUERY, TokenLogonSid,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut raw_token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(raw_token) };
    let mut required_bytes = 0;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenLogonSid,
            std::ptr::null_mut(),
            0,
            &mut required_bytes,
        )
    };
    if required_bytes == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let words = (required_bytes as usize).div_ceil(std::mem::size_of::<usize>());
    let mut buffer = vec![0usize; words];
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenLogonSid,
            buffer.as_mut_ptr().cast(),
            required_bytes,
            &mut required_bytes,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let groups = unsafe { &*buffer.as_ptr().cast::<TOKEN_GROUPS>() };
    if groups.GroupCount != 1 || (required_bytes as usize) < std::mem::size_of::<TOKEN_GROUPS>() {
        return Err(HostError::Protocol(
            "Windows returned an invalid logon SID group".into(),
        ));
    }

    let mut sid_text = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(groups.Groups[0].Sid, &mut sid_text) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let allocation = LocalAllocation(sid_text.cast());
    let mut length = 0;
    while length < 256 && unsafe { *sid_text.add(length) } != 0 {
        length += 1;
    }
    if length == 256 {
        return Err(HostError::Protocol(
            "Windows returned an invalid logon SID string".into(),
        ));
    }
    let value = String::from_utf16(unsafe { std::slice::from_raw_parts(sid_text, length) })
        .map_err(|_| HostError::Protocol("Windows logon SID is not valid UTF-16".into()))?;
    drop(allocation);
    Ok(value)
}

#[cfg(unix)]
async fn serve_unix_socket(driver: ComputerUseDriver, endpoint: String) -> Result<(), HostError> {
    use tokio::net::{UnixListener, UnixStream};

    let path = Path::new(&endpoint);
    prepare_unix_endpoint_parent(path)?;
    if path.exists() {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_socket() {
            return Err(HostError::Protocol(format!(
                "endpoint exists and is not a socket: {endpoint}"
            )));
        }
        match UnixStream::connect(path).await {
            Ok(_) => {
                return Err(HostError::Protocol(format!(
                    "endpoint is already in use: {endpoint}"
                )));
            }
            Err(error) if stale_unix_socket_error(&error) => std::fs::remove_file(path)?,
            Err(error) => return Err(error.into()),
        }
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let limiter = connection_limiter();
    loop {
        let permit = limiter
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| HostError::Protocol("Host connection limiter closed".into()))?;
        let (stream, _) = listener.accept().await?;
        let next_driver = driver.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _ = process_connection(next_driver, stream).await;
        });
    }
}

#[cfg(unix)]
pub(crate) fn prepare_unix_endpoint_parent(path: &Path) -> Result<(), HostError> {
    if !path.is_absolute() {
        return Err(HostError::Protocol(
            "Unix endpoint path must be absolute".into(),
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        HostError::Protocol("Unix endpoint path must have a private parent directory".into())
    })?;
    if !parent.exists() {
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(parent) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    if !dcc_mcp_cua_protocol::is_private_runtime_directory(parent) {
        return Err(HostError::Protocol(format!(
            "Unix endpoint parent must be owned by the current user with mode 0700: {}",
            parent.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn stale_unix_socket_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
    )
}
