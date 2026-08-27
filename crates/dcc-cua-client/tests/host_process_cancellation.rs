use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use dcc_cua_client::{HostProcess, SnapshotTransport};
use rstest::rstest;

fn compile_fixture(directory: &Path) -> PathBuf {
    let binary = directory.join(if cfg!(windows) {
        "cancellable-host.exe"
    } else {
        "cancellable-host"
    });
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("scripts/fixtures/cancellable_host.rs");
    let output = Command::new("rustc")
        .args(["--edition=2024", "-o"])
        .arg(&binary)
        .arg(source)
        .output()
        .expect("compile cancellable Host fixture");
    assert!(
        output.status.success(),
        "fixture compilation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary
}

async fn read_pid(path: &Path) -> u32 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(path)
                && let Ok(pid) = text.parse()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("fixture should publish its pid")
}

async fn assert_process_reaped(pid: u32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while process_is_running(pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        force_stop(pid);
        panic!("cancelled Host child {pid} remained alive")
    });
}

#[cfg(windows)]
fn process_is_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let running = unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT;
    unsafe { CloseHandle(handle) };
    running
}

#[cfg(windows)]
fn force_stop(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if !handle.is_null() {
        unsafe {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    unsafe { kill(pid as i32, 0) == 0 }
}

#[cfg(unix)]
fn force_stop(pid: u32) {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    unsafe {
        kill(pid as i32, 9);
    }
}

#[rstest]
#[tokio::test]
async fn cancelling_startup_reaps_the_spawned_host() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let binary = compile_fixture(directory.path());
    let pid_file = directory.path().join("startup.pid");
    let pid_argument = pid_file.to_string_lossy().into_owned();
    let host_args = ["--pid-file", &pid_argument, "--fixture-mode", "startup"];
    let pid = {
        let startup = HostProcess::spawn_with_host_args(
            &binary,
            "cancel-startup-test",
            SnapshotTransport::BinaryFrame,
            &host_args,
        );
        tokio::pin!(startup);
        tokio::select! {
            result = &mut startup => panic!("startup unexpectedly completed: {result:?}"),
            pid = read_pid(&pid_file) => pid,
        }
    };

    assert_process_reaped(pid).await;
}

#[rstest]
#[tokio::test]
async fn cancelling_shutdown_reaps_the_negotiated_host() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let binary = compile_fixture(directory.path());
    let pid_file = directory.path().join("shutdown.pid");
    let pid_argument = pid_file.to_string_lossy().into_owned();
    let host = HostProcess::spawn_with_host_args(
        &binary,
        "cancel-shutdown-test",
        SnapshotTransport::BinaryFrame,
        &["--pid-file", &pid_argument, "--fixture-mode", "shutdown"],
    )
    .await
    .expect("fixture should complete hello");
    let pid = host.id().expect("fixture pid");
    {
        let shutdown = host.shutdown();
        tokio::pin!(shutdown);
        tokio::select! {
            result = &mut shutdown => panic!("shutdown unexpectedly completed: {result:?}"),
            () = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
    }

    assert_process_reaped(pid).await;
}
