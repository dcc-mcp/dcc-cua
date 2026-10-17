//! Real foreground input only in the existing exclusive, owned CI fixture.
use dcc_cua_core::{
    ComputerUseAction, ComputerUseDriver, ComputerUseInputState, ComputerUseTargetScope,
};
use dcc_cua_indicator::{BannerTarget, ControlBanner};
#[allow(unused_imports)]
use rstest::rstest;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::{ComputerUseErrorCode, Duration, Instant};
use super::{bounded_session, inventory, native_fixture::Fixture};

async fn wait_for(mut predicate: impl FnMut() -> bool, reason: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "{reason}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn family(label: &str) -> (Vec<u64>, Vec<u64>) {
    let windows = inventory::read(std::process::id(), "owned-input-renderer-family");
    let banner = windows
        .iter()
        .find(|window| {
            window.identity.class == "DccCuaControlBanner" && window.identity.title == label
        })
        .expect("exact renderer banner");
    let thread = banner.identity.thread;
    let family: Vec<_> = windows
        .iter()
        .filter(|window| {
            window.identity.thread == thread
                && matches!(
                    window.identity.class.as_str(),
                    "DccCuaControlBanner" | "DccCuaControlFrame"
                )
        })
        .collect();
    (
        family.iter().map(|window| window.identity.hwnd).collect(),
        family
            .iter()
            .filter(|window| window.identity.class == "DccCuaControlFrame")
            .map(|window| window.identity.hwnd)
            .collect(),
    )
}

fn any_visible(windows: &[u64]) -> bool {
    windows
        .iter()
        .any(|raw| unsafe { IsWindowVisible(*raw as usize as HWND) } != 0)
}

fn assert_positive_overlap(raw: u64, target: u64) {
    let mut surface: RECT = unsafe { std::mem::zeroed() };
    let mut bounds: RECT = unsafe { std::mem::zeroed() };
    assert_ne!(
        unsafe { GetWindowRect(raw as usize as HWND, &mut surface) },
        0
    );
    assert_ne!(
        unsafe { GetWindowRect(target as usize as HWND, &mut bounds) },
        0
    );
    assert!(
        surface.left.max(bounds.left) < surface.right.min(bounds.right)
            && surface.top.max(bounds.top) < surface.bottom.min(bounds.bottom),
        "fixture presenter must actually overlap the native target"
    );
}

fn assert_blocked_by(target: u64, expected_roots: &[u64]) {
    let evidence =
        dcc_cua_platform_windows::exact_window_pixel_evidence(std::process::id(), target).unwrap();
    assert!(!evidence.unobscured);
    let failure = evidence
        .visibility_failure
        .expect("native failed-proof diagnostic");
    assert_eq!(
        failure.reason,
        dcc_cua_platform_windows::VisibleWindowCaptureReason::RootOverlap
    );
    assert_eq!(failure.blocker_process_id, Some(std::process::id()));
    let blocker = failure
        .blocker_window_handle
        .expect("exact native blocking root");
    assert!(
        expected_roots.contains(&blocker),
        "unexpected blocking root {blocker}: {failure:?}"
    );
}

pub(super) async fn verify(fixture: &Fixture, driver: &ComputerUseDriver) {
    let pid = std::process::id();
    let target = fixture.target as usize as HWND;
    let dpi = unsafe { GetDpiForWindow(target) };
    assert!(dpi > 0, "owned fixture DPI must be available");
    let width = (820_u64 * u64::from(dpi)).div_ceil(96) as i32;
    let height = (520_u64 * u64::from(dpi)).div_ceil(96) as i32;
    let screen = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    assert!(
        width + 48 < screen.0 && height + 80 < screen.1,
        "visible presenter fixture requires room: dpi={dpi} size={width}x{height} screen={screen:?}"
    );
    fixture.resize_input_roots(RECT {
        left: 16,
        top: 16,
        right: 16 + width,
        bottom: 16 + height,
    });
    fixture.focus_target();
    wait_for(
        || super::foreground() == target as usize,
        "owned fixture did not acquire foreground",
    )
    .await;
    eprintln!(
        "provider=dcc-cua runtime={} target_pid={pid} target_hwnd={} owned-input native fixture dpi={dpi}",
        env!("CARGO_PKG_VERSION"),
        fixture.target
    );
    let mut session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(pid),
                window_handle: Some(fixture.target),
                window_title: None,
            },
            "owned native input fixture",
            "owned-indicator-input",
        )
        .unwrap();
    let started = bounded_session(session.start_pixels_only()).await.unwrap();
    let (own_family, own_frames) = family(started["banner"]["label"].as_str().unwrap());
    assert!(
        own_family.len() > 1,
        "banner and physical frame bands must exist"
    );
    wait_for(
        || any_visible(&own_frames),
        "owned presenter never became visible",
    )
    .await;
    let visible_frame = own_frames
        .iter()
        .find(|raw| unsafe { IsWindowVisible(**raw as usize as HWND) } != 0)
        .unwrap();
    assert_positive_overlap(*visible_frame, fixture.target);

    // This is another genuine renderer, even though it shares the runtime and
    // target. Names, PID/class and matching bounds grant no input exemption.
    let peer = ControlBanner::start(BannerTarget {
        process_id: pid,
        window_handle: fixture.target,
        agent_name: "independent peer fixture".into(),
        application_name: "owned native input fixture".into(),
    })
    .unwrap();
    let (peer_family, peer_frames) = family(&peer.status().label);
    wait_for(
        || any_visible(&peer_frames),
        "peer presenter never became visible",
    )
    .await;
    let peer_source = peer.input_suppression_source();
    let source = peer_source.clone();
    let hwnd = fixture.target;
    let peer_lease = tokio::task::spawn_blocking(move || source.begin(pid, hwnd))
        .await
        .unwrap()
        .unwrap();
    assert!(!any_visible(&peer_family));
    assert!(
        any_visible(&own_frames),
        "private lease hid another task's renderer"
    );
    assert_blocked_by(fixture.target, &own_family);

    // An input lease may last longer than capture's ack timeout. A simultaneous
    // legitimate capture must still receive this renderer's acknowledgement.
    let capture_source = peer.capture_exclusion_source();
    let capture_lease = tokio::task::spawn_blocking(move || capture_source.begin())
        .await
        .unwrap()
        .unwrap();
    drop(capture_lease);
    assert!(
        !any_visible(&peer_family),
        "capture release revived input-suppressed surfaces"
    );
    drop(peer_lease);
    wait_for(
        || any_visible(&peer_frames) && any_visible(&own_frames),
        "presenters did not restore",
    )
    .await;

    let screenshot = bounded_session(session.screenshot_pixels_only())
        .await
        .unwrap();
    wait_for(
        || any_visible(&peer_frames) && any_visible(&own_frames),
        "capture did not restore presenters",
    )
    .await;
    let both_families: Vec<_> = own_family
        .iter()
        .chain(peer_family.iter())
        .copied()
        .collect();
    assert_blocked_by(fixture.target, &both_families);
    let baseline = fixture.target_key_events();
    let action = ComputerUseAction {
        action: "keypress".into(),
        keys: vec!["F8".into()],
        duration_ms: Some(80),
        observation_id: Some(screenshot.observation.observation_id),
        ..Default::default()
    };
    let refused = bounded_session(session.perform_action(&action))
        .await
        .expect_err("peer must occlude physical input");
    assert_eq!(refused.code, ComputerUseErrorCode::TargetUnavailable);
    assert!(refused.message.contains("TargetOccluded"), "{refused}");
    let details = refused.details.unwrap();
    assert_eq!(details.action_attempted, Some(false));
    assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
    assert_eq!(
        fixture.target_key_events(),
        baseline,
        "peer refusal inserted key events"
    );
    assert!(
        any_visible(&peer_frames),
        "native input hid the peer presenter"
    );

    // Destroying a stopped renderer while its lease exists cannot resurrect it.
    let source = peer_source.clone();
    let lease = tokio::task::spawn_blocking(move || source.begin(pid, hwnd))
        .await
        .unwrap()
        .unwrap();
    drop(peer);
    assert!(
        peer_family
            .iter()
            .all(|raw| unsafe { IsWindow(*raw as usize as HWND) } == 0)
    );
    drop(lease);
    assert!(peer_source.begin(pid, hwnd).is_err());
    assert!(
        peer_family
            .iter()
            .all(|raw| unsafe { IsWindow(*raw as usize as HWND) } == 0)
    );

    let screenshot = bounded_session(session.screenshot_pixels_only())
        .await
        .unwrap();
    wait_for(
        || any_visible(&own_frames),
        "owned frame must be visible before the positive input",
    )
    .await;
    assert_blocked_by(fixture.target, &own_family);
    let action = ComputerUseAction {
        observation_id: Some(screenshot.observation.observation_id),
        ..action
    };
    let delivered = bounded_session(session.perform_action(&action))
        .await
        .expect("own presentation must permit guarded native input");
    assert_eq!(delivered.value["delivery"]["delivery_completed"], true);
    assert_eq!(delivered.value["delivery"]["post_dispatch_validated"], true);
    wait_for(
        || fixture.target_key_events().len() >= baseline.len() + 2,
        "fixture did not read actual key down/up",
    )
    .await;
    assert_eq!(
        &fixture.target_key_events()[baseline.len()..],
        &[(WM_KEYDOWN, 0x77), (WM_KEYUP, 0x77)]
    );
    wait_for(
        || any_visible(&own_frames),
        "owned presentation did not restore after native postcheck",
    )
    .await;

    let screenshot = bounded_session(session.screenshot_pixels_only())
        .await
        .unwrap();
    // Wait for the capture lease's asynchronous presentation restoration, then
    // bind the foreign root's actual native overlap before dispatch.
    wait_for(
        || any_visible(&own_frames),
        "own frames did not restore before foreign refusal",
    )
    .await;
    fixture.block_capture(true);
    assert_blocked_by(fixture.target, &[fixture.blocker]);
    let foreign_baseline = fixture.target_key_events();
    let action = ComputerUseAction {
        observation_id: Some(screenshot.observation.observation_id),
        ..action
    };
    let refused = bounded_session(session.perform_action(&action))
        .await
        .expect_err("foreign root must still refuse");
    assert_eq!(refused.code, ComputerUseErrorCode::TargetUnavailable);
    assert!(refused.message.contains("TargetOccluded"), "{refused}");
    let details = refused.details.unwrap();
    assert_eq!(details.action_attempted, Some(false));
    assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
    assert_eq!(fixture.target_key_events(), foreign_baseline);
    assert_ne!(
        unsafe { IsWindowVisible(fixture.blocker as usize as HWND) },
        0,
        "foreign blocker was hidden"
    );
    fixture.block_capture(false);
    let stopped = bounded_session(session.stop()).await.unwrap();
    assert!(stopped.success && !stopped.active && !stopped.cleanup_pending);
    assert!(
        own_family
            .iter()
            .all(|raw| unsafe { IsWindow(*raw as usize as HWND) } == 0)
    );
    eprintln!(
        "owned indicator native input: real F8 down/up delivered; peer/foreign roots refused with zero events; scoped restoration, concurrent capture ack and stopped lease passed"
    );
}
