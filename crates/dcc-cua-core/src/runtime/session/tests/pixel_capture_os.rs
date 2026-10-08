// Native acquisition is the sole source of replacement evidence in this model.
use rstest::rstest;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExactWindowCaptureRoute {
    Wgc,
    VerifiedVisible,
}
pub type ExactWindowCaptureIdentityError = String;
impl std::fmt::Display for VisibleWindowCaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
pub fn exact_window_pixel_evidence(
    pid: u32,
    hwnd: u64,
) -> Result<ExactWindowPixelEvidence, VisibleWindowCaptureError> {
    assert_eq!((pid, hwnd), (42, 77));
    OS.with_borrow_mut(|os| {
        os.trace.push("native evidence");
        Ok(os
            .evidence
            .pop_front()
            .expect("independently acquired evidence"))
    })
}
pub fn exact_window_capture_route(pid: u32, hwnd: u64) -> Result<ExactWindowCaptureRoute, String> {
    assert_eq!((pid, hwnd), (42, 77));
    Ok(OS.with_borrow(|os| {
        if os.backend == Backend::Visible {
            ExactWindowCaptureRoute::VerifiedVisible
        } else {
            ExactWindowCaptureRoute::Wgc
        }
    }))
}
pub struct VisibleWindowCapture {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub bounds: [i32; 4],
}
pub fn capture_visible_window(
    pid: u32,
    hwnd: u64,
) -> Result<VisibleWindowCapture, VisibleWindowCaptureError> {
    assert_eq!((pid, hwnd), (42, 77));
    OS.with_borrow_mut(|os| os.trace.push("visible pixels"));
    Ok(VisibleWindowCapture {
        bgra: vec![255; 800 * 600 * 4],
        width: 800,
        height: 600,
        bounds: [0, 0, 800, 600],
    })
}
pub struct PersistentWgcCapture;
pub struct WgcCaptureError(Option<WgcGeometryError>);
impl WgcCaptureError {
    pub fn geometry_failure(&self) -> Option<WgcGeometryError> {
        self.0
    }
}
impl std::fmt::Display for WgcCaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("modeled WGC refusal")
    }
}
impl std::fmt::Display for WgcGeometryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
pub struct PersistentWgcFrame {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub geometry: WgcFrameGeometry,
}
impl PersistentWgcCapture {
    pub fn new(pid: u32, hwnd: u64) -> Result<Self, WgcCaptureError> {
        assert_eq!((pid, hwnd), (42, 77));
        match OS.with_borrow(|os| os.backend) {
            Backend::WgcFailure => Err(WgcCaptureError(None)),
            Backend::WgcGeometryFailure => Err(WgcCaptureError(Some(
                WgcGeometryError::FrameMetadataUnavailable,
            ))),
            _ => Ok(Self),
        }
    }
    pub fn next_measured_frame(
        &mut self,
        _: Duration,
    ) -> Result<PersistentWgcFrame, WgcCaptureError> {
        OS.with_borrow_mut(|os| os.trace.push("WGC pixels"));
        let backend = OS.with_borrow(|os| os.backend);
        if backend == Backend::WgcReadbackGeometryFailure {
            return Err(WgcCaptureError(Some(WgcGeometryError::FrameSizeChanged)));
        }
        let size = if backend == Backend::WgcDwm {
            [780, 590]
        } else {
            [800, 600]
        };
        let mut geometry = WgcFrameGeometry {
            item_size_before: size,
            item_size_after: size,
            pool_size: size,
            content_size: size,
            texture_size: size,
            row_pitch_bytes: size[0] * 4,
        };
        if backend == Backend::WgcActualShapeDrift {
            geometry.item_size_after[0] += 1;
        }
        Ok(PersistentWgcFrame {
            bgra: vec![255; size[0] as usize * size[1] as usize * 4],
            width: size[0],
            height: size[1],
            geometry,
        })
    }
}
