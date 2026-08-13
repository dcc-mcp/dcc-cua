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
    let hwnd = window_handle as usize as HWND;
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
    Ok(route_for_same_executable_root_count(
        enumeration.same_executable_root_count,
    ))
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

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use windows_sys::Win32::{
        Foundation::{HINSTANCE, HWND},
        System::Threading::GetCurrentProcessId,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, HWND_TOPMOST, SWP_SHOWWINDOW, SendMessageW,
            SetWindowPos, WM_PAINT, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        },
    };

    use super::{ExactWindowCaptureRoute, route_for_same_executable_root_count};
    use crate::{capture_visible_window, exact_window_capture_route};

    #[rstest]
    #[case(1, ExactWindowCaptureRoute::Wgc)]
    #[case(2, ExactWindowCaptureRoute::VerifiedVisible)]
    #[case(3, ExactWindowCaptureRoute::VerifiedVisible)]
    fn same_executable_multi_window_capture_requires_independent_pixel_proof(
        #[case] root_count: usize,
        #[case] expected: ExactWindowCaptureRoute,
    ) {
        assert_eq!(route_for_same_executable_root_count(root_count), expected);
    }

    #[rstest]
    fn same_executable_windows_capture_pixels_from_their_exact_hwnds() {
        const SS_BLACKRECT: u32 = 0x0000_0004;
        const SS_WHITERECT: u32 = 0x0000_0006;
        let black = TestWindow::new("dcc-cua-black", SS_BLACKRECT, 40, 40);
        let white = TestWindow::new("dcc-cua-white", SS_WHITERECT, 360, 40);
        std::thread::sleep(std::time::Duration::from_millis(100));
        let process_id = unsafe { GetCurrentProcessId() };

        assert_eq!(
            exact_window_capture_route(process_id, black.raw()).unwrap(),
            ExactWindowCaptureRoute::VerifiedVisible
        );
        assert_eq!(
            exact_window_capture_route(process_id, white.raw()).unwrap(),
            ExactWindowCaptureRoute::VerifiedVisible
        );

        assert_exact_luma_or_fail_closed(&black, 0..=31);
        assert_exact_luma_or_fail_closed(&white, 224..=255);
    }

    struct TestWindow(HWND);

    impl TestWindow {
        fn new(title: &str, static_style: u32, x: i32, y: i32) -> Self {
            let class = wide("STATIC");
            let title = wide(title);
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE | static_style,
                    x,
                    y,
                    280,
                    220,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    HINSTANCE::default(),
                    std::ptr::null(),
                )
            };
            assert!(!hwnd.is_null(), "create exact-window capture fixture");
            unsafe {
                SetWindowPos(hwnd, HWND_TOPMOST, x, y, 280, 220, SWP_SHOWWINDOW);
                SendMessageW(hwnd, WM_PAINT, 0, 0);
            }
            Self(hwnd)
        }

        fn raw(&self) -> u64 {
            self.0 as usize as u64
        }

        fn raise(&self) {
            unsafe {
                SetWindowPos(
                    self.0,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                        | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                        | SWP_SHOWWINDOW,
                );
                SendMessageW(self.0, WM_PAINT, 0, 0);
            }
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.0) };
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn center_luma(bgra: &[u8], width: u32, height: u32) -> u8 {
        let index = ((height as usize / 2) * width as usize + width as usize / 2) * 4;
        let blue = u16::from(bgra[index]);
        let green = u16::from(bgra[index + 1]);
        let red = u16::from(bgra[index + 2]);
        ((red + green + blue) / 3) as u8
    }

    fn assert_exact_luma_or_fail_closed(
        window: &TestWindow,
        expected: std::ops::RangeInclusive<u8>,
    ) {
        window.raise();
        match capture_visible_window(window.raw()) {
            Ok(capture) => {
                let luma = center_luma(&capture.bgra, capture.width, capture.height);
                assert!(
                    expected.contains(&luma),
                    "exact HWND returned unexpected center luma {luma}"
                );
            }
            Err(error) => assert!(
                error
                    .to_string()
                    .contains("another root window covers the exact HWND"),
                "ambiguous pixels must fail closed: {error}"
            ),
        }
    }
}
