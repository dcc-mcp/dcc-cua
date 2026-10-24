//! The context-only native capability set intentionally excludes input-surface APIs.
use windows_sys::Win32::{
    Foundation::FILETIME,
    System::{
        RemoteDesktop::ProcessIdToSessionId,
        Threading::{GetCurrentProcess, GetCurrentThreadId, GetProcessTimes},
    },
};

use super::ContextProbe;
use crate::desktop_context;

pub(super) struct NativeContext;

impl ContextProbe for NativeContext {
    fn process_id(&self) -> u32 {
        std::process::id()
    }

    fn thread_id(&self) -> u32 {
        // SAFETY: this value-only query reads the calling thread's identifier.
        unsafe { GetCurrentThreadId() }
    }

    fn process_birth(&self) -> Result<String, String> {
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: the current-process pseudo-handle is borrowed and all four
        // output buffers are live. No foreign process is opened or inspected.
        if unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(
            ((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
                .to_string(),
        )
    }

    fn session_id(&self) -> Result<u32, String> {
        let mut session = 0;
        // SAFETY: only the current process's value-only session ID is requested.
        if unsafe { ProcessIdToSessionId(std::process::id(), &mut session) } == 0 {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok(session)
        }
    }

    fn window_station(&self) -> Result<Option<String>, String> {
        desktop_context::process_window_station_name()
    }

    fn thread_desktop(&self) -> Result<Option<String>, String> {
        desktop_context::thread_desktop_name()
    }

    fn receives_input(&self) -> Result<bool, String> {
        desktop_context::thread_desktop_receives_input()
    }
}
