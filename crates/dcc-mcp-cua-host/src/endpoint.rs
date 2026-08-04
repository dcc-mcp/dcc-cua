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
    use tokio::net::windows::named_pipe::ServerOptions;

    let limiter = connection_limiter();
    loop {
        let permit = limiter
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| HostError::Protocol("Host connection limiter closed".into()))?;
        let server = ServerOptions::new().create(&endpoint)?;
        server.connect().await?;
        let next_driver = driver.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _ = process_connection(next_driver, server).await;
        });
    }
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
