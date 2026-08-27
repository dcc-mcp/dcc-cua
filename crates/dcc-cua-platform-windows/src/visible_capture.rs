use crate::capture_identity::validate_exact_window_owner;
use thiserror::Error;
use windows::Win32::{
    Foundation::{BOOL, HWND, LPARAM, POINT, RECT},
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, RGBQUAD, ReleaseDC, SRCCOPY,
        SelectObject,
    },
    UI::{
        HiDpi::{GetDpiForWindow, LogicalToPhysicalPointForPerMonitorDPI},
        WindowsAndMessaging::{
            EnumWindows, GA_ROOT, GetAncestor, GetWindowRect, IsIconic, IsWindow, IsWindowVisible,
        },
    },
};

const MAX_ROOT_WINDOWS: usize = 4_096;

#[derive(Debug, Error)]
#[error("visible exact-window capture failed: {0}")]
pub struct VisibleWindowCaptureError(String);

#[derive(Debug)]
pub struct VisibleWindowCapture {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExactWindowPixelEvidence {
    pub process_id: u32,
    pub window_handle: u64,
    pub bounds: [i32; 4],
    pub dpi: u32,
    pub visible: bool,
    pub minimized: bool,
    pub unobscured: bool,
}

fn capture_error(message: impl Into<String>) -> VisibleWindowCaptureError {
    VisibleWindowCaptureError(message.into())
}

fn rectangles_intersect(left: [i32; 4], right: [i32; 4]) -> bool {
    if left[2] <= 0 || left[3] <= 0 || right[2] <= 0 || right[3] <= 0 {
        return false;
    }
    let left_edge = i64::from(left[0]);
    let left_top = i64::from(left[1]);
    let left_right = left_edge + i64::from(left[2]);
    let left_bottom = left_top + i64::from(left[3]);
    let right_edge = i64::from(right[0]);
    let right_top = i64::from(right[1]);
    let right_right = right_edge + i64::from(right[2]);
    let right_bottom = right_top + i64::from(right[3]);
    left_edge < right_right
        && right_edge < left_right
        && left_top < right_bottom
        && right_top < left_bottom
}

unsafe fn root_or_self(window: HWND) -> HWND {
    let root = unsafe { GetAncestor(window, GA_ROOT) };
    if root.0.is_null() { window } else { root }
}

pub(crate) fn root_z_order_proves_unobscured(
    target_window_handle: u64,
    target_bounds: [i32; 4],
    roots: &[(u64, [i32; 4], bool)],
) -> bool {
    for &(window_handle, bounds, visible) in roots {
        if window_handle == target_window_handle {
            return visible && bounds == target_bounds;
        }
        if visible && rectangles_intersect(bounds, target_bounds) {
            return false;
        }
    }
    false
}

#[derive(Default)]
struct RootZOrderEnumeration {
    target_window_handle: u64,
    roots: Vec<(u64, [i32; 4], bool)>,
    failed: bool,
}

unsafe extern "system" fn collect_root_z_order(window: HWND, context: LPARAM) -> BOOL {
    let enumeration = unsafe { &mut *(context.0 as *mut RootZOrderEnumeration) };
    if enumeration.roots.len() >= MAX_ROOT_WINDOWS {
        enumeration.failed = true;
        return BOOL(0);
    }
    let visible = unsafe { IsWindowVisible(window) }.as_bool();
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(window, &mut rect) }.is_err() {
        enumeration.failed = true;
        return BOOL(0);
    }
    let window_handle = window.0 as usize as u64;
    enumeration.roots.push((
        window_handle,
        [
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
        ],
        visible,
    ));
    if window_handle == enumeration.target_window_handle {
        BOOL(0)
    } else {
        BOOL(1)
    }
}

unsafe fn target_is_unobscured(target: HWND, rect: RECT) -> bool {
    let target_root = unsafe { root_or_self(target) };
    let target_window_handle = target_root.0 as usize as u64;
    let mut enumeration = RootZOrderEnumeration {
        target_window_handle,
        ..Default::default()
    };
    let result = unsafe {
        EnumWindows(
            Some(collect_root_z_order),
            LPARAM(&mut enumeration as *mut RootZOrderEnumeration as isize),
        )
    };
    if enumeration.failed {
        return false;
    }
    let target_was_reached = enumeration
        .roots
        .last()
        .is_some_and(|(window_handle, _, _)| *window_handle == target_window_handle);
    if result.is_err() && !target_was_reached {
        return false;
    }
    root_z_order_proves_unobscured(
        target_window_handle,
        [
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
        ],
        &enumeration.roots,
    )
}

unsafe fn physical_capture_rect(
    target: HWND,
    logical: RECT,
) -> Result<RECT, VisibleWindowCaptureError> {
    let mut top_left = POINT {
        x: logical.left,
        y: logical.top,
    };
    let mut bottom_right = POINT {
        x: logical.right,
        y: logical.bottom,
    };
    if !unsafe { LogicalToPhysicalPointForPerMonitorDPI(target, &mut top_left) }.as_bool()
        || !unsafe { LogicalToPhysicalPointForPerMonitorDPI(target, &mut bottom_right) }.as_bool()
    {
        return Err(capture_error(
            "convert the exact HWND rectangle to physical desktop pixels",
        ));
    }
    let physical = RECT {
        left: top_left.x,
        top: top_left.y,
        right: bottom_right.x,
        bottom: bottom_right.y,
    };
    if physical.right - physical.left <= 4 || physical.bottom - physical.top <= 4 {
        return Err(capture_error(
            "the exact HWND physical desktop rectangle is invalid",
        ));
    }
    Ok(physical)
}

/// Snapshot the native evidence used to fence one exact-window pixel frame.
pub fn exact_window_pixel_evidence(
    process_id: u32,
    window_handle: u64,
) -> Result<ExactWindowPixelEvidence, VisibleWindowCaptureError> {
    validate_exact_window_owner(process_id, window_handle)
        .map_err(|error| capture_error(error.to_string()))?;
    let raw = usize::try_from(window_handle)
        .map_err(|error| capture_error(format!("convert window handle: {error}")))?;
    let hwnd = HWND(raw as *mut _);
    if hwnd.0.is_null() || !unsafe { IsWindow(hwnd) }.as_bool() {
        return Err(capture_error("the exact HWND no longer exists"));
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
    validate_exact_window_owner(process_id, window_handle)
        .map_err(|error| capture_error(error.to_string()))?;
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        return Err(capture_error("the exact HWND DPI is unavailable"));
    }
    Ok(ExactWindowPixelEvidence {
        process_id,
        window_handle,
        bounds: [rect.left, rect.top, width, height],
        dpi,
        visible: unsafe { IsWindowVisible(hwnd) }.as_bool(),
        minimized: unsafe { IsIconic(hwnd) }.as_bool(),
        unobscured: unsafe { target_is_unobscured(hwnd, rect) },
    })
}

/// Capture only an exact HWND's currently visible screen rectangle.
///
/// This fallback never asks the target process to paint. It is allowed only
/// when complete root-window z-order proof shows that no higher root covers the
/// target, preventing a desktop crop from being mislabeled as target pixels.
pub fn capture_visible_window(
    process_id: u32,
    window_handle: u64,
) -> Result<VisibleWindowCapture, VisibleWindowCaptureError> {
    validate_exact_window_owner(process_id, window_handle)
        .map_err(|error| capture_error(error.to_string()))?;
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
    if !unsafe { target_is_unobscured(hwnd, rect) } {
        return Err(capture_error(
            "the exact HWND rectangle is covered or its complete root-window z-order could not be proven",
        ));
    }

    let physical_rect = unsafe { physical_capture_rect(hwnd, rect) }?;
    let physical_width = physical_rect.right - physical_rect.left;
    let physical_height = physical_rect.bottom - physical_rect.top;

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
        let bitmap = CreateCompatibleBitmap(screen_dc, physical_width, physical_height);
        if bitmap.0.is_null() {
            let _ = DeleteDC(memory_dc);
            ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
            return Err(capture_error("create compatible bitmap"));
        }
        let previous = SelectObject(memory_dc, bitmap);
        let copied = BitBlt(
            memory_dc,
            0,
            0,
            physical_width,
            physical_height,
            screen_dc,
            physical_rect.left,
            physical_rect.top,
            SRCCOPY,
        );
        let mut bitmap_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: physical_width,
                biHeight: -physical_height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: (physical_width * physical_height * 4) as u32,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default(); 1],
        };
        let mut bgra = vec![0_u8; (physical_width * physical_height * 4) as usize];
        let rows = GetDIBits(
            memory_dc,
            bitmap,
            0,
            physical_height as u32,
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
        validate_exact_window_owner(process_id, window_handle)
            .map_err(|error| capture_error(error.to_string()))?;
        Ok(VisibleWindowCapture {
            bgra,
            width: physical_width as u32,
            height: physical_height as u32,
        })
    }
}
