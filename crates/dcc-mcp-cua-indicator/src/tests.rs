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
