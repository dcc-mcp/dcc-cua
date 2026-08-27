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
fn issue_228_visual_fallback_revalidates_native_pixels_after_accessibility() {
    let source = include_str!("../observation.rs");
    let visual_fallback = source
        .split_once("async fn capture_window_visually")
        .expect("visual fallback implementation exists")
        .1
        .split_once("async fn capture_window_pixels")
        .expect("explicit pixel route follows the visual fallback")
        .0;
    let accessibility = visual_fallback
        .find("visual_fallback_accessibility")
        .expect("visual fallback awaits accessibility");
    let final_native = visual_fallback
        .find("sample_exact_window_pixel_evidence")
        .expect("visual fallback resamples exact native evidence");
    let final_fence = visual_fallback
        .find("validate_final_exact_window_pixel_publication")
        .expect("visual fallback applies the final pixel publication fence");
    let publication = visual_fallback
        .find("self.observation = Some")
        .expect("visual fallback publishes one bounded observation");

    assert!(
        accessibility < final_native && final_native < final_fence && final_fence < publication,
        "native evidence and the final fence must run after accessibility and before publication"
    );
}

#[rstest]
#[case(
    target(42, 77, [20, 0, 800, 600]),
    [20, 0, 800, 600],
    96,
    true,
    "move during accessibility fallback"
)]
#[case(
    target(42, 77, [0, 0, 801, 600]),
    [0, 0, 801, 600],
    96,
    true,
    "resize during accessibility fallback"
)]
#[case(
    target(42, 77, [0, 0, 800, 600]),
    [0, 0, 800, 600],
    120,
    true,
    "DPI change during accessibility fallback"
)]
#[case(
    target(42, 77, [0, 0, 800, 600]),
    [0, 0, 800, 600],
    96,
    false,
    "occlusion during accessibility fallback"
)]
fn issue_228_visual_fallback_discards_post_accessibility_pixel_drift(
    #[case] final_inventory: WindowTarget,
    #[case] final_native_bounds: [i32; 4],
    #[case] final_native_dpi: u32,
    #[case] final_unobscured: bool,
    #[case] _label: &str,
) {
    let captured = target(42, 77, [0, 0, 800, 600]);
    let mut published = false;
    let result = validate_final_exact_window_pixel_publication(
        &captured,
        &final_inventory,
        ExactWindowPixelGeometry {
            bounds: captured.bounds,
            dpi: 96,
        },
        ExactWindowPixelGeometry {
            bounds: final_native_bounds,
            dpi: final_native_dpi,
        },
        17,
        ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        final_unobscured,
    );
    if result.is_ok() {
        published = true;
    }

    assert!(result.is_err(), "post-accessibility drift must fail closed");
    assert!(!published, "stale visual fallback pixels must not publish");
}

#[rstest]
fn issue_228_visual_fallback_publishes_stable_post_accessibility_pixels() {
    let captured = target(42, 77, [0, 0, 800, 600]);
    validate_final_exact_window_pixel_publication(
        &captured,
        &captured,
        ExactWindowPixelGeometry {
            bounds: captured.bounds,
            dpi: 96,
        },
        ExactWindowPixelGeometry {
            bounds: captured.bounds,
            dpi: 96,
        },
        17,
        ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        true,
    )
    .expect("stable visual fallback pixels may publish after accessibility");
}

#[rstest]
#[case(
    target(42, 77, [20, 0, 800, 600]),
    [20, 0, 800, 600],
    96,
    "inventory and native bounds moved after capture"
)]
#[case(
    target(42, 77, [0, 0, 800, 600]),
    [20, 0, 800, 600],
    96,
    "native bounds moved after final inventory"
)]
#[case(
    target(42, 77, [0, 0, 800, 600]),
    [0, 0, 800, 600],
    120,
    "native DPI changed after capture"
)]
fn issue_228_final_publication_fence_rejects_geometry_drift_between_revalidations(
    #[case] final_inventory: WindowTarget,
    #[case] final_native_bounds: [i32; 4],
    #[case] final_native_dpi: u32,
    #[case] _label: &str,
) {
    let captured = target(42, 77, [0, 0, 800, 600]);
    let error = validate_final_exact_window_pixel_publication(
        &captured,
        &final_inventory,
        ExactWindowPixelGeometry {
            bounds: captured.bounds,
            dpi: 96,
        },
        ExactWindowPixelGeometry {
            bounds: final_native_bounds,
            dpi: final_native_dpi,
        },
        5,
        ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        true,
    )
    .expect_err("geometry drift after capture must discard the pixel evidence");
    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
}

#[rstest]
fn issue_228_final_publication_fence_accepts_only_exact_capture_geometry() {
    let captured = target(42, 77, [-1920, 100, 1280, 720]);
    validate_final_exact_window_pixel_publication(
        &captured,
        &captured,
        ExactWindowPixelGeometry {
            bounds: captured.bounds,
            dpi: 144,
        },
        ExactWindowPixelGeometry {
            bounds: captured.bounds,
            dpi: 144,
        },
        5,
        ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        true,
    )
    .expect("unchanged final native and inventory geometry may publish");
}

#[rstest]
fn issue_228_window_content_publication_does_not_require_desktop_visibility() {
    let captured = target(42, 77, [100, 200, 800, 600]);
    validate_final_exact_window_pixel_publication(
        &captured,
        &captured,
        ExactWindowPixelGeometry {
            bounds: captured.bounds,
            dpi: 120,
        },
        ExactWindowPixelGeometry {
            bounds: captured.bounds,
            dpi: 120,
        },
        7,
        ExactWindowPixelCaptureMode::WindowContent,
        false,
    )
    .expect("window-content pixels remain exact when another desktop window overlaps them");
}

#[rstest]
fn issue_228_visible_desktop_crop_publication_requires_complete_visibility() {
    let captured = target(42, 77, [100, 200, 800, 600]);
    let error = validate_final_exact_window_pixel_publication(
        &captured,
        &captured,
        ExactWindowPixelGeometry {
            bounds: captured.bounds,
            dpi: 120,
        },
        ExactWindowPixelGeometry {
            bounds: captured.bounds,
            dpi: 120,
        },
        7,
        ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        false,
    )
    .expect_err("desktop-crop pixels must not publish when any overlap is unproven");
    assert_eq!(error.code, ComputerUseErrorCode::CaptureFailed);
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
