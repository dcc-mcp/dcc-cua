//! Owner-only reads; all entry points are confined to the opt-in CI test.
#[allow(unused_imports)]
use rstest::rstest;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{ERROR_SUCCESS, GetLastError, HWND, LPARAM, SetLastError};
use windows_sys::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::diagnostics::{TITLE_CAPACITY, TitleRead, inventory_report};
use super::ime::{Association, Owner};
use super::{Identity, Window};

fn inspect(pid: u32, hwnd: HWND, stage: &str) -> Result<Option<Window>, &'static str> {
    let mut actual_pid = 0;
    let thread = unsafe { GetWindowThreadProcessId(hwnd, &mut actual_pid) };
    if actual_pid != pid {
        return Ok(None);
    }
    if thread == 0 {
        return Err("invalid owner thread");
    }
    let mut class = [0_u16; 128];
    let length = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
    if length <= 0 || length as usize >= class.len() - 1 {
        return Err("class unavailable or truncated");
    }
    let class = String::from_utf16_lossy(&class[..length as usize]);
    let mut title = [0_u16; TITLE_CAPACITY];
    let mut length = 0;
    let delivered = unsafe {
        // SendMessageTimeout may fail without setting last-error. Clear stale
        // state, then capture failure immediately before any other Win32 call.
        SetLastError(ERROR_SUCCESS);
        SendMessageTimeoutW(
            hwnd,
            WM_GETTEXT,
            title.len(),
            title.as_mut_ptr() as isize,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            20,
            &mut length,
        )
    };
    // Raw last-error is only interpreted when delivery failed; it never gates
    // acceptance. Capture it before the final PID/thread Win32 observation.
    let win32_error = unsafe { GetLastError() };
    let mut final_pid = 0;
    let final_thread = unsafe { GetWindowThreadProcessId(hwnd, &mut final_pid) };
    let read = TitleRead {
        hwnd: hwnd as usize as u64,
        initial_pid: actual_pid,
        initial_thread: thread,
        class: &class,
        delivery: delivered,
        length,
        win32_error,
        final_pid,
        final_thread,
    };
    if read.failed(pid) {
        eprintln!(
            "owned-window-read {}",
            read.diagnostic(stage, pid).expect("owned read")
        );
        return Err("title unavailable/truncated or owner changed");
    }
    Ok(Some(Window {
        identity: Identity {
            hwnd: hwnd as usize as u64,
            pid,
            thread,
            class,
            title: String::from_utf16_lossy(&title[..length]),
        },
        visible: unsafe { IsWindowVisible(hwnd) } != 0,
    }))
}

pub(crate) fn read(pid: u32, stage: &str) -> Vec<Window> {
    read_until(pid, stage, Instant::now() + Duration::from_secs(2))
}

pub(super) fn read_until(pid: u32, stage: &str, deadline: Instant) -> Vec<Window> {
    struct Read {
        pid: u32,
        stage: String,
        deadline: Instant,
        windows: Vec<Window>,
        error: Option<&'static str>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, state: LPARAM) -> i32 {
        let state = unsafe { &mut *(state as *mut Read) };
        if Instant::now() >= state.deadline {
            state.error = Some("inventory deadline exceeded");
            return 0;
        }
        match inspect(state.pid, hwnd, &state.stage) {
            Ok(Some(window)) if state.windows.len() < 64 => state.windows.push(window),
            Ok(None) => {}
            Ok(Some(_)) => state.error = Some("inventory limit exceeded"),
            Err(error) => state.error = Some(error),
        }
        i32::from(state.error.is_none())
    }
    let mut state = Read {
        pid,
        stage: stage.to_owned(),
        deadline,
        windows: Vec::new(),
        error: None,
    };
    let complete = unsafe { EnumWindows(Some(visit), &raw mut state as isize) };
    state.windows.sort_by_key(|window| window.identity.hwnd);
    eprintln!(
        "{}",
        inventory_report(
            stage,
            pid,
            complete == 0 || state.error.is_some(),
            &state.windows
        )
    );
    assert!(
        complete != 0 && state.error.is_none() && Instant::now() < deadline,
        "bounded owner inventory failed at {stage}: {:?}",
        state.error
    );
    state.windows
}

/// Query only roots already accepted by the exact fixture/cursor/presenter
/// contract. Re-read both identities and the IMM association without retry.
pub(super) fn associations(
    owners: &[Owner],
    stage: &str,
    deadline: Instant,
) -> Result<Vec<Association>, &'static str> {
    let mut result = Vec::new();
    for owner in owners {
        if Instant::now() >= deadline {
            return Err("association deadline exceeded");
        }
        let id = &owner.window.identity;
        let hwnd = id.hwnd as usize as HWND;
        if inspect(id.pid, hwnd, stage)?.as_ref().map(|w| &w.identity) != Some(id) {
            return Err("exact owner changed before IMM query");
        }
        let ime = unsafe { ImmGetDefaultIMEWnd(hwnd) };
        if ime.is_null() {
            return Err("native IME association is NULL");
        }
        let companion = inspect(id.pid, ime, stage)?.ok_or("foreign IME companion")?;
        if unsafe { ImmGetDefaultIMEWnd(hwnd) } != ime
            || inspect(id.pid, hwnd, stage)?.as_ref().map(|w| &w.identity) != Some(id)
            || inspect(id.pid, ime, stage)?.as_ref() != Some(&companion)
            || Instant::now() >= deadline
        {
            return Err("IME association or full native identity changed");
        }
        result.push(Association {
            owner: id.clone(),
            companion: Some(companion),
        });
    }
    eprintln!("owned-IME associations stage={stage}: {result:?}");
    Ok(result)
}
