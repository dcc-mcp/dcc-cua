// Test-only model of OS acquisition, provider/target reads and the PNG sink.
// The capture function, validation bodies and both publication prefixes are
// compiled unchanged from production. No native API/Host is linked or run.
use rstest::rstest;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Wgc,
    Visible,
    WgcFailure,
    WgcGeometryFailure,
    WgcReadbackGeometryFailure,
    WgcActualShapeDrift,
    WgcDwm,
}
struct Native {
    backend: Backend,
    evidence: VecDeque<dcc_cua_platform_windows::ExactWindowPixelEvidence>,
    trace: Vec<&'static str>,
}
thread_local! { static OS: RefCell<Native> = RefCell::new(Native { backend: Backend::Wgc, evidence: VecDeque::new(), trace: vec![] }); }
static EXACT_WINDOW_CAPTURE_GENERATION: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComputerUseErrorCode {
    InvalidAction,
    InvalidTarget,
    StaleObservation,
    TargetMinimized,
    TargetUnavailable,
    CaptureFailed,
    BackendUnavailable,
    MissingWindow,
    InteractiveDesktopUnavailable,
}
#[derive(Debug)]
struct ComputerUseError {
    code: ComputerUseErrorCode,
    message: String,
    details: Option<ComputerUseErrorDetails>,
}
#[derive(Debug, Default)]
struct ComputerUseErrorDetails {
    capture: Option<ComputerUseCaptureDiagnostic>,
    phase: Option<ComputerUseErrorPhase>,
}
#[derive(Debug)]
enum ComputerUseErrorPhase {
    EvidenceDispatch,
}
impl ComputerUseError {
    fn new(code: ComputerUseErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }
    fn with_details(mut self, details: ComputerUseErrorDetails) -> Self {
        self.details = Some(details);
        self
    }
}
type ComputerUseResult<T> = Result<T, ComputerUseError>;
type ComputerUseScreenshot = ExactWindowCapture;
#[derive(Clone)]
struct WindowTarget {
    pid: u32,
    window_id: u64,
    bounds: [i32; 4],
    is_minimized: bool,
    is_on_screen: bool,
}
struct ControlBanner;
impl ControlBanner {
    fn begin_capture_exclusion(&self) -> Result<(), String> {
        Ok(())
    }
}
fn map_indicator_error(_: &str, _: String) -> ComputerUseError {
    ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, "indicator")
}
mod interactive_desktop {
    pub fn require_exact_window_observation_available() -> super::ComputerUseResult<()> {
        Ok(())
    }
}
mod tokio {
    pub mod task {
        pub fn spawn_blocking<T>(
            operation: impl FnOnce() -> T,
        ) -> std::future::Ready<Result<T, String>> {
            std::future::ready(Ok(operation()))
        }
    }
}
fn encode_bgra_to_png(_: &[u8], _: u32, _: u32) -> ComputerUseResult<Vec<u8>> {
    OS.with_borrow_mut(|os| os.trace.push("encode"));
    Ok(vec![1]) // encoding sink, not PNG correctness/native acceptance
}
pub(crate) mod capture_diagnostics {
    use super::*;
    pub struct PixelBufferDiagnostics;
    pub struct EncodedPixelFrame {
        pub data: Vec<u8>,
        pub diagnostics: Option<PixelBufferDiagnostics>,
    }
    pub fn encode_exact_frame(
        bgra: &[u8],
        width: u32,
        height: u32,
        enabled: bool,
        _: Duration,
    ) -> ComputerUseResult<EncodedPixelFrame> {
        // Hashes/PNG correctness use the real pure module's separate tests.
        Ok(EncodedPixelFrame {
            data: encode_bgra_to_png(bgra, width, height)?,
            diagnostics: enabled.then_some(PixelBufferDiagnostics),
        })
    }
}
struct ComputerUseSession {
    control_banner: Option<ControlBanner>,
    escalated: bool,
    target: Option<WindowTarget>,
    publications: usize,
}
impl ComputerUseSession {
    fn invalidate_action_observations(&mut self) {}
    async fn require_observed_target_available(&self) -> ComputerUseResult<WindowTarget> {
        Ok(self.target.clone().unwrap())
    }
    async fn revalidate_observed_exact_publication_target(
        &self,
        _: &WindowTarget,
    ) -> ComputerUseResult<WindowTarget> {
        Ok(self.target.clone().unwrap())
    }
    async fn visual_fallback_accessibility(&self, _: &WindowTarget, _: u32, _: u32, _: &str) {
        OS.with_borrow_mut(|os| os.trace.push("accessibility"));
    }
}
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut context) {
        std::task::Poll::Ready(result) => result,
        _ => panic!("modeled OS must complete"),
    }
}
fn evidence() -> dcc_cua_platform_windows::ExactWindowPixelEvidence {
    use dcc_cua_platform_windows::*;
    ExactWindowPixelEvidence {
        process_id: 42,
        window_handle: 77,
        bounds: [0, 0, 800, 600],
        visible_bounds: [0, 0, 800, 600],
        dpi: 96,
        visible: true,
        minimized: false,
        unobscured: true,
        instance: ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: 1001,
            window_thread_id: 7,
            window_class_hash: 99,
            owner_window_handle: 0,
        },
    }
}
fn session() -> (ComputerUseSession, WindowTarget) {
    let target = WindowTarget {
        pid: 42,
        window_id: 77,
        bounds: [0, 0, 800, 600],
        is_minimized: false,
        is_on_screen: true,
    };
    (
        ComputerUseSession {
            control_banner: None,
            escalated: true,
            target: Some(target.clone()),
            publications: 0,
        },
        target,
    )
}

#[rstest]
fn first_capture_instance_drift_cannot_reach_publication() {
    let mut failures = Vec::new();
    for backend in [Backend::Wgc, Backend::Visible, Backend::WgcFailure] {
        let a = evidence();
        let mut b = a;
        b.instance.process_creation_time_100ns += 1;
        OS.set(Native {
            backend,
            evidence: [a, b, b, b].into(),
            trace: vec![],
        });
        let (mut session, target) = session();
        let result = block_on(
            session.capture_window_pixels(&target, PixelObservationRoute::ExplicitPixelsOnly),
        );
        println!(
            "{backend:?}: first A->B, then B->B, result={:?}, publications={}, trace={:?}",
            result.as_ref().err(),
            session.publications,
            OS.with_borrow(|os| os.trace.clone())
        );
        if result.as_ref().err().map(|e| e.code) != Some(ComputerUseErrorCode::StaleObservation)
            || session.publications != 0
        {
            failures.push(backend);
        }
    }
    assert!(
        failures.is_empty(),
        "first-capture replacements published: {failures:?}"
    );
}

fn paths() -> [Option<PixelObservationRoute>; 4] {
    [
        Some(PixelObservationRoute::ExplicitPixelsOnly),
        Some(PixelObservationRoute::AccessibilityUnavailableDegraded),
        Some(PixelObservationRoute::AccessibilityTimeoutDegraded),
        None,
    ]
}

fn publish(
    session: &mut ComputerUseSession,
    target: &WindowTarget,
    path: Option<PixelObservationRoute>,
) -> ComputerUseResult<ExactWindowCapture> {
    match path {
        Some(route) => block_on(session.capture_window_pixels(target, route)),
        None => block_on(session.capture_window_visually(target, 32, 4)),
    }
}

fn changed_instance(component: usize) -> dcc_cua_platform_windows::ExactWindowPixelEvidence {
    let mut b = evidence();
    match component {
        0 => b.instance.process_creation_time_100ns += 1,
        1 => b.instance.window_thread_id += 1,
        2 => b.instance.window_class_hash += 1,
        3 => b.instance.owner_window_handle += 1,
        _ => unreachable!(),
    }
    // Every numeric target ID and both geometries remain unchanged. This is
    // simulated instance evidence, not a claimed live native handle race.
    let a = evidence();
    assert_eq!(
        (
            a.process_id,
            a.window_handle,
            a.bounds,
            a.visible_bounds,
            a.dpi
        ),
        (
            b.process_id,
            b.window_handle,
            b.bounds,
            b.visible_bounds,
            b.dpi
        )
    );
    b
}

#[rstest]
fn every_instance_component_is_fenced_inside_first_capture_on_all_shared_paths() {
    for backend in [Backend::Wgc, Backend::Visible, Backend::WgcFailure] {
        for path in paths() {
            for component in 0..4 {
                let a = evidence();
                let b = changed_instance(component);
                OS.set(Native {
                    backend,
                    evidence: [a, b, b, b].into(),
                    trace: vec![],
                });
                let (mut session, target) = session();
                let result = publish(&mut session, &target, path);
                assert_eq!(
                    result.err().map(|e| e.code),
                    Some(ComputerUseErrorCode::StaleObservation),
                    "{backend:?}/{path:?}/component{component}"
                );
                assert_eq!(session.publications, 0);
                OS.with_borrow(|os| {
                    assert_eq!(
                        os.evidence.len(),
                        2,
                        "first capture must fail before recapture"
                    );
                    assert_eq!(os.trace.len(), 3);
                    assert!(!os.trace.contains(&"encode") && !os.trace.contains(&"accessibility"));
                });
            }
        }
    }
}

#[rstest]
fn stable_instances_publish_on_all_shared_paths_and_capture_branches() {
    for backend in [Backend::Wgc, Backend::Visible, Backend::WgcFailure] {
        for path in paths() {
            let a = evidence();
            OS.set(Native {
                backend,
                evidence: [a; 4].into(),
                trace: vec![],
            });
            let (mut session, target) = session();
            let capture = publish(&mut session, &target, path).unwrap();
            assert_eq!(session.publications, 1);
            assert_eq!(capture.native_evidence, a);
            assert_eq!(
                capture.mode,
                if backend == Backend::Wgc {
                    ExactWindowPixelCaptureMode::WindowContent
                } else {
                    ExactWindowPixelCaptureMode::VisibleDesktopCrop
                }
            );
            OS.with_borrow(|os| {
                assert!(os.evidence.is_empty());
                assert_eq!(os.trace.iter().filter(|step| **step == "encode").count(), 2);
                assert_eq!(os.trace.contains(&"accessibility"), path.is_none());
            });
        }
    }
}

#[rstest]
fn late_instance_drift_remains_rejected_between_or_inside_final_capture() {
    for backend in [Backend::Wgc, Backend::Visible, Backend::WgcFailure] {
        for path in paths() {
            for component in 0..4 {
                let a = evidence();
                let b = changed_instance(component);
                for sequence in [[a, a, b, b], [a, a, a, b]] {
                    OS.set(Native {
                        backend,
                        evidence: sequence.into(),
                        trace: vec![],
                    });
                    let (mut session, target) = session();
                    assert_eq!(
                        publish(&mut session, &target, path).err().map(|e| e.code),
                        Some(ComputerUseErrorCode::StaleObservation)
                    );
                    assert_eq!(session.publications, 0);
                    OS.with_borrow(|os| {
                        assert!(os.evidence.is_empty());
                        assert!(
                            os.trace.contains(&"encode"),
                            "the stable first capture was actually reached"
                        );
                    });
                }
            }
        }
    }
}

#[rstest]
fn opt_in_diagnostics_use_only_the_existing_two_fenced_captures() {
    for backend in [Backend::Wgc, Backend::Visible, Backend::WgcFailure] {
        for enabled in [false, true] {
            let a = evidence();
            OS.set(Native {
                backend,
                evidence: [a, a, a, a].into(),
                trace: vec![],
            });
            let (mut session, target) = session();
            let captured = block_on(session.capture_window_pixels_with_diagnostics(
                &target,
                PixelObservationRoute::ExplicitPixelsOnly,
                enabled,
            ))
            .unwrap();
            assert_eq!(captured.diagnostics.is_some(), enabled);
            assert_eq!(session.publications, 1);
            assert_eq!(
                OS.with_borrow(|os| os.trace.iter().filter(|entry| **entry == "encode").count()),
                2
            );
            assert!(OS.with_borrow(|os| os.evidence.is_empty()));
        }
    }
}

#[rstest]
fn diagnostics_cannot_enable_a_semantic_degraded_capture() {
    OS.set(Native {
        backend: Backend::Visible,
        evidence: VecDeque::new(),
        trace: vec![],
    });
    let (mut session, target) = session();
    let result = block_on(session.capture_window_pixels_with_diagnostics(
        &target,
        PixelObservationRoute::AccessibilityUnavailableDegraded,
        true,
    ));
    assert_eq!(
        result.err().unwrap().code,
        ComputerUseErrorCode::InvalidAction
    );
    assert_eq!(session.publications, 0);
    assert!(OS.with_borrow(|os| os.trace.is_empty()));
}

#[rstest]
fn diagnostic_opt_in_never_publishes_after_native_instance_drift() {
    for backend in [Backend::Wgc, Backend::Visible, Backend::WgcFailure] {
        let a = evidence();
        let b = changed_instance(0);
        for sequence in [[a, b, b, b], [a, a, b, b], [a, a, a, b]] {
            OS.set(Native {
                backend,
                evidence: sequence.into(),
                trace: vec![],
            });
            let (mut session, target) = session();
            let result = block_on(session.capture_window_pixels_with_diagnostics(
                &target,
                PixelObservationRoute::ExplicitPixelsOnly,
                true,
            ));
            assert_eq!(
                result.err().unwrap().code,
                ComputerUseErrorCode::StaleObservation
            );
            assert_eq!(session.publications, 0);
        }
    }
}

#[rstest]
fn actual_wgc_geometry_failures_never_fall_back_or_publish() {
    for backend in [
        Backend::WgcGeometryFailure,
        Backend::WgcReadbackGeometryFailure,
        Backend::WgcActualShapeDrift,
    ] {
        for path in paths() {
            let a = evidence();
            OS.set(Native {
                backend,
                evidence: [a; 4].into(),
                trace: vec![],
            });
            let (mut session, target) = session();
            assert_eq!(
                publish(&mut session, &target, path).err().unwrap().code,
                ComputerUseErrorCode::StaleObservation
            );
            assert_eq!(session.publications, 0);
            OS.with_borrow(|os| {
                assert!(!os.trace.contains(&"visible pixels"));
                assert!(!os.trace.contains(&"encode"));
                assert!(!os.trace.contains(&"accessibility"));
            });
        }
    }
}

#[rstest]
fn actual_wgc_dimensions_publish_only_the_unique_dwm_origin() {
    for path in paths() {
        let mut a = evidence();
        a.visible_bounds = [10, 0, 780, 590];
        // WGC content may be occluded; this is not the desktop GDI route.
        a.unobscured = false;
        OS.set(Native {
            backend: Backend::WgcDwm,
            evidence: [a; 4].into(),
            trace: vec![],
        });
        let (mut session, target) = session();
        let capture = publish(&mut session, &target, path).unwrap();
        assert_eq!(session.publications, 1);
        assert_eq!(capture.bounds, [0, 0, 800, 600]);
        assert_eq!(capture.source_rect, [10, 0, 780, 590]);
        assert_eq!(
            capture.wgc_geometry.unwrap().origin,
            dcc_cua_platform_windows::WgcSourceOrigin::DwmExtendedFrame
        );
        OS.with_borrow(|os| {
            assert!(os.evidence.is_empty());
            assert_eq!(
                os.trace
                    .iter()
                    .filter(|step| **step == "WGC pixels")
                    .count(),
                2
            );
            assert_eq!(os.trace.iter().filter(|step| **step == "encode").count(), 2);
            assert!(!os.trace.contains(&"visible pixels"));
        });
    }
}
