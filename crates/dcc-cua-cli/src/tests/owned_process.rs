#![cfg(windows)]

use crate::owned_process::{CREATE_NO_WINDOW_FLAG, OwnedConsoleChildRole, command};
use rstest::rstest;
use std::io::Read;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
};

#[derive(Default)]
struct WindowCount {
    pid: u32,
    visible: usize,
}

unsafe extern "system" fn count_visible_child_windows(hwnd: HWND, state: LPARAM) -> i32 {
    let state = unsafe { &mut *(state as *mut WindowCount) };
    let mut owner_pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut owner_pid) };
    if owner_pid == state.pid && unsafe { IsWindowVisible(hwnd) } != 0 {
        state.visible += 1;
    }
    1
}

fn visible_window_count(pid: u32) -> usize {
    let mut state = WindowCount { pid, visible: 0 };
    unsafe {
        let _ = EnumWindows(
            Some(count_visible_child_windows),
            &mut state as *mut WindowCount as LPARAM,
        );
    }
    state.visible
}

#[rstest]
fn native_messaging_registry_child_is_hidden_and_preserves_output_and_wait() {
    assert_eq!(CREATE_NO_WINDOW_FLAG, 0x0800_0000);
    let role = OwnedConsoleChildRole::NativeMessagingRegistry;
    let mut child = command(role, "powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::Out.Write('registry-pipe-ok'); Start-Sleep -Milliseconds 500",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bounded registry helper");
    let pid = child.id();
    thread::sleep(Duration::from_millis(150));
    assert_eq!(visible_window_count(pid), 0, "role={role:?} pid={pid}");
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("stdout pipe")
        .read_to_string(&mut stdout)
        .expect("read stdout pipe");
    let status = child.wait().expect("wait for registry helper");
    assert!(status.success(), "role={role:?} pid={pid} status={status}");
    assert_eq!(stdout, "registry-pipe-ok");
}
