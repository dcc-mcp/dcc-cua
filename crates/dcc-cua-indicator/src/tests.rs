use rstest::rstest;

use super::*;

#[rstest]
#[case(0, 1, "CUA Control")]
#[case(1, 0, "CUA Control")]
#[case(1, 1, "")]
fn banner_target_requires_exact_bounded_identity(
    #[case] process_id: u32,
    #[case] window_handle: u64,
    #[case] label: &str,
) {
    let error = BannerTarget {
        process_id,
        window_handle,
        label: label.into(),
    }
    .validate()
    .expect_err("invalid banner target must be rejected");
    assert!(matches!(error, IndicatorError::InvalidTarget(_)));
}

#[cfg(windows)]
#[rstest]
#[case(0, 244)]
#[case(450, 188)]
#[case(900, 132)]
#[case(1_350, 188)]
#[case(1_800, 244)]
fn target_frame_uses_a_smooth_breathing_cycle(#[case] elapsed_ms: u64, #[case] alpha: u8) {
    assert_eq!(
        platform::breathing_frame_alpha(std::time::Duration::from_millis(elapsed_ms)),
        alpha
    );
}

#[cfg(windows)]
#[rstest]
#[case(48, 244, 48)]
#[case(244, 244, 244)]
#[case(48, 132, 26)]
#[case(244, 132, 132)]
fn target_frame_preserves_transparent_gradient_while_breathing(
    #[case] maximum: u8,
    #[case] breathing: u8,
    #[case] alpha: u8,
) {
    assert_eq!(platform::gradient_frame_alpha(maximum, breathing), alpha);
}

#[cfg(windows)]
#[rstest]
fn target_frame_fades_from_the_window_edge_toward_transparent_content() {
    let layers = platform::FRAME_LAYER_MAX_ALPHA;
    assert_eq!(layers[0], 210);
    assert_eq!(layers[layers.len() - 1], 4);
    assert!(layers.windows(2).all(|pair| pair[0] > pair[1]));
}

#[cfg(windows)]
#[rstest]
fn cursor_uses_a_small_black_pointer_over_a_larger_hollow_halo() {
    assert_eq!(platform::CURSOR_POINTER_SIZE, 24);
    assert_eq!(platform::CURSOR_HALO_SIZE, 68);
    assert_eq!(platform::CURSOR_POINTER_COLOR.0, 0);
    assert_eq!(platform::CURSOR_HALO_COLOR.0, 0x0092_CF9A);
}

#[cfg(windows)]
#[rstest]
fn cursor_halo_is_transparent_in_the_center_and_diffuses_outward() {
    let alpha = platform::CURSOR_HALO_LAYER_ALPHA;
    assert_eq!(alpha.len(), 8);
    assert!(alpha.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(platform::cursor_halo_outer_inset(68, 0), 0);
    assert_eq!(platform::cursor_halo_outer_inset(68, 7), 19);
    assert_eq!(platform::cursor_halo_inner_inset(68, 0), 31);
    assert_eq!(platform::cursor_halo_inner_inset(68, 7), 23);
    assert!((0..8).all(|layer| {
        platform::cursor_halo_outer_inset(68, layer) < platform::cursor_halo_inner_inset(68, layer)
    }));
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
fn session_colors_are_stable_and_avoid_the_default_banner_hue() {
    let first = session_color("agent", "session-1");
    assert_eq!(first, session_color("agent", "session-1"));
    assert!(first.hue().abs_diff(BannerColor::DEFAULT.hue()) >= 45);
    assert_ne!(first, session_color("agent", "session-2"));
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
fn an_existing_host_hotkey_uses_the_shared_escape_event() {
    let error = windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(
        windows::Win32::Foundation::ERROR_HOTKEY_ALREADY_REGISTERED.0,
    ));
    assert!(platform::hotkey_already_registered(&error));
}

#[cfg(windows)]
#[rstest]
#[case(false, true, 64, 64, true)]
#[case(true, true, 64, 64, false)]
#[case(true, true, 64, 80, true)]
fn cursor_shape_is_initialized_when_the_hidden_marker_first_appears(
    #[case] was_visible: bool,
    #[case] is_visible: bool,
    #[case] previous_size: i32,
    #[case] current_size: i32,
    #[case] update: bool,
) {
    assert_eq!(
        platform::cursor_shape_needs_update(was_visible, is_visible, previous_size, current_size),
        update
    );
}
