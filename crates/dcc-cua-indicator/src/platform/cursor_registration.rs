//! Process-local authority for the singleton renderer configured by DCC-CUA.
//! A class name alone never grants authority over a window in another process.
use std::sync::OnceLock;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetWindowThreadProcessId, SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW,
    WM_GETTEXT,
};

static RENDERER_ID: OnceLock<String> = OnceLock::new();

pub(crate) fn register(id: String) -> String {
    RENDERER_ID.get_or_init(|| id).clone()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OwnedCursorWindow {
    pub(super) raw: usize,
    thread_id: u32,
    process_id: u32,
    title: String,
}

pub(super) fn registered_window(window: HWND) -> Option<OwnedCursorWindow> {
    let renderer_id = RENDERER_ID.get()?;
    let mut process_id = 0;
    let thread_id = unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
    // This authority lives only in the current process. A peer must suppress
    // its own cursor via its own presenter, never by remote class-name lookup.
    if process_id != std::process::id() || thread_id == 0 {
        return None;
    }
    let mut class = [0_u16; 128];
    let length = unsafe { GetClassNameW(window, &mut class) };
    if length <= 0
        || String::from_utf16_lossy(&class[..length as usize]) != "Cua.AgentCursorOverlay"
    {
        return None;
    }
    let mut title = [0_u16; 256];
    let mut length = 0_usize;
    // Same-process GetWindowText sends an unbounded WM_GETTEXT. Keep even an
    // unresponsive owned renderer behind a bounded, fail-closed identity read.
    let read = unsafe {
        SendMessageTimeoutW(
            window,
            WM_GETTEXT,
            WPARAM(title.len()),
            LPARAM(title.as_mut_ptr() as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            20,
            Some(&mut length),
        )
    };
    if read.0 == 0 || length == 0 || length >= title.len() {
        return None;
    }
    let title = String::from_utf16_lossy(&title[..length]);
    if title != format!("Cua.AgentCursorOverlay.{renderer_id}") {
        return None;
    }
    Some(OwnedCursorWindow {
        raw: window.0 as usize,
        thread_id,
        process_id,
        title,
    })
}

impl OwnedCursorWindow {
    pub(super) fn still_registered(&self) -> bool {
        registered_window(HWND(self.raw as *mut _)).as_ref() == Some(self)
    }
}
