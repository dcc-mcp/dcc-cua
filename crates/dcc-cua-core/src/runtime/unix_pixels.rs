//! A retained native binding owns its worker through cancellation and stop.
use super::*;
use std::sync::Mutex;

#[cfg(target_os = "linux")]
pub(super) use super::linux_pixels::LinuxPixels as NativePixels;
#[cfg(target_os = "macos")]
pub(super) use super::macos_pixels::MacPixels as NativePixels;

#[cfg(target_os = "linux")]
pub(super) const PLATFORM: &str = "linux_x11";
#[cfg(target_os = "linux")]
pub(super) const CAPTURE_BACKEND: &str = "x11-get-image-exact";
#[cfg(target_os = "macos")]
pub(super) const PLATFORM: &str = "macos_windowserver";
#[cfg(target_os = "macos")]
pub(super) const CAPTURE_BACKEND: &str = "trycua-macos-exact-window";

pub(super) type Handle = Arc<Mutex<NativePixels>>;

pub(super) async fn operate<T: Send + 'static>(
    handle: Handle,
    operation: impl FnOnce(&mut NativePixels) -> ComputerUseResult<T> + Send + 'static,
) -> ComputerUseResult<T> {
    let worker_failed = || {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "native window capture worker did not return a result",
        )
    };
    tokio::task::spawn_blocking(move || {
        let mut bound = handle.lock().map_err(|_| worker_failed())?;
        operation(&mut bound)
    })
    .await
    .map_err(|_| worker_failed())?
}
