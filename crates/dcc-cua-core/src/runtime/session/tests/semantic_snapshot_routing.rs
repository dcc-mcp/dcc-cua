// Provider sinks for the unchanged production screenshot dispatch prefix.
// No native API, worker, Host, capture or input is started by these tests.
use std::future::Future;
use std::task::{Context, Poll, Waker};

type ComputerUseResult<T> = Result<T, &'static str>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WindowTarget {
    pid: u32,
    window_id: u64,
}
#[derive(Debug, PartialEq, Eq)]
enum ComputerUseScreenshot {
    Semantic(WindowTarget, u32, u32),
    Visual(WindowTarget),
    Pixels(WindowTarget, PixelObservationRoute),
    Live,
}
enum UpstreamSessionState {
    Active,
    VisualOnly,
}
enum BannerActivity {
    Observing,
}
struct ComputerUseSession {
    target: Option<WindowTarget>,
    escalated: bool,
    windows_uia: Option<()>,
    live_observation: Option<()>,
    pixel_observation_route: Option<PixelObservationRoute>,
    upstream_session_state: UpstreamSessionState,
    semantic_calls: usize,
    visual_calls: usize,
}
impl ComputerUseSession {
    fn new() -> Self {
        Self {
            target: Some(WindowTarget {
                pid: 42,
                window_id: 77,
            }),
            escalated: false,
            windows_uia: None,
            live_observation: None,
            pixel_observation_route: None,
            upstream_session_state: UpstreamSessionState::Active,
            semantic_calls: 0,
            visual_calls: 0,
        }
    }
    fn ensure_active(&self) -> ComputerUseResult<()> {
        Ok(())
    }
    fn begin_banner_activity(&self, _: BannerActivity) {}
    async fn refresh_upstream_session_before_observation_if_needed(
        &mut self,
    ) -> ComputerUseResult<()> {
        Ok(())
    }
    async fn require_observed_target_available(&self) -> ComputerUseResult<WindowTarget> {
        self.target.ok_or("missing exact target")
    }
    async fn live_observation_screenshot(&self) -> ComputerUseResult<ComputerUseScreenshot> {
        Ok(ComputerUseScreenshot::Live)
    }
    async fn capture_window_pixels(
        &self,
        target: &WindowTarget,
        route: PixelObservationRoute,
    ) -> ComputerUseResult<ComputerUseScreenshot> {
        Ok(ComputerUseScreenshot::Pixels(*target, route))
    }
    async fn capture_window_visually(
        &mut self,
        target: &WindowTarget,
        _: u32,
        _: u32,
    ) -> ComputerUseResult<ComputerUseScreenshot> {
        self.visual_calls += 1;
        if !self.escalated {
            return Err("visual fallback requires approval");
        }
        Ok(ComputerUseScreenshot::Visual(*target))
    }
    fn semantic_snapshot(
        &mut self,
        target: &WindowTarget,
        elements: u32,
        depth: u32,
    ) -> ComputerUseResult<ComputerUseScreenshot> {
        self.semantic_calls += 1;
        // A successful local semantic snapshot retains the UIA context.
        self.windows_uia = Some(());
        Ok(ComputerUseScreenshot::Semantic(*target, elements, depth))
    }
}
fn ready<T>(future: impl Future<Output = T>) -> T {
    match std::pin::pin!(future)
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("provider sinks must complete without OS work"),
    }
}

#[test]
fn repeated_semantic_snapshots_keep_exact_target_and_bounds_without_escalation() {
    let mut session = ComputerUseSession::new();
    let target = session.target.unwrap();
    for (elements, depth) in [(50, 5), (100, 8)] {
        assert_eq!(
            ready(session.screenshot_with_bounds(elements, depth)),
            Ok(ComputerUseScreenshot::Semantic(target, elements, depth))
        );
    }
    assert_eq!(session.semantic_calls, 2);
    assert_eq!(session.visual_calls, 0);
    assert!(!session.escalated);
}

#[test]
fn explicitly_approved_visual_fallback_remains_available() {
    let mut session = ComputerUseSession::new();
    session.windows_uia = Some(());
    session.escalated = true;
    assert_eq!(
        ready(session.screenshot_with_bounds(50, 5)),
        Ok(ComputerUseScreenshot::Visual(session.target.unwrap()))
    );
    assert_eq!(session.visual_calls, 1);
    assert_eq!(session.semantic_calls, 0);
}

#[test]
fn explicit_and_degraded_pixel_routes_precede_cached_uia() {
    for route in [
        PixelObservationRoute::ExplicitPixelsOnly,
        PixelObservationRoute::AccessibilityUnavailableDegraded,
        PixelObservationRoute::AccessibilityTimeoutDegraded,
    ] {
        let mut session = ComputerUseSession::new();
        session.windows_uia = Some(());
        session.pixel_observation_route = Some(route);
        assert_eq!(
            ready(session.screenshot_with_bounds(50, 5)),
            Ok(ComputerUseScreenshot::Pixels(
                session.target.unwrap(),
                route
            ))
        );
        assert_eq!((session.semantic_calls, session.visual_calls), (0, 0));
    }
}

#[test]
fn visual_only_session_retains_typed_timeout_route() {
    let mut session = ComputerUseSession::new();
    session.windows_uia = Some(());
    session.upstream_session_state = UpstreamSessionState::VisualOnly;
    let route = PixelObservationRoute::AccessibilityTimeoutDegraded;
    assert_eq!(
        ready(session.screenshot_with_bounds(50, 5)),
        Ok(ComputerUseScreenshot::Pixels(
            session.target.unwrap(),
            route
        ))
    );
    assert_eq!(session.pixel_observation_route, Some(route));
    assert_eq!((session.semantic_calls, session.visual_calls), (0, 0));
}

#[test]
fn cached_uia_cannot_bypass_missing_exact_target() {
    let mut session = ComputerUseSession::new();
    session.windows_uia = Some(());
    session.target = None;
    assert_eq!(
        ready(session.screenshot_with_bounds(50, 5)),
        Err("missing exact target")
    );
    assert_eq!((session.semantic_calls, session.visual_calls), (0, 0));
}
