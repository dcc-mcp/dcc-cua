use super::*;
use windows::Win32::UI::WindowsAndMessaging::{GW_HWNDNEXT, GetTopWindow};

// Retain only the presenter's instance and sample bands. Every input fence
// checks them against the current native z-order, including cached placements.
#[derive(Clone, Copy)]
pub(in crate::platform) struct Exposure {
    window: usize,
    instance: usize,
    bands: [RECT; 4],
    expected: RECT,
}

impl Exposure {
    pub(super) fn new(
        window: &OverlayWindow,
        placement: Placement,
        monitor: MonitorGeometry,
        depth: i32,
    ) -> Self {
        let rect = |x: i32, y: i32, width: i32, height: i32| RECT {
            left: x,
            top: y,
            right: x.saturating_add(width),
            bottom: y.saturating_add(height),
        };
        let (bands, expected) = match placement {
            Placement::Edge([x, y, w, h]) => ([rect(x, y, w, h); 4], rect(x, y, w, h)),
            Placement::Ring(g) => {
                let left = g.x.saturating_sub(depth);
                let top = g.y.saturating_sub(depth);
                let right = g.x.saturating_add(g.width);
                let bottom = g.y.saturating_add(g.height);
                (
                    [
                        rect(left, g.y, depth, g.height),
                        rect(right, g.y, depth, g.height),
                        rect(g.x, top, g.width, depth),
                        rect(g.x, bottom, g.width, depth),
                    ],
                    RECT {
                        left,
                        top,
                        right: right.saturating_add(depth),
                        bottom: bottom.saturating_add(depth),
                    },
                )
            }
        };
        let bands = bands.map(|b| {
            let left = b.left.max(monitor.left);
            let right = b.right.min(monitor.right);
            let top = b.top.max(monitor.top);
            let bottom = b.bottom.min(monitor.bottom);
            if left >= right || top >= bottom {
                return RECT::default();
            }
            let width = right.saturating_sub(left);
            let height = bottom.saturating_sub(top);
            let w = width.min(24);
            let h = height.min(24);
            rect(left + (width - w) / 2, top + (height - h) / 2, w, h)
        });
        Self {
            window: window.0.0 as usize,
            instance: window.1,
            bands,
            expected,
        }
    }

    pub(in crate::platform) fn exposed(self) -> bool {
        // The renderer records monitor and band rectangles in physical pixels.
        // An input worker can arrive with a different thread DPI context.
        let Ok(_awareness) = ThreadDpiAwareness::enter() else {
            return false;
        };
        let window = HWND(self.window as *mut _);
        let mut bounds = RECT::default();
        if !unsafe { IsWindowVisible(window) }.as_bool()
            || unsafe { GetPropW(window, OVERLAY_INSTANCE_PROP) }.0 as usize != self.instance
            || unsafe { GetWindowRect(window, &mut bounds) }.is_err()
            || bounds != self.expected
        {
            return false;
        }
        let mut exposed = self.bands.map(|b| b.left < b.right && b.top < b.bottom);
        let mut next = unsafe { GetTopWindow(None) };
        for _ in 0..512 {
            let Ok(root) = next else { return false };
            if root == window {
                return exposed.iter().any(|value| *value);
            }
            if unsafe { IsWindowVisible(root) }.as_bool() {
                let mut above = RECT::default();
                if unsafe {
                    DwmGetWindowAttribute(
                        root,
                        DWMWA_EXTENDED_FRAME_BOUNDS,
                        (&raw mut above).cast(),
                        std::mem::size_of::<RECT>() as u32,
                    )
                }
                .is_err()
                    && unsafe { GetWindowRect(root, &mut above) }.is_err()
                {
                    return false;
                }
                for (visible, band) in exposed.iter_mut().zip(self.bands) {
                    if above.left < band.right
                        && above.right > band.left
                        && above.top < band.bottom
                        && above.bottom > band.top
                    {
                        *visible = false;
                    }
                }
                if exposed.iter().all(|value| !*value) {
                    return false;
                }
            }
            next = unsafe { GetWindow(root, GW_HWNDNEXT) };
        }
        false
    }
}
