use std::mem::size_of;
use windows_sys::Win32::{
    Foundation::{GetLastError, HWND, LPARAM, SetLastError},
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::*,
};

use super::{NativeMenuProof, refused, validate_proof};
use crate::{UiaError, UiaTarget, exact_window_native_state};

fn hwnd(raw: u64) -> HWND {
    raw as usize as HWND
}

fn standard_popup(window: HWND) -> bool {
    let mut class = [0_u16; 32];
    let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
    length == 6 && class[..6] == [35, 51, 50, 55, 54, 56]
}

fn thread_info(thread: u32) -> Result<GUITHREADINFO, UiaError> {
    if thread == 0 {
        return Err(refused("zero GUI thread is not an exact target"));
    }
    let mut info = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetGUIThreadInfo(thread, &mut info) } == 0 {
        return Err(refused(
            "GetGUIThreadInfo failed for the exact target thread",
        ));
    }
    Ok(info)
}

struct Roots {
    pid: u32,
    thread: u32,
    visited: usize,
    overflow: bool,
    popups: Vec<u64>,
}

unsafe extern "system" fn visit(window: HWND, parameter: LPARAM) -> i32 {
    let roots = unsafe { &mut *(parameter as *mut Roots) };
    roots.visited += 1;
    if roots.visited > 4096 {
        roots.overflow = true;
        return 0;
    }
    let mut pid = 0;
    let thread = unsafe { GetWindowThreadProcessId(window, &mut pid) };
    if pid == roots.pid
        && thread == roots.thread
        && unsafe { IsWindowVisible(window) } != 0
        && standard_popup(window)
    {
        roots.popups.push(window as usize as u64);
    }
    1
}

fn popups(pid: u32, thread: u32) -> Result<Vec<u64>, UiaError> {
    let mut roots = Roots {
        pid,
        thread,
        visited: 0,
        overflow: false,
        popups: Vec::new(),
    };
    let completed = unsafe { EnumWindows(Some(visit), &raw mut roots as isize) } != 0;
    if !completed || roots.overflow {
        return Err(refused("native popup enumeration was incomplete"));
    }
    Ok(roots.popups)
}

pub(super) fn read(target: UiaTarget) -> Result<NativeMenuProof, UiaError> {
    let popup = exact_window_native_state(target.process_id, target.window_handle)?;
    let mut cloaked = 1_u32;
    unsafe {
        windows::Win32::Graphics::Dwm::DwmGetWindowAttribute(
            windows::Win32::Foundation::HWND(hwnd(target.window_handle)),
            windows::Win32::Graphics::Dwm::DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            size_of::<u32>() as u32,
        )
    }
    .map_err(|_| refused("native popup cloaking state is unavailable"))?;
    if cloaked != 0 {
        return Err(refused("native popup is cloaked"));
    }
    let info = thread_info(popup.instance.window_thread_id)?;
    if info.flags & (GUI_INMENUMODE | GUI_POPUPMENUMODE) != GUI_INMENUMODE | GUI_POPUPMENUMODE
        || info.hwndMenuOwner.is_null()
    {
        return Err(UiaError::InvalidTarget(format!(
            "native_menu_inactive: popup_visible={}, flags={}, menu_owner={}; no cancellation message was sent",
            popup.visible, info.flags, info.hwndMenuOwner as usize as u64,
        )));
    }
    let owner = exact_window_native_state(target.process_id, info.hwndMenuOwner as usize as u64)
        .map_err(|_| refused("native GUI thread has no verifiable menu owner"))?;
    if standard_popup(hwnd(owner.window_handle)) {
        return Err(refused(
            "derived menu owner cannot itself be a standard menu popup",
        ));
    }
    let proof = NativeMenuProof {
        popup,
        owner,
        flags: info.flags,
        active_window_handle: info.hwndActive as usize as u64,
        capture_window_handle: info.hwndCapture as usize as u64,
    };
    validate_proof(
        target,
        &proof,
        standard_popup(hwnd(target.window_handle)),
        &popups(target.process_id, popup.instance.window_thread_id)?,
        info.hwndMoveSize as usize as u64,
    )?;
    // Double read every native instance and thread relationship. HWND reuse or
    // a concurrent popup transition cannot mint a cancellation capability.
    let after_popup = exact_window_native_state(target.process_id, target.window_handle)?;
    let after_owner = exact_window_native_state(target.process_id, owner.window_handle)?;
    let after = thread_info(popup.instance.window_thread_id)?;
    if popup != after_popup
        || owner != after_owner
        || info.flags != after.flags
        || info.hwndMenuOwner != after.hwndMenuOwner
        || info.hwndActive != after.hwndActive
        || info.hwndCapture != after.hwndCapture
        || info.hwndMoveSize != after.hwndMoveSize
        || popups(target.process_id, popup.instance.window_thread_id)? != [popup.window_handle]
    {
        return Err(refused(
            "native menu identity or relationship changed while reading",
        ));
    }
    Ok(proof)
}

pub(super) fn is_owner_thread(proof: &NativeMenuProof) -> bool {
    (unsafe { GetCurrentThreadId() }) == proof.owner.instance.window_thread_id
}

pub(super) fn send_once(proof: &NativeMenuProof) -> Result<(), UiaError> {
    let mut result = 0;
    unsafe { SetLastError(0) };
    let delivered = unsafe {
        SendMessageTimeoutW(
            hwnd(proof.owner.window_handle),
            WM_CANCELMODE,
            0,
            0,
            SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT | SMTO_BLOCK,
            250,
            &mut result,
        )
    };
    let error = unsafe { GetLastError() };
    if delivered == 0 {
        return Err(UiaError::OperationFailed(format!(
            "single WM_CANCELMODE attempt failed or timed out; os_error={error}; dismissal unconfirmed"
        )));
    }
    // WM_CANCELMODE's normal result is zero. Neither this result nor successful
    // delivery proves that the application honored cancellation.
    Ok(())
}

pub(super) fn confirm(
    proof: &NativeMenuProof,
    gate: &mut dyn FnMut() -> Result<(), UiaError>,
) -> Result<(), UiaError> {
    for attempt in 0..21 {
        gate()?;
        let owner = exact_window_native_state(proof.owner.process_id, proof.owner.window_handle)?;
        if owner.instance != proof.owner.instance {
            return Err(refused("menu owner instance changed after the message"));
        }
        let popup_hidden_or_gone = if unsafe { IsWindow(hwnd(proof.popup.window_handle)) } == 0 {
            true
        } else {
            let popup =
                exact_window_native_state(proof.popup.process_id, proof.popup.window_handle)?;
            if popup.instance != proof.popup.instance {
                return Err(refused("target popup HWND was reused after the message"));
            }
            !popup.visible
        };
        let info = thread_info(owner.instance.window_thread_id)?;
        let inactive = info.flags & (GUI_INMENUMODE | GUI_POPUPMENUMODE | GUI_SYSTEMMENUMODE) == 0
            && info.hwndMenuOwner.is_null();
        let no_replacement = popups(owner.process_id, owner.instance.window_thread_id)?.is_empty();
        gate()?;
        // Owner survival is required after all post-dispatch measurements too.
        let final_owner = exact_window_native_state(owner.process_id, owner.window_handle)?;
        if final_owner.instance != proof.owner.instance {
            return Err(refused("menu owner did not survive dismissal readback"));
        }
        if popup_hidden_or_gone && inactive && no_replacement {
            return Ok(());
        }
        if attempt < 20 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    Err(UiaError::OperationFailed(
        "native menu readback deadline elapsed; popup disappearance and owner survival were not jointly confirmed".into(),
    ))
}
