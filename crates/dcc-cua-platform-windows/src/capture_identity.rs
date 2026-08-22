use std::os::windows::ffi::OsStringExt;

use thiserror::Error;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM},
    System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    },
    UI::WindowsAndMessaging::{
        EnumWindows, GA_ROOT, GetAncestor, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExactWindowCaptureRoute {
    Wgc,
    VerifiedVisible,
}

pub(crate) const fn route_for_same_executable_root_count(
    same_executable_root_count: usize,
) -> ExactWindowCaptureRoute {
    if same_executable_root_count > 1 {
        ExactWindowCaptureRoute::VerifiedVisible
    } else {
        ExactWindowCaptureRoute::Wgc
    }
}

#[derive(Debug, Error)]
#[error("exact-window capture identity proof failed: {0}")]
pub struct ExactWindowCaptureIdentityError(String);

struct RootEnumeration {
    target_process_id: u32,
    target_image: String,
    same_executable_root_count: usize,
}

/// Decide whether WGC pixels can be attributed solely from one exact HWND.
///
/// WGC frames do not expose a source HWND. When another visible top-level
/// window belongs to the same executable, the request identity cannot prove
/// the returned frame identity, so callers must use independently verified
/// visible pixels or fail closed.
pub fn exact_window_capture_route(
    process_id: u32,
    window_handle: u64,
) -> Result<ExactWindowCaptureRoute, ExactWindowCaptureIdentityError> {
    validate_exact_window_owner(process_id, window_handle)?;

    let target_image = process_image_identity(process_id)?;
    let mut enumeration = RootEnumeration {
        target_process_id: process_id,
        target_image,
        same_executable_root_count: 0,
    };
    let enumerated = unsafe {
        EnumWindows(
            Some(count_same_executable_root),
            &mut enumeration as *mut RootEnumeration as LPARAM,
        )
    };
    if enumerated == 0 {
        return Err(identity_error(
            "Windows could not enumerate top-level windows",
        ));
    }
    if enumeration.same_executable_root_count == 0 {
        return Err(identity_error(
            "the exact HWND disappeared during capture identity proof",
        ));
    }
    validate_exact_window_owner(process_id, window_handle)?;
    Ok(route_for_same_executable_root_count(
        enumeration.same_executable_root_count,
    ))
}

pub(crate) fn validate_exact_window_owner(
    process_id: u32,
    window_handle: u64,
) -> Result<(), ExactWindowCaptureIdentityError> {
    let raw_handle = usize::try_from(window_handle)
        .map_err(|error| identity_error(format!("convert exact window handle: {error}")))?;
    let hwnd = raw_handle as HWND;
    if hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
        return Err(identity_error("the exact HWND no longer exists"));
    }
    let mut actual_process_id = 0_u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut actual_process_id) };
    if actual_process_id == 0 || actual_process_id != process_id {
        return Err(identity_error(
            "the exact HWND no longer belongs to the granted process",
        ));
    }
    if unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd {
        return Err(identity_error(
            "the exact HWND is not a top-level root window",
        ));
    }
    Ok(())
}

unsafe extern "system" fn count_same_executable_root(hwnd: HWND, lparam: LPARAM) -> i32 {
    let enumeration = unsafe { &mut *(lparam as *mut RootEnumeration) };
    if unsafe { IsWindowVisible(hwnd) } == 0 || unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd {
        return 1;
    }
    let mut process_id = 0_u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) };
    if process_id == 0 {
        return 1;
    }
    if process_id == enumeration.target_process_id
        || process_image_identity(process_id).is_ok_and(|image| image == enumeration.target_image)
    {
        enumeration.same_executable_root_count =
            enumeration.same_executable_root_count.saturating_add(1);
    }
    1
}

fn process_image_identity(process_id: u32) -> Result<String, ExactWindowCaptureIdentityError> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return Err(identity_error("the target process image is unavailable"));
    }
    let mut buffer = vec![0_u16; 32_768];
    let mut length = buffer.len() as u32;
    let queried = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            buffer.as_mut_ptr(),
            &mut length,
        )
    };
    unsafe { CloseHandle(process) };
    if queried == 0 || length == 0 {
        return Err(identity_error("the target process image is unavailable"));
    }
    buffer.truncate(length as usize);
    let image = std::ffi::OsString::from_wide(&buffer)
        .to_string_lossy()
        .to_lowercase();
    if image.is_empty() {
        return Err(identity_error("the target process image is unavailable"));
    }
    Ok(image)
}

fn identity_error(message: impl Into<String>) -> ExactWindowCaptureIdentityError {
    ExactWindowCaptureIdentityError(message.into())
}
