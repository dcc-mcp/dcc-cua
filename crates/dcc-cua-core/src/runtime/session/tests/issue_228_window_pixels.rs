use super::*;
use rstest::rstest;

#[cfg(windows)]
#[rstest]
#[tokio::test]
async fn issue_228_rejected_start_preserves_active_explicit_pixels_route() {
    check_rejected_start_preserves_pixels_route(false).await;
}

#[cfg(windows)]
#[rstest]
#[tokio::test]
async fn issue_228_rejected_start_preserves_active_explicit_pixels_route_with_request() {
    check_rejected_start_preserves_pixels_route(true).await;
}

#[cfg(windows)]
async fn check_rejected_start_preserves_pixels_route(guarded: bool) {
    // Remote in-memory driver only: the active guard must reject before any
    // provider, native target resolution, banner or capture can be entered.
    let (mut session, calls) = counting_session();
    session.pixel_observation_route = Some(PixelObservationRoute::ExplicitPixelsOnly);
    assert!(session.is_pixels_only());
    session.upstream_session_state = UpstreamSessionState::VisualOnly {
        reason: "explicit pixels-only observation; accessibility provider was not started".into(),
    };
    let observation_id = session.observation.as_ref().unwrap().observation_id.clone();
    let target_before = session.target.as_ref().unwrap().clone();
    let error = if guarded {
        session
            .start_with_request(&ComputerUseSessionStartRequest::default())
            .await
    } else {
        session.start().await
    }
    .expect_err("already active");
    assert_eq!(error.code, ComputerUseErrorCode::InvalidAction);
    assert!(session.active);
    assert_eq!(
        session.pixel_observation_route,
        Some(PixelObservationRoute::ExplicitPixelsOnly)
    );
    assert_eq!(
        session.observation.as_ref().unwrap().observation_id,
        observation_id
    );
    assert!(
        matches!(&session.upstream_session_state, UpstreamSessionState::VisualOnly { reason } if reason == "explicit pixels-only observation; accessibility provider was not started")
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    session
        .ensure_active()
        .expect("still eligible for observations");
    let target_after = session.target.as_ref().unwrap();
    assert_eq!(
        (
            target_before.pid,
            target_before.window_id,
            target_before.bounds
        ),
        (
            target_after.pid,
            target_after.window_id,
            target_after.bounds
        )
    );
    // Use the same provenance builder consumed by capture_window_pixels. The
    // actual screenshot method is exercised after both rejections in CI.
    let provenance = exact_window_pixel_provenance(
        session.pixel_observation_route.unwrap(),
        target_after,
        11,
        96,
        "test-native-boundary",
    );
    assert_eq!(provenance["observation_mode"], "pixels_only");
    assert_eq!(provenance["degraded"], false);
    assert_eq!(provenance["accessibility_available"], false);
}

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
        ComputerUseErrorCode::NoAccessibilityProvider,
        "controlled fixture has no accessibility provider",
    );
    let failed = ComputerUseError::new(
        ComputerUseErrorCode::BackendUnavailable,
        "controlled provider failed while reading a semantic child",
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
    assert_eq!(pixel_route_for_accessibility_failure(&failed), None);

    let unavailable_tool = cua_driver_sdk::ToolResult {
        is_error: true,
        error_code: Some("no_accessibility_provider".into()),
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
    let failed_tool = cua_driver_sdk::ToolResult {
        error_code: Some("backend_unavailable".into()),
        text: "provider failed".into(),
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
    assert_eq!(pixel_route_for_uia_tool_failure(&failed_tool), None);
}

fn publication_fence(
    bounds: [i32; 4],
    dpi: u32,
    generation: u64,
    mode: ExactWindowPixelCaptureMode,
) -> ExactWindowPixelPublicationFence {
    ExactWindowPixelPublicationFence {
        geometry: ExactWindowPixelGeometry { bounds, dpi },
        source_rect: bounds,
        generation,
        mode,
        instance: ExactWindowPixelInstanceIdentity {
            process_creation_time_100ns: 1001,
            window_thread_id: 7,
            window_class_hash: 0xA11CE,
            owner_window_handle: 0,
        },
    }
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
        publication_fence(
            captured.bounds,
            96,
            17,
            ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        ),
        publication_fence(
            final_native_bounds,
            final_native_dpi,
            18,
            ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        ),
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
        publication_fence(
            captured.bounds,
            96,
            17,
            ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        ),
        publication_fence(
            captured.bounds,
            96,
            18,
            ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        ),
        true,
    )
    .expect("stable visual fallback pixels may publish after accessibility");
}

#[rstest]
fn issue_228_visible_crop_fences_a_distinct_stable_physical_source_rect() {
    let captured = target(42, 77, [0, 0, 816, 616]);
    let source_rect = [8, 8, 800, 600];
    let mut before = publication_fence(
        captured.bounds,
        96,
        17,
        ExactWindowPixelCaptureMode::VisibleDesktopCrop,
    );
    before.source_rect = source_rect;
    let mut after = publication_fence(
        captured.bounds,
        96,
        18,
        ExactWindowPixelCaptureMode::VisibleDesktopCrop,
    );
    after.source_rect = source_rect;

    validate_final_exact_window_pixel_publication(&captured, &captured, before, after, true)
        .expect("stable DWM crop may differ from stable PMv2 inventory bounds");

    after.source_rect[0] += 1;
    let error =
        validate_final_exact_window_pixel_publication(&captured, &captured, before, after, true)
            .expect_err("DWM crop drift must discard desktop pixels");
    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
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
        publication_fence(
            captured.bounds,
            96,
            5,
            ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        ),
        publication_fence(
            final_native_bounds,
            final_native_dpi,
            6,
            ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        ),
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
        publication_fence(
            captured.bounds,
            144,
            5,
            ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        ),
        publication_fence(
            captured.bounds,
            144,
            6,
            ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        ),
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
        publication_fence(
            captured.bounds,
            120,
            7,
            ExactWindowPixelCaptureMode::WindowContent,
        ),
        publication_fence(
            captured.bounds,
            120,
            8,
            ExactWindowPixelCaptureMode::WindowContent,
        ),
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
        publication_fence(
            captured.bounds,
            120,
            7,
            ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        ),
        publication_fence(
            captured.bounds,
            120,
            8,
            ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        ),
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

#[rstest]
fn issue_228_final_publication_compares_distinct_capture_generations_and_modes() {
    let captured = target(42, 77, [0, 0, 800, 600]);
    let error = validate_final_exact_window_pixel_publication(
        &captured,
        &captured,
        publication_fence(
            captured.bounds,
            96,
            17,
            ExactWindowPixelCaptureMode::WindowContent,
        ),
        publication_fence(
            captured.bounds,
            96,
            18,
            ExactWindowPixelCaptureMode::VisibleDesktopCrop,
        ),
        true,
    )
    .expect_err("a fresh final capture may not change capture mode");

    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
}

#[rstest]
fn issue_228_final_publication_rejects_reused_numeric_pid_hwnd_with_new_instance() {
    let captured = target(42, 77, [0, 0, 800, 600]);
    let before = publication_fence(
        captured.bounds,
        96,
        17,
        ExactWindowPixelCaptureMode::WindowContent,
    );
    let mut after = publication_fence(
        captured.bounds,
        96,
        18,
        ExactWindowPixelCaptureMode::WindowContent,
    );
    after.instance = ExactWindowPixelInstanceIdentity {
        process_creation_time_100ns: 2002,
        window_thread_id: 7,
        window_class_hash: 0xB0B,
        owner_window_handle: 0,
    };
    let error =
        validate_final_exact_window_pixel_publication(&captured, &captured, before, after, true)
            .expect_err("same numeric PID/HWND must not hide process/window replacement");

    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
    assert_eq!(captured.pid, 42);
    assert_eq!(captured.window_id, 77);
}
