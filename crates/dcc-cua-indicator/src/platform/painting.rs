use super::*;

pub(super) fn paint_surface(
    device: windows::Win32::Graphics::Gdi::HDC,
    bounds: RECT,
    dpi: u32,
    theme: usize,
) {
    let radius = scale(12, dpi);
    let outer =
        unsafe { CreateRoundRectRgn(0, 0, bounds.right, bounds.bottom, radius * 2, radius * 2) };
    let inner = unsafe {
        CreateRoundRectRgn(
            1,
            1,
            bounds.right - 1,
            bounds.bottom - 1,
            radius * 2,
            radius * 2,
        )
    };
    if !outer.0.is_null() && !inner.0.is_null() {
        let border = unsafe { CreateSolidBrush(LINE_VARIANTS[theme.min(LINE_VARIANTS.len() - 1)]) };
        let surface =
            unsafe { CreateSolidBrush(SURFACE_VARIANTS[theme.min(SURFACE_VARIANTS.len() - 1)]) };
        let _ = unsafe { FillRgn(device, outer, border) };
        let _ = unsafe { FillRgn(device, inner, surface) };
        let _ = unsafe { DeleteObject(HGDIOBJ(border.0)) };
        let _ = unsafe { DeleteObject(HGDIOBJ(surface.0)) };
    } else {
        let surface = unsafe { CreateSolidBrush(SURFACE) };
        let _ = unsafe { FillRect(device, &bounds, surface) };
        let _ = unsafe { DeleteObject(HGDIOBJ(surface.0)) };
    }
    let _ = unsafe { DeleteObject(HGDIOBJ(outer.0)) };
    let _ = unsafe { DeleteObject(HGDIOBJ(inner.0)) };
}

pub(super) fn paint_fallback_icon(device: windows::Win32::Graphics::Gdi::HDC, dpi: u32) {
    let left = scale(10, dpi);
    let top = scale(10, dpi);
    let size = scale(24, dpi);
    let region = unsafe {
        CreateRoundRectRgn(
            left,
            top,
            left + size,
            top + size,
            scale(7, dpi),
            scale(7, dpi),
        )
    };
    if !region.0.is_null() {
        let brush = unsafe { CreateSolidBrush(ACCENT) };
        let _ = unsafe { FillRgn(device, region, brush) };
        let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
        let _ = unsafe { DeleteObject(HGDIOBJ(region.0)) };
    }
    draw_text(
        device,
        "D",
        RECT {
            left,
            top,
            right: left + size,
            bottom: top + size,
        },
        scale(12, dpi),
        FW_SEMIBOLD.0 as i32,
        TEXT,
        true,
    );
}

pub(super) fn paint_activity(
    device: windows::Win32::Graphics::Gdi::HDC,
    activity: BannerActivity,
    dpi: u32,
) {
    let left = scale(43, dpi);
    let top = scale(18, dpi);
    let size = scale(8, dpi);
    let region = unsafe { CreateEllipticRgn(left, top, left + size, top + size) };
    if !region.0.is_null() {
        let color = activity.color();
        let brush = unsafe { CreateSolidBrush(rgb(color.red, color.green, color.blue)) };
        let _ = unsafe { FillRgn(device, region, brush) };
        let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
        let _ = unsafe { DeleteObject(HGDIOBJ(region.0)) };
    }
}

pub(super) fn paint_copy(
    device: windows::Win32::Graphics::Gdi::HDC,
    bounds: RECT,
    dpi: u32,
    identity: &str,
    activity: BannerActivity,
    recording: bool,
    live_observation: bool,
) {
    let stop_width = scale(78, dpi);
    let divider_x = bounds.right - stop_width - scale(9, dpi);
    let divider = RECT {
        left: divider_x,
        top: scale(10, dpi),
        right: divider_x + 1,
        bottom: bounds.bottom - scale(10, dpi),
    };
    let brush = unsafe { CreateSolidBrush(LINE) };
    let _ = unsafe { FillRect(device, &divider, brush) };
    let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
    let text_left = scale(58, dpi);
    let recording_width = if recording { scale(48, dpi) } else { 0 };
    let live_width = if live_observation { scale(52, dpi) } else { 0 };
    let badges_left = divider_x - recording_width - live_width;
    let text_right = badges_left - scale(7, dpi);
    draw_text(
        device,
        identity,
        RECT {
            left: text_left,
            top: scale(3, dpi),
            right: text_right,
            bottom: scale(23, dpi),
        },
        scale(12, dpi),
        FW_SEMIBOLD.0 as i32,
        TEXT,
        false,
    );
    let mut badge_left = badges_left;
    if recording {
        paint_persistent_badge(device, "REC", badge_left, recording_width, RECORDING, dpi);
        badge_left += recording_width;
    }
    if live_observation {
        let live_color = BannerActivity::Observing.color();
        paint_persistent_badge(
            device,
            "LIVE",
            badge_left,
            live_width,
            rgb(live_color.red, live_color.green, live_color.blue),
            dpi,
        );
    }
    draw_text(
        device,
        activity.localized_label(&system_language_tag()),
        RECT {
            left: text_left,
            top: scale(21, dpi),
            right: text_right,
            bottom: scale(41, dpi),
        },
        scale(11, dpi),
        FW_NORMAL.0 as i32,
        MUTED,
        false,
    );
    let stop_label = if system_language_tag().starts_with("zh") {
        "Esc 停止"
    } else {
        "Esc Stop"
    };
    draw_text(
        device,
        stop_label,
        RECT {
            left: divider_x + scale(6, dpi),
            top: 0,
            right: bounds.right - scale(4, dpi),
            bottom: bounds.bottom,
        },
        scale(11, dpi),
        FW_SEMIBOLD.0 as i32,
        MUTED,
        true,
    );
}

fn paint_persistent_badge(
    device: windows::Win32::Graphics::Gdi::HDC,
    label: &str,
    left: i32,
    width: i32,
    color: COLORREF,
    dpi: u32,
) {
    let dot_size = scale(7, dpi);
    let dot_left = left + scale(3, dpi);
    let dot_top = scale(18, dpi);
    let dot =
        unsafe { CreateEllipticRgn(dot_left, dot_top, dot_left + dot_size, dot_top + dot_size) };
    if !dot.0.is_null() {
        let brush = unsafe { CreateSolidBrush(color) };
        let _ = unsafe { FillRgn(device, dot, brush) };
        let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
        let _ = unsafe { DeleteObject(HGDIOBJ(dot.0)) };
    }
    draw_text(
        device,
        label,
        RECT {
            left: dot_left + scale(10, dpi),
            top: scale(11, dpi),
            right: left + width - scale(3, dpi),
            bottom: scale(33, dpi),
        },
        scale(10, dpi),
        FW_SEMIBOLD.0 as i32,
        color,
        false,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    device: windows::Win32::Graphics::Gdi::HDC,
    text: &str,
    mut bounds: RECT,
    height: i32,
    weight: i32,
    color: COLORREF,
    centered: bool,
) {
    let font = unsafe {
        CreateFontW(
            -height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            u32::from(DEFAULT_PITCH.0),
            w!("Segoe UI"),
        )
    };
    if font.0.is_null() {
        return;
    }
    let previous = unsafe { SelectObject(device, HGDIOBJ(font.0)) };
    let _ = unsafe { SetBkMode(device, TRANSPARENT) };
    let _ = unsafe { SetTextColor(device, color) };
    let mut text = text.encode_utf16().collect::<Vec<_>>();
    let horizontal = if centered { DT_CENTER.0 } else { DT_LEFT.0 };
    let format = windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(
        horizontal | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0,
    );
    let _ = unsafe { DrawTextW(device, &mut text, &raw mut bounds, format) };
    let _ = unsafe { SelectObject(device, previous) };
    let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
}
