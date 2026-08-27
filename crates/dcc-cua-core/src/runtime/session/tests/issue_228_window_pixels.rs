use super::*;
use rstest::rstest;

fn target(pid: u32, window_id: u64, bounds: [i32; 4]) -> WindowTarget {
    WindowTarget {
        pid,
        window_id,
        title: "Controlled custom renderer".into(),
        app_name: "fixture.exe".into(),
        bounds,
        is_foreground: true,
        is_minimized: false,
        is_on_screen: true,
        z_index: Some(3),
    }
}

#[rstest]
#[case(PixelObservationRoute::ExplicitPixelsOnly, "pixels_only", false)]
#[case(
    PixelObservationRoute::AccessibilityUnavailableDegraded,
    "accessibility_unavailable_degraded",
    true
)]
#[case(
    PixelObservationRoute::AccessibilityTimeoutDegraded,
    "accessibility_timeout_degraded",
    true
)]
fn issue_228_pixel_routes_have_distinct_stable_provenance(
    #[case] route: PixelObservationRoute,
    #[case] observation_mode: &str,
    #[case] degraded: bool,
) {
    let provenance = exact_window_pixel_provenance(
        route,
        &target(42, 77, [-1920, 100, 1280, 720]),
        11,
        144,
        "dcc-cua-wgc-exact-window",
    );

    assert_eq!(provenance["scope"], "window");
    assert_eq!(provenance["whole_desktop_capture"], false);
    assert_eq!(provenance["observation_mode"], observation_mode);
    assert_eq!(provenance["degraded"], degraded);
    assert_eq!(provenance["process_id"], 42);
    assert_eq!(provenance["window_handle"], 77);
    assert_eq!(provenance["capture_generation"], 11);
    assert_eq!(provenance["window_dpi"], 144);
    assert!(provenance.get("window_title").is_none());
}

#[rstest]
fn issue_228_missing_and_hung_accessibility_providers_degrade_but_target_loss_does_not() {
    let missing = ComputerUseError::new(
        ComputerUseErrorCode::BackendUnavailable,
        "controlled fixture has no accessibility provider",
    );
    let hung = ComputerUseError::new(
        ComputerUseErrorCode::InputFailed,
        "controlled provider exceeded the bounded observation deadline",
    )
    .with_details(ComputerUseErrorDetails {
        timed_out: Some(true),
        ..Default::default()
    });
    let vanished = ComputerUseError::new(
        ComputerUseErrorCode::TargetUnavailable,
        "controlled fixture window vanished",
    );
    let prose_only = ComputerUseError::new(
        ComputerUseErrorCode::InputFailed,
        "a title contains the words timed out",
    );

    assert_eq!(
        pixel_route_for_accessibility_failure(&missing),
        Some(PixelObservationRoute::AccessibilityUnavailableDegraded)
    );
    assert_eq!(
        pixel_route_for_accessibility_failure(&hung),
        Some(PixelObservationRoute::AccessibilityTimeoutDegraded)
    );
    assert_eq!(pixel_route_for_accessibility_failure(&vanished), None);
    assert_eq!(pixel_route_for_accessibility_failure(&prose_only), None);

    let unavailable_tool = cua_driver_sdk::ToolResult {
        is_error: true,
        error_code: Some("backend_unavailable".into()),
        raw_json: "{}".into(),
        text: "provider unavailable".into(),
        structured_json: None,
        images: Vec::new(),
        degraded: false,
        action: None,
        verification: None,
    };
    let timeout_tool = cua_driver_sdk::ToolResult {
        error_code: Some("uia_timeout".into()),
        text: "provider timed out".into(),
        ..unavailable_tool.clone()
    };
    assert_eq!(
        pixel_route_for_uia_tool_failure(&unavailable_tool),
        Some(PixelObservationRoute::AccessibilityUnavailableDegraded)
    );
    assert_eq!(
        pixel_route_for_uia_tool_failure(&timeout_tool),
        Some(PixelObservationRoute::AccessibilityTimeoutDegraded)
    );
}

#[rstest]
#[case(target(42, 77, [-1920, 100, 1280, 720]), "stable negative-coordinate monitor")]
#[case(target(42, 77, [3840, -600, 1600, 900]), "stable mixed-DPI monitor")]
fn issue_228_exact_window_publication_accepts_stable_native_evidence(
    #[case] before: WindowTarget,
    #[case] _label: &str,
) {
    let after = before.clone();
    validate_exact_window_pixel_publication(&before, &after, 144, 144, 9, 9)
        .expect("stable exact-window evidence publishes");
}

#[rstest]
#[case(target(43, 77, [0, 0, 800, 600]), 96, 5, "PID reuse")]
#[case(target(42, 78, [0, 0, 800, 600]), 96, 5, "HWND substitution")]
#[case(target(42, 77, [20, 0, 800, 600]), 96, 5, "window movement")]
#[case(target(42, 77, [0, 0, 801, 600]), 96, 5, "window resize")]
#[case(target(42, 77, [0, 0, 800, 600]), 120, 5, "DPI transition")]
#[case(target(42, 77, [0, 0, 800, 600]), 96, 6, "capture generation change")]
fn issue_228_exact_window_publication_rejects_stale_or_substituted_evidence(
    #[case] after: WindowTarget,
    #[case] after_dpi: u32,
    #[case] after_generation: u64,
    #[case] _label: &str,
) {
    let before = target(42, 77, [0, 0, 800, 600]);
    let error = validate_exact_window_pixel_publication(
        &before,
        &after,
        96,
        after_dpi,
        5,
        after_generation,
    )
    .expect_err("changed capture evidence must fail closed");
    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
}

#[rstest]
#[case(true, false, true, "minimized")]
#[case(false, true, true, "hidden")]
#[case(false, false, false, "occluded")]
fn issue_228_exact_window_publication_rejects_uncapturable_target_state(
    #[case] minimized: bool,
    #[case] hidden: bool,
    #[case] unobscured: bool,
    #[case] _label: &str,
) {
    let before = target(42, 77, [0, 0, 800, 600]);
    let mut after = before;
    after.is_minimized = minimized;
    after.is_on_screen = !hidden;
    let error = validate_exact_window_pixel_target_state(&after, unobscured)
        .expect_err("unsafe target state must fail closed");
    assert!(matches!(
        error.code,
        ComputerUseErrorCode::TargetMinimized
            | ComputerUseErrorCode::TargetUnavailable
            | ComputerUseErrorCode::CaptureFailed
    ));
}
