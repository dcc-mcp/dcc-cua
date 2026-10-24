//! Borrowed current-executor metadata only; no input-desktop or cursor queries.
#[cfg(windows)]
pub(crate) fn thread_desktop_name() -> Result<Option<String>, String> {
    user_object_name(current_thread_desktop()?, "current thread desktop")
}

#[cfg(windows)]
fn current_thread_desktop() -> Result<windows_sys::Win32::Foundation::HANDLE, String> {
    use windows_sys::Win32::System::{
        StationsAndDesktops::GetThreadDesktop, Threading::GetCurrentThreadId,
    };

    // SAFETY: the current thread ID is valid for the lifetime of this call;
    // GetThreadDesktop returns a borrowed desktop handle that must not be closed.
    let desktop = unsafe { GetThreadDesktop(GetCurrentThreadId()) };
    if desktop.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(desktop)
}

#[cfg(windows)]
pub(crate) fn process_window_station_name() -> Result<Option<String>, String> {
    use windows_sys::Win32::System::StationsAndDesktops::GetProcessWindowStation;

    // SAFETY: this returns our process's borrowed window-station handle. It must
    // not be closed, and querying its name does not modify its access policy.
    let station = unsafe { GetProcessWindowStation() };
    if station.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    user_object_name(station, "process window station")
}

#[cfg(windows)]
pub(crate) fn thread_desktop_receives_input() -> Result<bool, String> {
    use std::mem;
    use windows_sys::Win32::System::StationsAndDesktops::{GetUserObjectInformationW, UOI_IO};

    let desktop = current_thread_desktop()?;
    let mut receives_input = 0_i32;
    let mut required_bytes = 0_u32;
    // SAFETY: UOI_IO writes one BOOL to this valid buffer. The borrowed handle
    // belongs to our current thread; no desktop is opened, switched, or changed.
    if unsafe {
        GetUserObjectInformationW(
            desktop,
            UOI_IO,
            (&raw mut receives_input).cast(),
            mem::size_of::<i32>() as u32,
            &mut required_bytes,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if required_bytes != mem::size_of::<i32>() as u32 {
        return Err("UOI_IO returned an invalid BOOL buffer size".into());
    }
    Ok(receives_input != 0)
}

#[cfg(windows)]
fn user_object_name(
    object: windows_sys::Win32::Foundation::HANDLE,
    description: &str,
) -> Result<Option<String>, String> {
    use std::{mem, ptr};
    use windows_sys::Win32::{
        Foundation::ERROR_INSUFFICIENT_BUFFER,
        System::StationsAndDesktops::{GetUserObjectInformationW, UOI_NAME},
    };

    let mut required_bytes = 0_u32;
    // SAFETY: the null buffer is an explicit size query for this borrowed handle.
    let queried = unsafe {
        GetUserObjectInformationW(object, UOI_NAME, ptr::null_mut(), 0, &mut required_bytes)
    };
    // Capture GetLastError immediately: a denied query must remain an error,
    // rather than being silently represented as an unknown name.
    if queried == 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
            return Err(error.to_string());
        }
    }
    if required_bytes < mem::size_of::<u16>() as u32 {
        return Ok(None);
    }

    let units = (required_bytes as usize).div_ceil(mem::size_of::<u16>());
    let mut buffer = vec![0_u16; units];
    // SAFETY: the buffer is writable for the reported size and the output length
    // pointer is valid for the duration of the call.
    let succeeded = unsafe {
        GetUserObjectInformationW(
            object,
            UOI_NAME,
            buffer.as_mut_ptr().cast(),
            required_bytes,
            &mut required_bytes,
        )
    } != 0;
    if !succeeded {
        return Err(std::io::Error::last_os_error().to_string());
    }

    let length = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    String::from_utf16(&buffer[..length])
        .map(Some)
        .map_err(|error| format!("{description} name is not valid UTF-16: {error}"))
}
