use super::*;
use windows::Win32::Graphics::Gdi::CreateRectRgn;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowDisplayAffinity, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
};

// This presenter has no surface in the target rectangle. It stays behind the
// exact target in z-order and is excluded by the compositor from capture.
const CLASS: PCWSTR = w!("DccCuaExteriorControlFrame");

pub(super) struct ExecutionFrame {
    window: OverlayWindow,
    placement: Option<Placement>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Placement {
    Ring(TargetGeometry),
    // A maximized window often leaves an exposed taskbar along one edge.
    Edge([i32; 4]),
}

impl ExecutionFrame {
    pub(super) fn new() -> Result<Self, IndicatorError> {
        let window = OverlayWindow::new(create_window(
            CLASS,
            Some(frame_window_proc),
            "",
            TARGET_FRAME_ALPHA_MAX,
        )?)?;
        let mut affinity = 0;
        unsafe {
            SetWindowDisplayAffinity(window.0, WDA_EXCLUDEFROMCAPTURE)
                .and_then(|()| GetWindowDisplayAffinity(window.0, &raw mut affinity))
        }
        .map_err(|error| IndicatorError::Rendering(format!("exclude execution frame: {error}")))?;
        if affinity != WDA_EXCLUDEFROMCAPTURE.0 {
            return Err(IndicatorError::Rendering(
                "execution frame exclusion was not applied".into(),
            ));
        }
        Ok(Self {
            window,
            placement: None,
        })
    }

    pub(super) fn update(
        &mut self,
        target: HWND,
        geometry: Option<TargetGeometry>,
    ) -> Result<bool, IndicatorError> {
        if !self.window.still_owned() {
            return Err(IndicatorError::Rendering(
                "execution frame identity changed".into(),
            ));
        }
        let Some(geometry) = geometry else {
            self.hide();
            return Ok(false);
        };
        let depth = scale(3, geometry.dpi);
        let monitor = read_monitor_geometry(target)?;
        let placement = if has_outer_edge(geometry, monitor, depth) {
            Some(Placement::Ring(geometry))
        } else {
            monitor_edge(geometry, monitor, depth).map(Placement::Edge)
        };
        let Some(placement) = placement else {
            self.hide();
            return Ok(false);
        };
        if self.placement == Some(placement) && unsafe { IsWindowVisible(self.window.0) }.as_bool()
        {
            return Ok(true);
        }
        match placement {
            Placement::Ring(geometry) => self.place(target, geometry, depth)?,
            Placement::Edge([x, y, width, height]) => unsafe {
                if SetWindowRgn(self.window.0, None, true) == 0 {
                    return Err(IndicatorError::Rendering(
                        "reset the execution edge region".into(),
                    ));
                }
                SetWindowPos(
                    self.window.0,
                    Some(HWND_TOPMOST),
                    x,
                    y,
                    width,
                    height,
                    SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
                )
            }
            .map_err(|error| {
                IndicatorError::Rendering(format!("position the execution edge: {error}"))
            })?,
        }
        self.placement = Some(placement);
        Ok(unsafe { IsWindowVisible(self.window.0) }.as_bool())
    }

    fn hide(&mut self) {
        if self.placement.take().is_some() {
            let _ = unsafe { ShowWindow(self.window.0, SW_HIDE) };
        }
    }

    fn place(
        &self,
        target: HWND,
        geometry: TargetGeometry,
        depth: i32,
    ) -> Result<(), IndicatorError> {
        let width = geometry.width.saturating_add(2 * depth);
        let height = geometry.height.saturating_add(2 * depth);
        unsafe {
            SetWindowPos(
                self.window.0,
                Some(HWND_NOTOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            )
            .map_err(|error| {
                IndicatorError::Rendering(format!("restore target-scoped frame order: {error}"))
            })?;
            let outer = CreateRectRgn(0, 0, width, height);
            let inner = CreateRectRgn(depth, depth, width - depth, height - depth);
            let shaped = !outer.0.is_null()
                && !inner.0.is_null()
                && CombineRgn(Some(outer), Some(outer), Some(inner), RGN_DIFF) != RGN_ERROR;
            let _ = DeleteObject(HGDIOBJ(inner.0));
            if !shaped || SetWindowRgn(self.window.0, Some(outer), true) == 0 {
                let _ = DeleteObject(HGDIOBJ(outer.0));
                return Err(IndicatorError::Rendering(
                    "shape the exterior execution frame".into(),
                ));
            }
            SetWindowPos(
                self.window.0,
                Some(target),
                geometry.x.saturating_sub(depth),
                geometry.y.saturating_sub(depth),
                width,
                height,
                SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            )
        }
        .map_err(|error| {
            IndicatorError::Rendering(format!("position the execution frame: {error}"))
        })
    }
}

fn monitor_edge(target: TargetGeometry, monitor: MonitorGeometry, depth: i32) -> Option<[i32; 4]> {
    let right = target.x.checked_add(target.width)?;
    let bottom = target.y.checked_add(target.height)?;
    if target.x < monitor.left
        || target.y < monitor.top
        || right > monitor.right
        || bottom > monitor.bottom
    {
        return None;
    }
    if monitor.bottom.saturating_sub(bottom) >= depth {
        Some([target.x, bottom, target.width, depth])
    } else if target.y.saturating_sub(monitor.top) >= depth {
        Some([target.x, target.y - depth, target.width, depth])
    } else if monitor.right.saturating_sub(right) >= depth {
        Some([right, target.y, depth, target.height])
    } else if target.x.saturating_sub(monitor.left) >= depth {
        Some([target.x - depth, target.y, depth, target.height])
    } else {
        None
    }
}

fn has_outer_edge(target: TargetGeometry, monitor: MonitorGeometry, depth: i32) -> bool {
    let right = target.x.saturating_add(target.width);
    let bottom = target.y.saturating_add(target.height);
    let fits = |start: i32, end: i32, from: i32, to: i32| start >= from && end <= to;
    let vertical = target.y < monitor.work_bottom && bottom > monitor.work_top;
    let horizontal = target.x < monitor.work_right && right > monitor.work_left;
    (vertical
        && (fits(
            target.x.saturating_sub(depth),
            target.x,
            monitor.work_left,
            monitor.work_right,
        ) || fits(
            right,
            right.saturating_add(depth),
            monitor.work_left,
            monitor.work_right,
        )))
        || (horizontal
            && (fits(
                target.y.saturating_sub(depth),
                target.y,
                monitor.work_top,
                monitor.work_bottom,
            ) || fits(
                bottom,
                bottom.saturating_add(depth),
                monitor.work_top,
                monitor.work_bottom,
            )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an interactive Windows desktop with DWM"]
    fn compositor_accepts_the_owned_execution_frame() {
        let _frame = ExecutionFrame::new().unwrap();
    }

    #[test]
    fn exterior_edge_must_fit_on_the_targets_monitor() {
        let monitor = MonitorGeometry {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
            work_left: -1920,
            work_top: 0,
            work_right: 0,
            work_bottom: 1040,
        };
        let full = TargetGeometry {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1040,
            dpi: 96,
        };
        assert!(!has_outer_edge(full, monitor, 3));
        assert_eq!(monitor_edge(full, monitor, 3), Some([-1920, 1040, 1920, 3]));
        assert_eq!(
            monitor_edge(
                TargetGeometry {
                    height: 1080,
                    ..full
                },
                monitor,
                3
            ),
            None
        );
        assert!(has_outer_edge(
            TargetGeometry {
                height: 1037,
                ..full
            },
            monitor,
            3
        ));
        assert!(!has_outer_edge(
            TargetGeometry {
                x: 100,
                y: 2000,
                ..full
            },
            monitor,
            3
        ));
    }
}
