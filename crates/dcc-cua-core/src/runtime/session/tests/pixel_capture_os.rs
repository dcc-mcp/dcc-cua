// Native acquisition is the sole source of replacement evidence in this model.
use rstest::rstest;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExactWindowCaptureRoute {
    Wgc,
    VerifiedVisible,
}
pub fn exact_window_pixel_evidence(
    pid: u32,
    hwnd: u64,
) -> Result<ExactWindowPixelEvidence, String> {
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
pub fn capture_visible_window(pid: u32, hwnd: u64) -> Result<VisibleWindowCapture, String> {
    assert_eq!((pid, hwnd), (42, 77));
    OS.with_borrow_mut(|os| os.trace.push("visible pixels"));
    Ok(VisibleWindowCapture {
        bgra: vec![255; 4],
        width: 1,
        height: 1,
        bounds: [0, 0, 800, 600],
    })
}
pub fn capture_window_content(pid: u32, hwnd: u64) -> Result<VisibleWindowCapture, String> {
    assert_eq!((pid, hwnd), (42, 77));
    OS.with_borrow_mut(|os| os.trace.push("PrintWindow pixels"));
    Ok(VisibleWindowCapture {
        bgra: vec![255; 4],
        width: 1,
        height: 1,
        bounds: [0, 0, 800, 600],
    })
}
pub struct PersistentWgcCapture;
impl PersistentWgcCapture {
    pub fn new(pid: u32, hwnd: u64) -> Result<Self, String> {
        assert_eq!((pid, hwnd), (42, 77));
        if OS.with_borrow(|os| os.backend == Backend::WgcFailure) {
            Err("modeled WGC refusal".into())
        } else {
            Ok(Self)
        }
    }
    pub fn next_frame(&mut self, _: Duration) -> Result<(Vec<u8>, u32, u32), String> {
        OS.with_borrow_mut(|os| os.trace.push("WGC pixels"));
        Ok((vec![255; 4], 1, 1))
    }
}
