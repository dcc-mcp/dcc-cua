use rstest::rstest;

use super::*;

#[cfg(windows)]
#[test]
fn native_overlays_are_hit_test_transparent_and_never_activate() {
    assert_eq!(platform::overlay_input_result(0x0084).unwrap().0, -1);
    assert_eq!(platform::overlay_input_result(0x0021).unwrap().0, 3);
    assert!(platform::overlay_input_result(0x000f).is_none());
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
    let geometry = platform::geometry_for_test(
        platform::TargetGeometry {
            x: 200,
            y: 200,
            width: 1200,
            height: 700,
            dpi: 96,
        },
        monitor(),
    );
    assert_eq!(geometry, (560, 148, 480, 44, false));
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
fn target_frame_fades_inward_across_forty_pixels(#[case] band: usize, #[case] alpha: u8) {
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
fn target_frame_gradient_is_exactly_forty_device_independent_pixels() {
    assert_eq!(TARGET_FRAME_THICKNESS_DIP, 40);
    assert_eq!(TARGET_FRAME_GRADIENT_STEPS, 20);
    assert_eq!(target_frame_band_insets(40, 0), Some((0, 2)));
    assert_eq!(target_frame_band_insets(40, 19), Some((38, 40)));
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
    let geometry = platform::geometry_for_test(
        platform::TargetGeometry {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            dpi: 96,
        },
        monitor(),
    );
    assert_eq!(geometry, (720, 16, 480, 44, true));
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
    use windows::Win32::UI::WindowsAndMessaging::{HC_ACTION, WM_KEYDOWN, WM_KEYUP};

    assert_eq!(
        platform::escape_key_transition(HC_ACTION as i32, WM_KEYDOWN, VK_ESCAPE.0 as u32),
        Some(true)
    );
    assert_eq!(
        platform::escape_key_transition(HC_ACTION as i32, WM_KEYUP, VK_ESCAPE.0 as u32),
        Some(false)
    );
    assert_eq!(
        platform::escape_key_transition(HC_ACTION as i32, WM_KEYDOWN, b'A'.into()),
        None
    );
}
