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

#[rstest]
#[case("focus", ComputerUseErrorCode::TargetUnavailable)]
#[case("cover", ComputerUseErrorCode::CaptureFailed)]
#[case("revoke", ComputerUseErrorCode::InvalidAction)]
#[case("escape", ComputerUseErrorCode::UserInterrupted)]
#[tokio::test]
#[ignore = "requires a private authenticated Xvfb display"]
async fn linux_pixels_native_sequence_stops_after_delivered_prefix(
    #[case] cause: &'static str,
    #[case] expected: ComputerUseErrorCode,
    #[values("key_sequence", "drag")] operation: &str,
) {
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{InputFocus, KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
    use x11rb::protocol::xtest::ConnectionExt as _;

    let fixture = Fixture::new();
    let other = fixture.connection.generate_id().unwrap();
    fixture.create(other, 180, 20, 0);
    fixture
        .connection
        .set_input_focus(InputFocus::PARENT, fixture.window, x11rb::CURRENT_TIME)
        .unwrap()
        .check()
        .unwrap();
    let lease = Arc::new(crate::RecordingAuthorization::new(u64::MAX));
    let (mut session, _) = counting_session();
    session.active = false;
    session.scope = fixture.scope();
    session.live_observation = None;
    eprintln!(
        "provider=dcc-cua runtime={} pid={} hwnd={}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        fixture.window
    );
    session.start_pixels_only().await.unwrap();
    session
        .enable_guarded_unix_input(lease.clone())
        .await
        .unwrap();
    let snapshot = session.screenshot_pixels_only().await.unwrap();
    let mut fields = if operation == "drag" {
        json!({"path":[{"x":5,"y":5},{"x":55,"y":35}],"duration_ms":600,"steps":2})
    } else {
        json!({"keys":["A","B"],"delay_ms":200})
    };
    fields["action"] = json!(operation);
    fields["input_kind"] = json!("raw_input");
    fields["intent"] = json!("ordinary_edit");
    fields["delivery_mode"] = json!("foreground");
    fields["observation_id"] = json!(snapshot.observation.observation_id);
    let action = serde_json::from_value(fields).unwrap();
    let baseline = fixture
        .connection
        .query_tree(fixture.root)
        .unwrap()
        .reply()
        .unwrap()
        .children;
    let deadline = Instant::now() + Duration::from_secs(5);
    let intervene = async {
        loop {
            let recipient = match fixture.connection.poll_for_event().unwrap() {
                Some(Event::KeyRelease(key)) if operation == "key_sequence" => Some(key.event),
                Some(Event::ButtonPress(down)) if operation == "drag" => Some(down.event),
                _ => None,
            };
            if let Some(window) = recipient {
                assert_eq!(window, fixture.window);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "initial input was never delivered"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert_eq!(
            fixture
                .connection
                .query_tree(fixture.root)
                .unwrap()
                .reply()
                .unwrap()
                .children
                .len(),
            baseline.len() + 4
        );
        match cause {
            "focus" => fixture
                .connection
                .set_input_focus(InputFocus::PARENT, other, x11rb::CURRENT_TIME)
                .unwrap()
                .check()
                .unwrap(),
            "cover" => fixture
                .connection
                .configure_window(other, &ConfigureWindowAux::new().x(20).y(20))
                .unwrap()
                .check()
                .unwrap(),
            "revoke" => lease.revoke(),
            "escape" => {
                let first = fixture.connection.setup().min_keycode;
                let count = fixture.connection.setup().max_keycode - first + 1;
                let map = fixture
                    .connection
                    .get_keyboard_mapping(first, count)
                    .unwrap()
                    .reply()
                    .unwrap();
                let escape = first
                    + map
                        .keysyms
                        .chunks_exact(usize::from(map.keysyms_per_keycode))
                        .position(|keys| keys[0] == 0xff1b)
                        .unwrap() as u8;
                for kind in [KEY_PRESS_EVENT, KEY_RELEASE_EVENT] {
                    fixture
                        .connection
                        .xtest_fake_input(kind, escape, 0, fixture.root, 0, 0, 0)
                        .unwrap()
                        .check()
                        .unwrap();
                }
            }
            _ => unreachable!(),
        }
    };
    let (result, ()) = tokio::join!(
        session.perform_action_with_deadline(&action, Some(deadline)),
        intervene
    );
    let error = result.unwrap_err();
    assert_eq!(error.code, expected);
    assert_eq!(
        structured_error_details(&error).action_attempted,
        Some(true)
    );
    assert_eq!(structured_error_details(&error).blind_retry, Some(false));
    assert!(session.stop().await.unwrap().success);
    assert_eq!(
        fixture
            .connection
            .query_tree(fixture.root)
            .unwrap()
            .reply()
            .unwrap()
            .children,
        baseline
    );
    let mut releases = Vec::new();
    while let Some(event) = fixture.connection.poll_for_event().unwrap() {
        if let Event::ButtonRelease(up) = &event {
            releases.push((up.event, up.detail, up.event_x, up.event_y));
        }
        assert!(
            !matches!(event, Event::KeyPress(key) if key.response_type & 0x80 != 0)
                && !matches!(event, Event::MotionNotify(motion) if motion.response_type & 0x80 != 0),
            "synthetic input continued after interruption"
        );
    }
    if operation == "drag" {
        assert_eq!(releases, [(fixture.window, 1, 5, 5)]);
    }
}

#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
#[ignore = "requires a private authenticated Xvfb display"]
async fn linux_pixels_native_click_only_reaches_a_same_client_child(#[case] same_client: bool) {
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::InputFocus;
    let fixture = Fixture::new();
    let (foreign, _) = x11rb::connect(None).unwrap();
    let creator = if same_client {
        &fixture.connection
    } else {
        &foreign
    };
    let child = creator.generate_id().unwrap();
    creator
        .create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            child,
            fixture.window,
            5,
            5,
            20,
            20,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new(),
        )
        .unwrap()
        .check()
        .unwrap();
    creator.map_window(child).unwrap().check().unwrap();
    fixture
        .connection
        .set_input_focus(InputFocus::PARENT, fixture.window, x11rb::CURRENT_TIME)
        .unwrap()
        .check()
        .unwrap();
    let (mut session, _) = counting_session();
    session.active = false;
    session.scope = fixture.scope();
    session.live_observation = None;
    eprintln!(
        "provider=dcc-cua runtime={} pid={} hwnd={}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        fixture.window
    );
    session.start_pixels_only().await.unwrap();
    session
        .enable_guarded_unix_input(Arc::new(crate::RecordingAuthorization::new(u64::MAX)))
        .await
        .unwrap();
    let observation = session.screenshot_pixels_only().await.unwrap().observation;
    let action = ComputerUseAction {
        action: "click".into(),
        x: Some(8.),
        y: Some(9.),
        observation_id: Some(observation.observation_id),
        ..Default::default()
    };
    let result = session
        .perform_action_with_deadline(&action, Some(Instant::now() + Duration::from_secs(3)))
        .await;
    if same_client {
        result.unwrap();
        let mut coordinates = None;
        while let Some(event) = creator.poll_for_event().unwrap() {
            if let Event::ButtonRelease(button) = event {
                coordinates = Some((button.event, button.event_x, button.event_y));
            }
        }
        assert_eq!(coordinates, Some((child, 3, 4)));
    } else {
        let error = result.unwrap_err();
        assert_eq!(
            structured_error_details(&error).input_sent,
            Some(ComputerUseInputState::NotSent)
        );
        assert_eq!(error.code, ComputerUseErrorCode::TargetUnavailable);
    }
    assert!(session.stop().await.unwrap().success);
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
async fn linux_pixels_native_bounding_shape_must_intersect_the_target_to_occlude() {
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
    // A zero-input, full-screen sibling may paint only a few pixels outside the target.
    set_shape(&[Rectangle {
        x: 0,
        y: 0,
        width: 4,
        height: 1,
    }]);
    session.screenshot_pixels_only().await.unwrap();
    set_shape(&[Rectangle {
        x: 99,
        y: 79,
        width: 1,
        height: 1,
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
    assert!(session.unix_pixels.is_none());
    assert!(session.observation.is_none());
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}
