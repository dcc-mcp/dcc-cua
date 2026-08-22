use rstest::rstest;

use super::*;

#[cfg(windows)]
#[rstest]
fn compact_targets_hide_banner_and_identity_selects_stable_hues() {
    assert!(platform::compact_target(platform::TargetGeometry {
        x: 0,
        y: 0,
        width: 799,
        height: 900,
        dpi: 96,
    }));
    assert!(!platform::compact_target(platform::TargetGeometry {
        x: 0,
        y: 0,
        width: 1200,
        height: 700,
        dpi: 96,
    }));
    assert_eq!(
        platform::theme_for_identity("Agent A"),
        platform::theme_for_identity("Agent A")
    );
    assert_ne!(
        platform::theme_for_identity("Agent A"),
        platform::theme_for_identity("Agent B")
    );
}

#[cfg(windows)]
#[rstest]
fn native_overlays_are_hit_test_transparent_and_never_activate() {
    assert_eq!(platform::overlay_input_result(0x0084).unwrap().0, -1);
    assert_eq!(platform::overlay_input_result(0x0021).unwrap().0, 3);
    assert!(platform::overlay_input_result(0x000f).is_none());
}

#[cfg(windows)]
#[rstest]
fn exact_target_foreground_uses_target_scoped_overlay_policy() {
    let policy =
        platform::target_presentation_policy(true, false, 0x100, 0x100, Some(0x100), Some(0x100));

    assert_eq!(
        policy,
        platform::TargetPresentationPolicy::ExactTargetForeground
    );
    assert!(policy.is_visible());
}

#[cfg(windows)]
#[rstest]
fn target_owned_modal_foreground_keeps_the_indicator_visible() {
    let policy =
        platform::target_presentation_policy(true, false, 0x100, 0x100, Some(0x200), Some(0x100));

    assert_eq!(
        policy,
        platform::TargetPresentationPolicy::OwnedModalForeground
    );
    assert!(policy.is_visible());
}

#[cfg(windows)]
#[rstest]
fn same_bounds_foreign_to_target_foreground_requests_exactly_one_z_order_resync() {
    let mut sync = platform::TargetOverlaySyncState::new(
        platform::TargetPresentationPolicy::TargetScopedBehindUnrelatedForeground,
    );

    let resyncs = [
        platform::TargetPresentationPolicy::TargetScopedBehindUnrelatedForeground,
        platform::TargetPresentationPolicy::ExactTargetForeground,
        platform::TargetPresentationPolicy::ExactTargetForeground,
    ]
    .into_iter()
    .filter(|presentation| sync.observe(*presentation))
    .count();

    assert_eq!(resyncs, 1);
}

#[cfg(windows)]
#[rstest]
fn hidden_to_visible_restore_requests_exactly_one_z_order_resync() {
    let mut sync =
        platform::TargetOverlaySyncState::new(platform::TargetPresentationPolicy::Hidden);

    let resyncs = [
        platform::TargetPresentationPolicy::Hidden,
        platform::TargetPresentationPolicy::ExactTargetForeground,
        platform::TargetPresentationPolicy::ExactTargetForeground,
    ]
    .into_iter()
    .filter(|presentation| sync.observe(*presentation))
    .count();

    assert_eq!(resyncs, 1);
}

#[cfg(windows)]
#[rstest]
#[case(platform::TargetPresentationPolicy::ExactTargetForeground)]
#[case(platform::TargetPresentationPolicy::TargetScopedBehindUnrelatedForeground)]
fn entering_owned_modal_foreground_requests_exactly_one_z_order_resync(
    #[case] previous: platform::TargetPresentationPolicy,
) {
    let mut sync = platform::TargetOverlaySyncState::new(previous);

    assert!(sync.observe(platform::TargetPresentationPolicy::OwnedModalForeground));
    assert!(!sync.observe(platform::TargetPresentationPolicy::OwnedModalForeground));
}

#[cfg(windows)]
#[rstest]
fn hidden_to_owned_modal_restore_requests_exactly_one_z_order_resync() {
    let mut sync =
        platform::TargetOverlaySyncState::new(platform::TargetPresentationPolicy::Hidden);

    assert!(sync.observe(platform::TargetPresentationPolicy::OwnedModalForeground));
    assert!(!sync.observe(platform::TargetPresentationPolicy::OwnedModalForeground));
}

#[cfg(windows)]
#[rstest]
fn stable_visible_ticks_do_not_churn_target_relative_z_order() {
    let mut sync = platform::TargetOverlaySyncState::new(
        platform::TargetPresentationPolicy::TargetScopedBehindUnrelatedForeground,
    );

    assert!(
        [platform::TargetPresentationPolicy::TargetScopedBehindUnrelatedForeground; 8]
            .into_iter()
            .all(|presentation| !sync.observe(presentation))
    );
}

#[cfg(windows)]
#[rstest]
fn unrelated_foreground_keeps_the_indicator_behind_that_window() {
    let policy =
        platform::target_presentation_policy(true, false, 0x100, 0x100, Some(0x300), Some(0x300));

    assert_eq!(
        policy,
        platform::TargetPresentationPolicy::TargetScopedBehindUnrelatedForeground
    );
    assert!(policy.is_visible());
}

#[cfg(windows)]
#[rstest]
fn native_overlay_tracks_the_exact_target_without_global_topmost() {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GW_HWNDNEXT, GW_OWNER, GWL_EXSTYLE, GetWindow,
        GetWindowLongPtrW, IsWindowVisible, WINDOW_EX_STYLE, WINDOW_STYLE, WS_EX_TOPMOST, WS_POPUP,
    };
    use windows::core::w;

    struct TestWindow(HWND);

    impl Drop for TestWindow {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }

    let target = TestWindow(
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("dcc-cua-indicator-test-target"),
                WINDOW_STYLE(WS_POPUP.0),
                0,
                0,
                64,
                64,
                None,
                None,
                None,
                None,
            )
        }
        .expect("create an isolated Win32 target window"),
    );
    let overlay = TestWindow(platform::create_frame_overlay().expect("create an isolated overlay"));
    platform::position_target_scoped_overlay(overlay.0, target.0, 4, 4, 48, 48, false)
        .expect("position the overlay directly above the exact target");
    assert!(!unsafe { IsWindowVisible(overlay.0).as_bool() });

    let owner = unsafe { GetWindow(overlay.0, GW_OWNER) }.unwrap_or(HWND(core::ptr::null_mut()));
    assert!(
        owner.0.is_null(),
        "the overlay must not become part of the target accessibility tree",
    );
    assert_eq!(
        unsafe { GetWindow(overlay.0, GW_HWNDNEXT) }.expect("read overlay z-order successor"),
        target.0,
        "the exact granted target HWND must remain directly below the overlay",
    );
    let ex_style = unsafe { GetWindowLongPtrW(overlay.0, GWL_EXSTYLE) } as u32;
    assert_eq!(
        ex_style & WS_EX_TOPMOST.0,
        0,
        "a target-owned overlay must not leak above unrelated foreground windows",
    );
}

#[rstest]
#[case(0, 1)]
#[case(1, 0)]
fn banner_target_requires_exact_process_and_window(
    #[case] process_id: u32,
    #[case] window_handle: u64,
) {
    let error = BannerTarget {
        process_id,
        window_handle,
        agent_name: "Codex".into(),
        application_name: "Maya".into(),
    }
    .validate()
    .expect_err("invalid banner target must be rejected");
    assert!(matches!(error, IndicatorError::InvalidTarget(_)));
}

#[rstest]
#[case(BannerActivity::Connecting, "正在连接…")]
#[case(BannerActivity::Ready, "已连接 · 等待操作")]
#[case(BannerActivity::Observing, "正在观察画面")]
#[case(BannerActivity::PointerInput, "正在使用鼠标")]
#[case(BannerActivity::KeyboardInput, "正在输入文本")]
#[case(BannerActivity::Navigating, "正在切换界面")]
#[case(BannerActivity::Waiting, "正在等待应用")]
#[case(BannerActivity::Recording, "正在录制")]
#[case(BannerActivity::Stopping, "正在停止…")]
#[case(BannerActivity::Operating, "正在操作")]
fn every_banner_activity_has_operator_visible_copy(
    #[case] activity: BannerActivity,
    #[case] expected: &str,
) {
    assert_eq!(activity.localized_label("zh-CN"), expected);
    assert_eq!(BannerActivity::from_code(activity as u8), activity);
}

#[rstest]
fn input_states_share_the_warning_color_without_session_randomization() {
    assert_eq!(
        BannerActivity::PointerInput.color(),
        BannerActivity::KeyboardInput.color()
    );
    assert_eq!(
        BannerActivity::Ready.color(),
        BannerColor {
            red: 115,
            green: 215,
            blue: 167,
        }
    );
    assert_eq!(
        BannerActivity::Operating.color(),
        BannerActivity::PointerInput.color(),
    );
    assert_eq!(
        BannerActivity::Operating.localized_label("en-US"),
        "Operating",
    );
}

#[rstest]
fn persistent_banner_indicators_do_not_replace_the_current_operation() {
    let indicators = BannerIndicators {
        recording: true,
        live_observation: true,
    };

    assert_eq!(
        BannerActivity::PointerInput.presented_with(indicators),
        BannerActivity::PointerInput,
        "a pointer action must remain visible while showcase recording continues",
    );
    assert_eq!(
        indicators.badges(),
        [
            Some(BannerBadge::Recording),
            Some(BannerBadge::LiveObservation),
        ],
        "persistent REC and LIVE badges must remain visible during an operation",
    );
    assert_eq!(
        BannerActivity::Ready.presented_with(indicators),
        BannerActivity::Observing,
        "an idle session with live observation should return to observing",
    );
    assert!(indicators.recording);
}

#[rstest]
fn trajectory_only_recording_remains_visible_while_idle() {
    let indicators = BannerIndicators {
        recording: true,
        live_observation: false,
    };

    assert_eq!(
        BannerActivity::Ready.presented_with(indicators),
        BannerActivity::Recording,
    );
}

#[rstest]
fn activity_guard_restores_idle_after_error_paths() {
    let activity = std::sync::Arc::new(BannerActivitySignal::new(BannerActivity::Ready));
    {
        let _guard = BannerActivityGuard::begin(
            std::sync::Arc::clone(&activity),
            BannerActivity::PointerInput,
        );
    }
    assert_eq!(activity.load(), BannerActivity::Ready);
}

#[rstest]
fn stale_activity_guard_cannot_clear_a_newer_operation() {
    let activity = std::sync::Arc::new(BannerActivitySignal::new(BannerActivity::Ready));
    let stale = BannerActivityGuard::begin(
        std::sync::Arc::clone(&activity),
        BannerActivity::PointerInput,
    );
    let current =
        BannerActivityGuard::begin(std::sync::Arc::clone(&activity), BannerActivity::Operating);

    drop(stale);
    assert_eq!(activity.load(), BannerActivity::Operating);
    drop(current);
    assert_eq!(activity.load(), BannerActivity::Ready);
}

#[rstest]
fn indicator_failures_preserve_target_loss_as_a_typed_status() {
    let failure = BannerFailure::from(&IndicatorError::InvalidTarget(
        "control target window no longer exists".into(),
    ));

    assert_eq!(failure.kind, BannerFailureKind::TargetLost);
    assert_eq!(
        failure.message,
        "invalid banner target: control target window no longer exists"
    );
    assert_eq!(
        serde_json::to_value(&failure).expect("banner failure serializes")["kind"],
        "target_lost"
    );
}

#[rstest]
fn backend_indicator_failures_remain_distinct_from_target_loss() {
    let failure = BannerFailure::from(&IndicatorError::Backend("paint failed".into()));

    assert_eq!(failure.kind, BannerFailureKind::Backend);
    assert_eq!(
        failure.message,
        "control banner backend failed: paint failed"
    );
}

#[rstest]
fn shared_theme_contract_drives_cursor_indicator_and_motion_tokens() {
    let contract: serde_json::Value =
        serde_json::from_str(include_str!("../theme/dcc-cua-theme.json"))
            .expect("packaged theme contract must be valid JSON");
    assert_eq!(contract["cursor"]["theme_id"], SHARED_CURSOR_THEME_ID,);
    assert_eq!(
        contract["cursor"]["reduced_motion"],
        SHARED_REDUCED_MOTION_POLICY,
    );
    let accent = contract["cursor"]["accent"]
        .as_str()
        .expect("cursor accent is a string");
    assert_eq!(accent, "#A663FF");
    assert_eq!(color_from_token(theme_tokens::ACCENT), SHARED_CURSOR_ACCENT,);
}

#[rstest]
#[case(
    IndicatorMotionPolicy::Auto,
    true,
    ResolvedIndicatorMotion::Animate,
    true,
    IndicatorMotionSource::SystemPreference
)]
#[case(
    IndicatorMotionPolicy::Auto,
    false,
    ResolvedIndicatorMotion::Reduce,
    false,
    IndicatorMotionSource::SystemPreference
)]
#[case(
    IndicatorMotionPolicy::Reduce,
    true,
    ResolvedIndicatorMotion::Reduce,
    false,
    IndicatorMotionSource::SessionOverride
)]
#[case(
    IndicatorMotionPolicy::Animate,
    false,
    ResolvedIndicatorMotion::Animate,
    true,
    IndicatorMotionSource::SessionOverride
)]
fn session_motion_policy_resolves_to_auditable_indicator_behavior(
    #[case] requested: IndicatorMotionPolicy,
    #[case] system_animations: bool,
    #[case] resolved: ResolvedIndicatorMotion,
    #[case] motion_enabled: bool,
    #[case] source: IndicatorMotionSource,
) {
    let status = IndicatorMotionStatus::resolve(requested, system_animations);
    assert_eq!(
        status,
        IndicatorMotionStatus {
            requested,
            resolved,
            motion_enabled,
            source,
        }
    );
    let alpha_at_start = indicator_frame_alpha(status, Duration::ZERO);
    let alpha_at_quarter_cycle = indicator_frame_alpha(status, Duration::from_millis(450));
    if motion_enabled {
        assert_ne!(alpha_at_start, alpha_at_quarter_cycle);
    } else {
        assert_eq!(alpha_at_start, TARGET_FRAME_ALPHA_MAX);
        assert_eq!(alpha_at_quarter_cycle, TARGET_FRAME_ALPHA_MAX);
    }
}

#[rstest]
fn unavailable_system_preference_fails_closed_without_defeating_an_explicit_override() {
    assert_eq!(
        IndicatorMotionStatus::resolve_from_system(IndicatorMotionPolicy::Auto, None),
        IndicatorMotionStatus {
            requested: IndicatorMotionPolicy::Auto,
            resolved: ResolvedIndicatorMotion::Reduce,
            motion_enabled: false,
            source: IndicatorMotionSource::SafeFallback,
        }
    );
    assert_eq!(
        IndicatorMotionStatus::resolve_from_system(IndicatorMotionPolicy::Animate, None),
        IndicatorMotionStatus {
            requested: IndicatorMotionPolicy::Animate,
            resolved: ResolvedIndicatorMotion::Animate,
            motion_enabled: true,
            source: IndicatorMotionSource::SessionOverride,
        }
    );
}

#[rstest]
fn indicator_motion_evidence_has_a_stable_public_wire_shape() {
    let status = IndicatorMotionStatus::resolve(IndicatorMotionPolicy::Animate, false);

    assert_eq!(
        serde_json::to_value(status).expect("motion evidence serializes"),
        serde_json::json!({
            "requested": "animate",
            "resolved": "animate",
            "motion_enabled": true,
            "source": "session_override"
        })
    );
}

#[rstest]
#[case(7, 7, false)]
#[case(7, 8, true)]
fn interrupt_generation_broadcasts_only_to_existing_sessions(
    #[case] started: u64,
    #[case] current: u64,
    #[case] interrupted: bool,
) {
    assert_eq!(interrupt_generation_changed(started, current), interrupted);
}

#[rstest]
fn programmatic_interrupt_advances_the_shared_generation() {
    let started = interrupt_generation();
    let current = broadcast_interrupt();
    assert!(interrupt_generation_changed(started, current));
}

#[rstest]
#[case("en-US", "agent is controlling Blender")]
#[case("zh-CN", "agent 正在操作 Blender")]
#[case("ja-JP", "agent が Blender を操作中")]
fn control_labels_follow_the_language_tag(#[case] language: &str, #[case] expected: &str) {
    assert_eq!(
        localized_control_label_for_language(language, "agent", "Blender"),
        expected
    );
}

#[rstest]
fn control_labels_strip_control_characters_and_bound_names() {
    let label = localized_control_label_for_language("en", "\tagent\n", " Blender ");
    assert_eq!(label, "agent is controlling Blender");
}

#[cfg(windows)]
#[rstest]
fn a_windowed_target_places_the_banner_above_the_window() {
    let geometry = platform::banner_geometry(
        platform::TargetGeometry {
            x: 200,
            y: 200,
            width: 1200,
            height: 700,
            dpi: 96,
        },
        monitor(),
    );
    assert_eq!(
        (
            geometry.x,
            geometry.y,
            geometry.width,
            geometry.height,
            geometry.inside_target,
        ),
        (560, 148, 480, 44, false)
    );
}

#[rstest]
#[case(0, 132)]
#[case(450, 90)]
#[case(900, 48)]
#[case(1_350, 90)]
#[case(1_800, 132)]
fn target_frame_uses_one_subtle_breathing_cycle(#[case] elapsed_ms: u64, #[case] alpha: u8) {
    assert_eq!(
        breathing_frame_alpha(Duration::from_millis(elapsed_ms)),
        alpha
    );
}

#[rstest]
#[case(0, 132)]
#[case(1, 119)]
#[case(5, 74)]
#[case(10, 33)]
#[case(15, 8)]
#[case(19, 0)]
fn target_frame_fades_inward_across_forty_five_pixels(#[case] band: usize, #[case] alpha: u8) {
    assert_eq!(target_frame_band_alpha(132, band), alpha);
}

#[rstest]
#[case(40)]
#[case(60)]
#[case(80)]
#[case(100)]
#[case(53)]
fn target_frame_bands_cover_the_scaled_gradient_without_gaps(#[case] thickness: i32) {
    let bands = (0..TARGET_FRAME_GRADIENT_STEPS)
        .map(|band| target_frame_band_insets(thickness, band).expect("valid band"))
        .collect::<Vec<_>>();
    assert_eq!(bands.first(), Some(&(0, bands[0].1)));
    assert_eq!(bands.last().map(|band| band.1), Some(thickness));
    for pair in bands.windows(2) {
        assert_eq!(pair[0].1, pair[1].0, "gradient bands must meet exactly");
    }
    assert!(bands.iter().all(|(outer, inner)| outer < inner));
}

#[rstest]
fn target_frame_gradient_is_exactly_forty_five_device_independent_pixels() {
    assert_eq!(TARGET_FRAME_THICKNESS_DIP, 45);
    assert_eq!(TARGET_FRAME_GRADIENT_STEPS, 20);
    assert_eq!(target_frame_band_insets(45, 0), Some((0, 2)));
    assert_eq!(target_frame_band_insets(45, 19), Some((42, 45)));
}

#[cfg(windows)]
#[rstest]
#[case(96, 45)]
#[case(120, 56)]
#[case(144, 68)]
#[case(192, 90)]
fn target_frame_scales_forty_five_dip_from_the_exact_target_monitor(
    #[case] dpi: u32,
    #[case] expected_physical_pixels: i32,
) {
    let geometry = platform::target_frame_geometry(platform::TargetGeometry {
        x: -2_560,
        y: -1_440,
        width: 2_560,
        height: 1_440,
        dpi,
    });

    assert_eq!(geometry.thickness, expected_physical_pixels);
}

#[cfg(windows)]
#[rstest]
#[case(144, 96, 144)]
#[case(192, 96, 192)]
#[case(192, 120, 192)]
fn controlled_probe_dpi_is_authoritative_for_legacy_target_windows(
    #[case] probe_dpi: u32,
    #[case] target_window_dpi: u32,
    #[case] expected: u32,
) {
    assert_eq!(
        platform::resolve_indicator_dpi(Some(probe_dpi), target_window_dpi),
        expected,
    );
}

#[cfg(windows)]
#[rstest]
#[case(None, 144, 144)]
#[case(Some(0), 120, 120)]
#[case(None, 0, 96)]
fn controlled_probe_failure_falls_back_to_the_target_window(
    #[case] probe_dpi: Option<u32>,
    #[case] target_window_dpi: u32,
    #[case] expected: u32,
) {
    assert_eq!(
        platform::resolve_indicator_dpi(probe_dpi, target_window_dpi),
        expected,
    );
}

#[cfg(windows)]
#[rstest]
fn indicator_dpi_resolution_does_not_call_the_legacy_monitor_dpi_api() {
    let forbidden_api = ["Get", "Dpi", "For", "Monitor"].concat();
    assert!(
        !include_str!("platform.rs").contains(&forbidden_api),
        "a PMv2 indicator thread must resolve monitor DPI through its controlled HWND",
    );
}

#[cfg(windows)]
#[rstest]
fn controlled_dpi_probe_is_hidden_ownerless_nonactivating_and_raii_cleaned_up() {
    use windows::Win32::UI::WindowsAndMessaging::{
        GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongPtrW, IsWindow, IsWindowVisible,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    };

    let awareness = platform::ThreadDpiAwareness::enter().expect("enter PMv2 for the DPI probe");
    let probe =
        platform::DpiProbeWindow::create(&awareness).expect("create a hidden PMv2 DPI probe");
    let handle = probe.handle();
    assert!(!unsafe { IsWindowVisible(handle).as_bool() });
    assert!(
        unsafe { GetWindow(handle, GW_OWNER) }
            .map(|owner| owner.0.is_null())
            .unwrap_or(true),
    );
    let ex_style = unsafe { GetWindowLongPtrW(handle, GWL_EXSTYLE) } as u32;
    assert_ne!(ex_style & WS_EX_TOOLWINDOW.0, 0);
    assert_ne!(ex_style & WS_EX_NOACTIVATE.0, 0);
    assert_eq!(ex_style & WS_EX_TOPMOST.0, 0);

    drop(probe);
    assert!(!unsafe { IsWindow(Some(handle)).as_bool() });
}

#[cfg(windows)]
#[rstest]
fn dpi_probe_point_stays_inside_the_selected_negative_coordinate_monitor() {
    use windows::Win32::Foundation::RECT;

    let point = platform::dpi_probe_point(
        RECT {
            left: -2_400,
            top: -1_300,
            right: -640,
            bottom: -100,
        },
        RECT {
            left: -2_560,
            top: -1_440,
            right: 0,
            bottom: 0,
        },
    );

    assert_eq!(point, (-1_520, -700));
}

#[cfg(windows)]
#[rstest]
fn dpi_unaware_target_migration_recomputes_the_physical_gradient_depth() {
    let geometry = |x, probe_dpi| {
        platform::target_frame_geometry(platform::TargetGeometry {
            x,
            y: 80,
            width: 1_600,
            height: 900,
            dpi: platform::resolve_indicator_dpi(Some(probe_dpi), 96),
        })
    };

    let primary = geometry(120, 96);
    let secondary = geometry(-2_560, 144);

    assert_eq!((primary.x, primary.thickness), (120, 45));
    assert_eq!((secondary.x, secondary.thickness), (-2_560, 68));
}

#[cfg(windows)]
#[rstest]
fn target_frame_preserves_negative_secondary_monitor_coordinates() {
    let geometry = platform::target_frame_geometry(platform::TargetGeometry {
        x: -2_560,
        y: -1_440,
        width: 2_560,
        height: 1_440,
        dpi: 144,
    });

    assert_eq!(
        (geometry.x, geometry.y, geometry.width, geometry.height),
        (-2_560, -1_440, 2_560, 1_440)
    );
    assert_eq!(geometry.thickness, 68);
}

#[cfg(windows)]
#[rstest]
#[case(60, 50, 24)]
#[case(24, 24, 11)]
#[case(2, 2, 0)]
fn target_frame_never_consumes_the_entire_small_target(
    #[case] width: i32,
    #[case] height: i32,
    #[case] expected_thickness: i32,
) {
    let geometry = platform::target_frame_geometry(platform::TargetGeometry {
        x: 320,
        y: 240,
        width,
        height,
        dpi: 96,
    });

    assert_eq!(geometry.thickness, expected_thickness);
}

#[rstest]
fn small_target_skips_zero_width_gradient_bands_without_losing_edge_coverage() {
    assert_eq!(visible_target_frame_band(11, 0), None);
    assert_eq!(visible_target_frame_band(11, 1), Some((0, 1)));
    assert_eq!(visible_target_frame_band(11, 19), Some((10, 11)));
    assert_eq!(
        (0..TARGET_FRAME_GRADIENT_STEPS)
            .filter(|band| visible_target_frame_band(11, *band).is_some())
            .count(),
        11
    );
}

#[rstest]
fn target_frame_visibility_reports_whether_any_gradient_band_can_render() {
    assert!(!target_frame_has_visible_band(0));
    assert!(target_frame_has_visible_band(1));
    assert!(target_frame_has_visible_band(11));
}

#[cfg(windows)]
#[rstest]
fn target_frame_re_resolves_position_and_dpi_when_the_target_changes_monitors() {
    let primary = platform::target_frame_geometry(platform::TargetGeometry {
        x: 120,
        y: 80,
        width: 1_600,
        height: 900,
        dpi: 96,
    });
    let secondary = platform::target_frame_geometry(platform::TargetGeometry {
        x: -2_560,
        y: -1_440,
        width: 2_560,
        height: 1_440,
        dpi: 144,
    });

    assert_eq!((primary.x, primary.y, primary.thickness), (120, 80, 45));
    assert_eq!(
        (secondary.x, secondary.y, secondary.thickness),
        (-2_560, -1_440, 68)
    );
    assert!(primary != secondary);
}

#[cfg(windows)]
#[rstest]
fn banner_stays_on_a_negative_coordinate_secondary_monitor() {
    let geometry = platform::banner_geometry(
        platform::TargetGeometry {
            x: -2_400,
            y: -1_300,
            width: 1_600,
            height: 900,
            dpi: 144,
        },
        platform::MonitorGeometry {
            left: -2_560,
            top: -1_440,
            right: 0,
            bottom: 0,
            work_left: -2_560,
            work_top: -1_440,
            work_right: 0,
            work_bottom: -60,
        },
    );

    assert!(geometry.x < 0);
    assert!(geometry.y < 0);
    assert!(geometry.x >= -2_560);
    assert!(geometry.x + geometry.width <= 0);
}

#[cfg(windows)]
#[rstest]
fn maximized_target_uses_the_visible_dwm_frame_instead_of_invisible_resize_borders() {
    use windows::Win32::Foundation::RECT;

    let visible = platform::visible_target_rect(
        RECT {
            left: -8,
            top: -8,
            right: 1_928,
            bottom: 1_088,
        },
        Some(RECT {
            left: 0,
            top: 0,
            right: 1_920,
            bottom: 1_080,
        }),
    );

    assert_eq!(
        (visible.left, visible.top, visible.right, visible.bottom),
        (0, 0, 1_920, 1_080)
    );
}

#[cfg(windows)]
#[rstest]
fn invalid_dwm_frame_falls_back_to_the_window_rect() {
    use windows::Win32::Foundation::RECT;

    let fallback = RECT {
        left: -2_560,
        top: 80,
        right: -640,
        bottom: 1_160,
    };
    let visible = platform::visible_target_rect(
        fallback,
        Some(RECT {
            left: 10,
            top: 10,
            right: 10,
            bottom: 10,
        }),
    );

    assert_eq!(
        (visible.left, visible.top, visible.right, visible.bottom),
        (fallback.left, fallback.top, fallback.right, fallback.bottom)
    );
}

#[rstest]
fn target_frame_alpha_is_monotonic_from_edge_to_center() {
    let alphas = (0..TARGET_FRAME_GRADIENT_STEPS)
        .map(|band| target_frame_band_alpha(TARGET_FRAME_ALPHA_MAX, band))
        .collect::<Vec<_>>();
    assert!(alphas.windows(2).all(|pair| pair[0] >= pair[1]));
    assert_eq!(alphas[0], TARGET_FRAME_ALPHA_MAX);
    assert_eq!(alphas.last(), Some(&0));
}

#[cfg(windows)]
#[rstest]
fn a_fullscreen_target_places_the_banner_inside_the_safe_inset() {
    let geometry = platform::banner_geometry(
        platform::TargetGeometry {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            dpi: 96,
        },
        monitor(),
    );
    assert_eq!(
        (
            geometry.x,
            geometry.y,
            geometry.width,
            geometry.height,
            geometry.inside_target,
        ),
        (720, 16, 480, 44, true)
    );
}

#[cfg(windows)]
fn monitor() -> platform::MonitorGeometry {
    platform::MonitorGeometry {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
        work_left: 0,
        work_top: 0,
        work_right: 1920,
        work_bottom: 1040,
    }
}

#[cfg(windows)]
#[rstest]
fn escape_hook_recognizes_key_transitions_without_exclusive_registration() {
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
    use windows::Win32::UI::WindowsAndMessaging::{
        HC_ACTION, LLKHF_INJECTED, WM_KEYDOWN, WM_KEYUP,
    };

    assert_eq!(
        platform::escape_key_transition(HC_ACTION as i32, WM_KEYDOWN, VK_ESCAPE.0 as u32, 0),
        Some(true)
    );
    assert_eq!(
        platform::escape_key_transition(HC_ACTION as i32, WM_KEYUP, VK_ESCAPE.0 as u32, 0),
        Some(false)
    );
    assert_eq!(
        platform::escape_key_transition(HC_ACTION as i32, WM_KEYDOWN, b'A'.into(), 0),
        None
    );
    assert_eq!(
        platform::escape_key_transition(
            HC_ACTION as i32,
            WM_KEYDOWN,
            VK_ESCAPE.0 as u32,
            LLKHF_INJECTED.0,
        ),
        None,
        "agent-injected Escape must reach the granted target instead of stopping the Host",
    );
}

#[cfg(windows)]
#[rstest]
fn escape_hook_is_a_complete_passthrough_without_active_banners() {
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
    use windows::Win32::UI::WindowsAndMessaging::{HC_ACTION, WM_KEYDOWN, WM_KEYUP};

    assert_eq!(
        platform::escape_key_transition_for_active_banners(
            0,
            HC_ACTION as i32,
            WM_KEYDOWN,
            VK_ESCAPE.0 as u32,
            0,
        ),
        None,
    );
    assert_eq!(
        platform::escape_key_transition_for_active_banners(
            0,
            HC_ACTION as i32,
            WM_KEYUP,
            VK_ESCAPE.0 as u32,
            0,
        ),
        None,
    );
    assert_eq!(
        platform::escape_key_transition_for_active_banners(
            1,
            HC_ACTION as i32,
            WM_KEYDOWN,
            VK_ESCAPE.0 as u32,
            0,
        ),
        Some(true),
    );
}

#[cfg(windows)]
#[rstest]
fn escape_hub_lifecycle_restarts_dead_hooks_and_stops_after_the_last_banner() {
    use platform::{EscapeHubAcquireAction, EscapeHubReleaseAction};

    assert_eq!(
        platform::escape_hub_acquire_action(false, false),
        EscapeHubAcquireAction::Start
    );
    assert_eq!(
        platform::escape_hub_acquire_action(true, true),
        EscapeHubAcquireAction::Reuse
    );
    assert_eq!(
        platform::escape_hub_acquire_action(true, false),
        EscapeHubAcquireAction::Restart
    );
    assert_eq!(
        platform::escape_hub_release_action(2),
        EscapeHubReleaseAction::KeepRunning
    );
    assert_eq!(
        platform::escape_hub_release_action(1),
        EscapeHubReleaseAction::Stop
    );
}
