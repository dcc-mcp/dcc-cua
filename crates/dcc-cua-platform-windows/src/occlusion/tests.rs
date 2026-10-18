use super::*;
use rstest::rstest;

#[rstest]
fn partial_overlay_redacts_foreign_pixels_without_changing_target_coordinates() {
    let bounds = [-2, -1, 4, 3];
    let mut pixels = vec![37; 4 * 3 * 4];
    assert!(redact_bgra(&mut pixels, bounds, &[[0, -2, 3, 2]]).unwrap());
    for y in 0..3 {
        for x in 0..4 {
            let pixel = &pixels[(y * 4 + x) * 4..(y * 4 + x + 1) * 4];
            assert_eq!(
                pixel,
                if y == 0 && x >= 2 {
                    &[0, 0, 0, 255]
                } else {
                    &[37; 4]
                }
            );
        }
    }
    assert!(point_visible((-1, 0), bounds, &[[0, -2, 3, 2]]));
    assert!(!point_visible((0, -1), bounds, &[[0, -2, 3, 2]]));
    assert!(!point_visible((2, 0), bounds, &[]));
}

#[rstest]
fn complete_union_is_not_mistaken_for_observable_target_pixels() {
    let mut pixels = vec![9; 4 * 3 * 4];
    assert!(!redact_bgra(&mut pixels, [0, 0, 4, 3], &[[0, 0, 3, 3], [2, 0, 2, 3]]).unwrap());
    assert!(pixels.chunks_exact(4).all(|p| p == [0, 0, 0, 255]));
}

#[rstest]
fn unobscured_pixels_are_preserved_and_invalid_geometry_is_refused() {
    let mut pixels = vec![9; 16];
    assert!(redact_bgra(&mut pixels, [0, 0, 2, 2], &[]).unwrap());
    assert_eq!(pixels, vec![9; 16]);
    assert!(redact_bgra(&mut pixels, [0, 0, 3, 2], &[]).is_err());
    assert!(redact_bgra(&mut pixels, [0, 0, 2, 2], &[[i32::MAX, 0, 2, 2]]).is_err());
}
