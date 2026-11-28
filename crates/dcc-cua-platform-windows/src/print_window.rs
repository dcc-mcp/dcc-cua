//! HWND-owned content capture through the Win32 `PrintWindow` contract.
//!
//! This adapter never copies pixels from the desktop. It asks the exact HWND
//! to render into a private memory bitmap, so the result remains attributable
//! to the bound process/window even when another window covers the target.
//! Applications are allowed to decline or incompletely implement WM_PRINT,
//! therefore the caller keeps WGC as the preferred backend and treats this
//! adapter as a capability fallback.

use thiserror::Error;
use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HBITMAP, HDC, RGBQUAD, ReleaseDC,
        SelectObject,
    },
    UI::WindowsAndMessaging::GetWindowRect,
};

use crate::capture_identity::validate_exact_window_owner;
use crate::visible_capture::ThreadDpiAwarenessGuard;
use windows_sys::Win32::Storage::Xps::PrintWindow;

const PW_RENDERFULLCONTENT: u32 = 0x0002;
const MAX_PRINT_WINDOW_PIXELS: usize = 64 * 1024 * 1024;

#[derive(Debug, Error)]
#[error("PrintWindow capture failed: {message}")]
pub struct PrintWindowCaptureError {
    message: String,
    pub os_error: Option<i32>,
}

#[derive(Debug)]
pub struct PrintWindowCapture {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub bounds: [i32; 4],
}

fn capture_error(message: impl Into<String>) -> PrintWindowCaptureError {
    PrintWindowCaptureError {
        message: message.into(),
        os_error: None,
    }
}

fn capture_os_error(
    message: impl Into<String>,
    error: windows::core::Error,
) -> PrintWindowCaptureError {
    PrintWindowCaptureError {
        message: format!("{}: {error}", message.into()),
        os_error: Some(error.code().0),
    }
}

fn pixel_byte_len(width: i32, height: i32) -> Result<usize, PrintWindowCaptureError> {
    if width <= 0 || height <= 0 {
        return Err(capture_error("the exact HWND has empty physical bounds"));
    }
    let pixels = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or_else(|| capture_error("the exact HWND physical bounds overflow"))?;
    if pixels > MAX_PRINT_WINDOW_PIXELS {
        return Err(capture_error(
            "the exact HWND exceeds the PrintWindow pixel limit",
        ));
    }
    pixels
        .checked_mul(4)
        .ok_or_else(|| capture_error("the PrintWindow byte length overflows"))
}

fn read_printed_bitmap(
    memory_dc: HDC,
    bitmap: HBITMAP,
    width: i32,
    height: i32,
) -> Result<Vec<u8>, PrintWindowCaptureError> {
    let byte_len = pixel_byte_len(width, height)?;
    let mut bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: u32::try_from(byte_len)
                .map_err(|_| capture_error("the PrintWindow bitmap exceeds the DIB size limit"))?,
            ..Default::default()
        },
        bmiColors: [RGBQUAD::default(); 1],
    };
    let mut bgra = vec![0_u8; byte_len];
    let rows = unsafe {
        GetDIBits(
            memory_dc,
            bitmap,
            0,
            u32::try_from(height).map_err(|_| capture_error("the PrintWindow height overflows"))?,
            Some(bgra.as_mut_ptr().cast()),
            &mut bitmap_info,
            DIB_RGB_COLORS,
        )
    };
    if rows != height {
        return Err(capture_error(format!(
            "PrintWindow bitmap readback returned {rows} of {height} rows"
        )));
    }
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel[3] = 255; // BI_RGB reserves this byte instead of encoding alpha.
    }
    Ok(bgra)
}

/// Capture the exact HWND into a private bitmap without reading the desktop.
/// The target must remain the same native process/window instance across the
/// caller's before/after evidence fence.
pub fn capture_print_window(
    process_id: u32,
    window_handle: u64,
) -> Result<PrintWindowCapture, PrintWindowCaptureError> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    static BUSY: AtomicBool = AtomicBool::new(false);
    struct Permit;
    impl Drop for Permit {
        fn drop(&mut self) {
            BUSY.store(false, Ordering::Release);
        }
    }

    if BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(capture_error(
            "another PrintWindow request is still rendering",
        ));
    }
    let permit = Permit;
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("dcc-cua-print-window".into())
        .spawn(move || {
            // A timeout cannot cancel GDI. Keep all handles and the slot owned by this worker.
            let _permit = permit;
            let _ = tx.send(render_window(process_id, window_handle));
        })
        .map_err(|error| capture_error(format!("start PrintWindow worker: {error}")))?;
    rx.recv_timeout(Duration::from_millis(750))
        .map_err(|error| capture_error(format!("PrintWindow readback unavailable: {error}")))?
}

fn render_window(
    process_id: u32,
    window_handle: u64,
) -> Result<PrintWindowCapture, PrintWindowCaptureError> {
    let _dpi = ThreadDpiAwarenessGuard::per_monitor_v2()
        .map_err(|error| capture_error(error.to_string()))?;
    validate_exact_window_owner(process_id, window_handle)
        .map_err(|error| capture_error(error.to_string()))?;

    let hwnd = HWND(window_handle as usize as *mut _);
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }
        .map_err(|error| capture_os_error("read exact HWND bounds", error))?;
    let width = rect.right.saturating_sub(rect.left);
    let height = rect.bottom.saturating_sub(rect.top);
    let _ = pixel_byte_len(width, height)?;

    let window_dc = unsafe { GetDC(hwnd) };
    if window_dc.0.is_null() {
        return Err(capture_error("acquire the exact HWND device context"));
    }
    let memory_dc = unsafe { CreateCompatibleDC(window_dc) };
    if memory_dc.0.is_null() {
        unsafe { ReleaseDC(hwnd, window_dc) };
        return Err(capture_error(
            "create the private PrintWindow device context",
        ));
    }
    let bitmap = unsafe { CreateCompatibleBitmap(window_dc, width, height) };
    if bitmap.0.is_null() {
        unsafe {
            let _ = DeleteDC(memory_dc);
            ReleaseDC(hwnd, window_dc);
        }
        return Err(capture_error("create the private PrintWindow bitmap"));
    }
    let previous = unsafe { SelectObject(memory_dc, bitmap) };
    if previous.0.is_null() || previous.0 as isize == -1 {
        unsafe {
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(memory_dc);
            ReleaseDC(hwnd, window_dc);
        }
        return Err(capture_error("select the private PrintWindow bitmap"));
    }

    let printed =
        unsafe { PrintWindow(hwnd.0.cast(), memory_dc.0.cast(), PW_RENDERFULLCONTENT) != 0 };
    let restored = unsafe { SelectObject(memory_dc, previous) };
    let result = if !printed {
        Err(capture_error(
            "the exact HWND declined the PW_RENDERFULLCONTENT request",
        ))
    } else if restored.0 != bitmap.0 {
        Err(capture_error("restore the PrintWindow device context"))
    } else {
        read_printed_bitmap(memory_dc, bitmap, width, height)
    };
    unsafe {
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(memory_dc);
        ReleaseDC(hwnd, window_dc);
    }
    result.map(|bgra| PrintWindowCapture {
        bgra,
        width: width as u32,
        height: height as u32,
        bounds: [rect.left, rect.top, width, height],
    })
}

#[cfg(test)]
mod tests;
