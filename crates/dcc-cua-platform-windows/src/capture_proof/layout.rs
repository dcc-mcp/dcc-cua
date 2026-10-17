//! Exact-target content-free layout facts. These never authorize a mutation.
use serde::Serialize;
use windows::Win32::{
    Foundation::{GetLastError, HWND, POINT, RECT, SetLastError, WIN32_ERROR},
    Graphics::Gdi::ClientToScreen,
    UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GWL_STYLE, GetClientRect, GetWindowLongPtrW, IsZoomed, WINDOW_LONG_PTR_INDEX,
    },
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativeTargetLayoutProof {
    pub maximized: bool,
    /// Raw 32-bit GWL_STYLE/GWL_EXSTYLE values, without host-specific interpretation.
    pub style: Option<u32>,
    pub style_win32_error: Option<u32>,
    pub extended_style: Option<u32>,
    pub extended_style_win32_error: Option<u32>,
    /// RECT edges [left, top, right, bottom] in target client coordinates.
    pub client_rect_edges: Option<[i32; 4]>,
    pub client_rect_hresult: Option<i32>,
    /// The same two corners mapped to physical desktop coordinates in PMv2 scope.
    pub client_screen_rect_edges: Option<[i32; 4]>,
    pub client_screen_win32_error: Option<u32>,
}

fn extents(edges: [i32; 4]) -> Option<[i32; 2]> {
    let width = edges[2].checked_sub(edges[0])?;
    let height = edges[3].checked_sub(edges[1])?;
    (width >= 0 && height >= 0).then_some([width, height])
}

impl NativeTargetLayoutProof {
    pub(super) fn complete(&self) -> bool {
        self.style.is_some()
            && self.style_win32_error.is_none()
            && self.extended_style.is_some()
            && self.extended_style_win32_error.is_none()
            && self.client_rect_hresult.is_none()
            && self.client_screen_win32_error.is_none()
            && self.client_rect_edges.and_then(extents).is_some()
            && self.client_rect_edges.and_then(extents)
                == self.client_screen_rect_edges.and_then(extents)
    }
}

pub(super) fn style_read_result(value: isize, last_error: u32) -> Result<u32, u32> {
    if value == 0 && last_error != 0 {
        Err(last_error)
    } else {
        Ok(value as u32)
    }
}

fn window_style(hwnd: HWND, index: WINDOW_LONG_PTR_INDEX) -> Result<u32, u32> {
    // Zero is a valid style value, so distinguish it from the documented error return.
    unsafe { SetLastError(WIN32_ERROR(0)) };
    let value = unsafe { GetWindowLongPtrW(hwnd, index) };
    style_read_result(value, unsafe { GetLastError() }.0)
}

fn screen_corners(hwnd: HWND, rect: RECT) -> Result<[i32; 4], u32> {
    let mut first = POINT {
        x: rect.left,
        y: rect.top,
    };
    let mut last = POINT {
        x: rect.right,
        y: rect.bottom,
    };
    for point in [&mut first, &mut last] {
        unsafe { SetLastError(WIN32_ERROR(0)) };
        if !unsafe { ClientToScreen(hwnd, point) }.as_bool() {
            return Err(unsafe { GetLastError() }.0);
        }
    }
    Ok([first.x, first.y, last.x, last.y])
}

pub(super) fn target_layout(hwnd: HWND) -> NativeTargetLayoutProof {
    let style = window_style(hwnd, GWL_STYLE);
    let extended_style = window_style(hwnd, GWL_EXSTYLE);
    let mut rect = RECT::default();
    let client = unsafe { GetClientRect(hwnd, &mut rect) };
    let screen = client.as_ref().ok().map(|()| screen_corners(hwnd, rect));
    NativeTargetLayoutProof {
        maximized: unsafe { IsZoomed(hwnd) }.as_bool(),
        style: style.as_ref().ok().copied(),
        style_win32_error: style.err(),
        extended_style: extended_style.as_ref().ok().copied(),
        extended_style_win32_error: extended_style.err(),
        client_rect_edges: client
            .as_ref()
            .ok()
            .map(|()| [rect.left, rect.top, rect.right, rect.bottom]),
        client_rect_hresult: client.err().map(|error| error.code().0),
        client_screen_rect_edges: screen.as_ref().and_then(|read| read.as_ref().ok().copied()),
        client_screen_win32_error: screen.and_then(Result::err),
    }
}
