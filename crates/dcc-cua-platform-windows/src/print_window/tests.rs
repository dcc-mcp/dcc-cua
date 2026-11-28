use super::pixel_byte_len;
use rstest::rstest;

#[rstest]
fn print_window_pixel_size_is_bounded() {
    assert_eq!(pixel_byte_len(2, 3).expect("valid size"), 24);
    assert!(pixel_byte_len(0, 3).is_err());
    assert!(pixel_byte_len(8_193, 8_193).is_err());
}
