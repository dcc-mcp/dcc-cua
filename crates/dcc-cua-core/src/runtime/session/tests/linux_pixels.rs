use super::*;
use rstest::rstest;
use x11rb::connection::Connection as _;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClipOrdering, ConfigureWindowAux, ConnectionExt as _,
    CreateWindowAux, PropMode, Rectangle, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

#[rstest]
#[tokio::test]
async fn linux_pixels_read_only_gates_do_not_dispatch() {
    let (mut session, calls) = counting_session();
    session.pixel_observation_route = Some(PixelObservationRoute::ExplicitPixelsOnly);
    for error in [
        session.require_observed_input_available().unwrap_err(),
        session
            .require_observed_window_activation_available()
            .unwrap_err(),
        session.activate().await.unwrap_err(),
        session
            .call_bound_tool_without_refresh("get_session_state", json!({}))
            .await
            .unwrap_err(),
        session
            .call_mutating_bound_tool_without_refresh("bring_to_front", json!({}))
            .await
            .unwrap_err(),
    ] {
        assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
        assert_eq!(
            structured_error_details(&error).input_sent,
            Some(ComputerUseInputState::NotSent)
        );
    }
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

struct Fixture {
    connection: RustConnection,
    root: u32,
    window: u32,
}
impl Fixture {
    fn new() -> Self {
        let (connection, screen) =
            x11rb::connect(None).expect("CI must supply an authenticated Xvfb display");
        let root = connection.setup().roots[screen].root;
        let window = connection.generate_id().unwrap();
        let fixture = Self {
            connection,
            root,
            window,
        };
        fixture.create(window, 20, 20, 0xff0000);
        fixture
    }
    fn create(&self, window: u32, x: i16, y: i16, color: u32) {
        self.connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                self.root,
                x,
                y,
                80,
                60,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new().background_pixel(color),
            )
            .unwrap()
            .check()
            .unwrap();
        let atom = self
            .connection
            .intern_atom(false, b"_NET_WM_PID")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        self.connection
            .change_property32(
                PropMode::REPLACE,
                window,
                atom,
                AtomEnum::CARDINAL,
                &[std::process::id()],
            )
            .unwrap()
            .check()
            .unwrap();
        self.connection.map_window(window).unwrap().check().unwrap();
        self.sync();
    }
    fn sync(&self) {
        self.connection.get_input_focus().unwrap().reply().unwrap();
    }
    fn scope(&self) -> ComputerUseTargetScope {
        ComputerUseTargetScope {
            process_id: Some(std::process::id()),
            window_handle: Some(u64::from(self.window)),
            window_title: None,
        }
    }
}

// This is not silently skipped when a display is missing: CI invokes this
// ignored native test explicitly, in its own authenticated Xvfb display.
#[ignore = "requires a private non-composited X11 display; exercised in Linux CI"]
#[rstest]
#[tokio::test]
async fn linux_pixels_native_window_lifecycle_and_freshness() {
    let fixture = Fixture::new();
    let (mut session, calls) = counting_session();
    session.active = false;
    session.scope = fixture.scope();
    session.live_observation = None;
    session.start_pixels_only().await.unwrap();
    assert_eq!(session.status()["upstream_session"]["state"], "pixels_only");
    assert_eq!(session.status()["upstream_session"]["read_only"], true);
    let first = session.screenshot_pixels_only().await.unwrap();
    assert_eq!(
        (first.observation.width, first.observation.height),
        (80, 60)
    );
    assert_eq!(first.observation.process_id, std::process::id());
    assert_eq!(first.observation.window_handle, u64::from(fixture.window));
    assert_eq!(first.observation.capture_provenance["read_only"], true);
    assert_eq!(first.accessibility["accessibility_available"], false);
    fixture
        .connection
        .change_window_attributes(
            fixture.window,
            &ChangeWindowAttributesAux::new().background_pixel(0x0000ff),
        )
        .unwrap()
        .check()
        .unwrap();
    fixture
        .connection
        .clear_area(false, fixture.window, 0, 0, 0, 0)
        .unwrap()
        .check()
        .unwrap();
    fixture.sync();
    let second = session.screenshot().await.unwrap();
    assert_ne!(
        first.data, second.data,
        "a new GetImage must observe repainted pixels"
    );
    assert_eq!(
        second.observation.capture_provenance["capture_generation"],
        2
    );
    assert!(session.activate().await.is_err());
    assert_eq!(
        calls.load(AtomicOrdering::SeqCst),
        0,
        "no accessibility/provider or input calls"
    );
    // Destroy and immediately reuse exactly the same XID and PID. A new
    // stat/property lookup alone would incorrectly accept the replacement.
    fixture
        .connection
        .destroy_window(fixture.window)
        .unwrap()
        .check()
        .unwrap();
    fixture.create(fixture.window, 20, 20, 0xff0000);
    assert_eq!(
        session.screenshot_pixels_only().await.unwrap_err().code,
        ComputerUseErrorCode::InvalidTarget
    );
    assert!(session.observation.is_none());
    let stopped = session.stop().await.unwrap();
    assert!(stopped.success);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[ignore = "requires a private non-composited X11 display; exercised in Linux CI"]
#[rstest]
#[tokio::test]
async fn linux_pixels_native_empty_shaped_sibling_does_not_occlude() {
    let fixture = Fixture::new();
    let overlay = fixture.connection.generate_id().unwrap();
    fixture.create(overlay, 0, 0, 0x00ff00);
    fixture
        .connection
        .configure_window(overlay, &ConfigureWindowAux::new().width(640).height(480))
        .unwrap()
        .check()
        .unwrap();
    // Input passthrough never exempts visible pixels from occlusion.
    fixture
        .connection
        .shape_rectangles(
            SO::SET,
            SK::INPUT,
            ClipOrdering::UNSORTED,
            overlay,
            0,
            0,
            &[],
        )
        .unwrap()
        .check()
        .unwrap();
    let set_shape = |rectangles: &[Rectangle]| {
        fixture
            .connection
            .shape_rectangles(
                SO::SET,
                SK::BOUNDING,
                ClipOrdering::UNSORTED,
                overlay,
                0,
                0,
                rectangles,
            )
            .unwrap()
            .check()
            .unwrap();
        fixture.sync();
    };
    // The upstream Linux cursor renderer maps exactly this kind of empty
    // full-screen backing window before any cursor has been made visible.
    set_shape(&[]);
    let (mut session, calls) = counting_session();
    session.active = false;
    session.scope = fixture.scope();
    session.live_observation = None;
    session.start_pixels_only().await.unwrap();
    session.screenshot_pixels_only().await.unwrap();
    // Any nonempty shape restores conservative geometry-based occlusion,
    // even if the shape's own pixels happen to be outside the target.
    set_shape(&[Rectangle {
        x: 200,
        y: 150,
        width: 20,
        height: 20,
    }]);
    assert_eq!(
        session.screenshot_pixels_only().await.unwrap_err().code,
        ComputerUseErrorCode::CaptureFailed
    );
    assert!(session.observation.is_none());
    assert!(session.stop().await.unwrap().success);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[ignore = "requires a private X11 server; no real desktop is ever grabbed"]
#[rstest]
#[tokio::test]
async fn linux_pixels_native_cancel_and_stalled_server_drain_without_publication() {
    let fixture = Fixture::new();
    let (mut session, calls) = counting_session();
    session.active = false;
    session.scope = fixture.scope();
    session.live_observation = None;
    session.start_pixels_only().await.unwrap();
    session.screenshot_pixels_only().await.unwrap();
    // Freeze only this private Xvfb server from its fixture connection. The
    // capture connection must time out at transport level, not detach forever.
    fixture.connection.grab_server().unwrap().check().unwrap();
    let canceled =
        tokio::time::timeout(Duration::from_millis(40), session.screenshot_pixels_only()).await;
    assert!(canceled.is_err());
    assert!(
        session.observation.is_none(),
        "cancellation must consume earlier evidence before await"
    );
    let stopped = tokio::time::timeout(Duration::from_secs(6), session.stop())
        .await
        .expect("transport deadline must drain without releasing the fixture grab")
        .unwrap();
    fixture.connection.ungrab_server().unwrap().check().unwrap();
    assert!(stopped.success);
    assert!(session.linux_pixels.is_none());
    assert!(session.observation.is_none());
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}
