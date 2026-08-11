use thiserror::Error;
use windows::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, RGBQUAD, ReleaseDC, SRCCOPY,
        SelectObject,
    },
    UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow,
        WindowFromPoint,
    },
};

#[derive(Debug, Error)]
#[error("visible exact-window capture failed: {0}")]
pub struct VisibleWindowCaptureError(String);

#[derive(Debug)]
pub struct VisibleWindowCapture {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

fn capture_error(message: impl Into<String>) -> VisibleWindowCaptureError {
    VisibleWindowCaptureError(message.into())
}

fn obscured_from_covered_samples(covered_samples: usize) -> bool {
    covered_samples >= 2
}

unsafe fn root_or_self(window: HWND) -> HWND {
    let root = unsafe { GetAncestor(window, GA_ROOT) };
    if root.0.is_null() { window } else { root }
}

unsafe fn target_is_obscured(target: HWND, rect: RECT) -> bool {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let center_x = rect.left + width / 2;
    let center_y = rect.top + height / 2;
    let points = [
        POINT {
            x: center_x,
            y: center_y,
        },
        POINT {
            x: rect.left + width / 4,
            y: center_y,
        },
        POINT {
            x: rect.left + width * 3 / 4,
            y: center_y,
        },
        POINT {
            x: center_x,
            y: rect.top + height / 4,
        },
        POINT {
            x: center_x,
            y: rect.top + height * 3 / 4,
        },
    ];
    let target_root = unsafe { root_or_self(target) };
    let mut target_process_id = 0_u32;
    unsafe { GetWindowThreadProcessId(target_root, Some(&mut target_process_id)) };
    let covered_samples = points
        .into_iter()
        .filter(|point| {
            let owner = unsafe { WindowFromPoint(*point) };
            if owner.0.is_null() {
                return false;
            }
            let owner_root = unsafe { root_or_self(owner) };
            if owner_root == target_root {
                return false;
            }
            let mut owner_process_id = 0_u32;
            unsafe { GetWindowThreadProcessId(owner_root, Some(&mut owner_process_id)) };
            owner_process_id == 0 || owner_process_id != target_process_id
        })
        .count();
    obscured_from_covered_samples(covered_samples)
}

/// Capture only an exact HWND's currently visible screen rectangle.
///
/// This fallback never asks the target process to paint. It is allowed only
/// when z-order sampling proves that another root window does not cover the
/// target, preventing a desktop crop from being mislabeled as target pixels.
pub fn capture_visible_window(
    window_handle: u64,
) -> Result<VisibleWindowCapture, VisibleWindowCaptureError> {
    let raw = usize::try_from(window_handle)
        .map_err(|error| capture_error(format!("convert window handle: {error}")))?;
    let hwnd = HWND(raw as *mut _);
    if hwnd.0.is_null() || !unsafe { IsWindow(hwnd) }.as_bool() {
        return Err(capture_error("the exact HWND no longer exists"));
    }
    if unsafe { IsIconic(hwnd) }.as_bool() {
        return Err(capture_error("the exact HWND is minimized"));
    }

    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }
        .map_err(|error| capture_error(format!("read exact window bounds: {error}")))?;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 4 || height <= 4 {
        return Err(capture_error(format!(
            "the exact HWND has invalid bounds {width}x{height}"
        )));
    }
    if unsafe { target_is_obscured(hwnd, rect) } {
        return Err(capture_error(
            "another root window covers the exact HWND at two or more verification points",
        ));
    }

    unsafe {
        let screen_dc = GetDC(HWND(std::ptr::null_mut()));
        if screen_dc.0.is_null() {
            return Err(capture_error("acquire desktop device context"));
        }
        let memory_dc = CreateCompatibleDC(screen_dc);
        if memory_dc.0.is_null() {
            ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
            return Err(capture_error("create compatible device context"));
        }
        let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
        if bitmap.0.is_null() {
            let _ = DeleteDC(memory_dc);
            ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
            return Err(capture_error("create compatible bitmap"));
        }
        let previous = SelectObject(memory_dc, bitmap);
        let copied = BitBlt(
            memory_dc, 0, 0, width, height, screen_dc, rect.left, rect.top, SRCCOPY,
        );
        let mut bitmap_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: (width * height * 4) as u32,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default(); 1],
        };
        let mut bgra = vec![0_u8; (width * height * 4) as usize];
        let rows = GetDIBits(
            memory_dc,
            bitmap,
            0,
            height as u32,
            Some(bgra.as_mut_ptr().cast()),
            &mut bitmap_info,
            DIB_RGB_COLORS,
        );
        SelectObject(memory_dc, previous);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(memory_dc);
        ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
        copied.map_err(|error| capture_error(format!("copy visible window pixels: {error}")))?;
        if rows == 0 {
            return Err(capture_error("read visible window bitmap"));
        }
        Ok(VisibleWindowCapture {
            bgra,
            width: width as u32,
            height: height as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(0, false)]
    #[case(1, false)]
    #[case(2, true)]
    #[case(5, true)]
    fn visible_crop_requires_at_least_four_of_five_target_samples(
        #[case] covered_samples: usize,
        #[case] expected_obscured: bool,
    ) {
        assert_eq!(
            obscured_from_covered_samples(covered_samples),
            expected_obscured
        );
    }
}
