use rstest::rstest;

use super::*;

#[rstest]
#[case(0, 1, "DCC UI Control")]
#[case(1, 0, "DCC UI Control")]
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
fn cursor_uses_the_smaller_smooth_outer_to_inner_gradient() {
    assert_eq!(platform::CURSOR_SIZE, 52);
    assert_eq!(platform::FRAME_LAYER_MAX_ALPHA.len(), 8);
}

#[cfg(windows)]
#[rstest]
#[case(7, 7, false)]
#[case(7, 8, true)]
fn escape_generation_broadcasts_only_to_existing_sessions(
    #[case] started: u64,
    #[case] current: u64,
    #[case] interrupted: bool,
) {
    assert_eq!(
        platform::escape_generation_changed(started, current),
        interrupted
    );
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
