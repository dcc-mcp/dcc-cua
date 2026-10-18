//! Physical desktop rectangles shared by redacted capture and pointer visibility.

fn valid(rect: [i32; 4]) -> bool {
    rect[2] > 0
        && rect[3] > 0
        && rect[0].checked_add(rect[2]).is_some()
        && rect[1].checked_add(rect[3]).is_some()
}

pub(crate) fn intersection(a: [i32; 4], b: [i32; 4]) -> Option<[i32; 4]> {
    if !valid(a) || !valid(b) {
        return None;
    }
    let left = a[0].max(b[0]);
    let top = a[1].max(b[1]);
    let right = (a[0] + a[2]).min(b[0] + b[2]);
    let bottom = (a[1] + a[3]).min(b[1] + b[3]);
    (right > left && bottom > top).then(|| [left, top, right - left, bottom - top])
}

pub(crate) fn point_visible(point: (i32, i32), bounds: [i32; 4], redactions: &[[i32; 4]]) -> bool {
    let contains = |r: [i32; 4]| {
        valid(r)
            && point.0 >= r[0]
            && point.1 >= r[1]
            && point.0 < r[0] + r[2]
            && point.1 < r[1] + r[3]
    };
    contains(bounds) && redactions.iter().all(|r| valid(*r) && !contains(*r))
}

/// Returns whether any target pixels remain. Covered pixels are never published.
pub(crate) fn redact_bgra(
    pixels: &mut [u8],
    bounds: [i32; 4],
    redactions: &[[i32; 4]],
) -> Result<bool, &'static str> {
    if !valid(bounds) || redactions.iter().any(|r| !valid(*r)) {
        return Err("invalid redaction rectangle");
    }
    let width = bounds[2] as usize;
    let height = bounds[3] as usize;
    if width.checked_mul(height).and_then(|n| n.checked_mul(4)) != Some(pixels.len()) {
        return Err("redaction buffer does not match its physical bounds");
    }
    let mut visible = false;
    for y in 0..height {
        let physical_y = bounds[1] + y as i32;
        let mut spans: Vec<_> = redactions
            .iter()
            .filter_map(|r| intersection(bounds, *r))
            .filter(|r| physical_y >= r[1] && physical_y < r[1] + r[3])
            .map(|r| {
                (
                    (r[0] - bounds[0]) as usize,
                    (r[0] - bounds[0] + r[2]) as usize,
                )
            })
            .collect();
        spans.sort_unstable();
        let mut covered = 0;
        let mut end = 0;
        for (start, next) in spans {
            let start = start.max(end);
            if next > start {
                for pixel in
                    pixels[(y * width + start) * 4..(y * width + next) * 4].chunks_exact_mut(4)
                {
                    pixel.copy_from_slice(&[0, 0, 0, 255]);
                }
                covered += next - start;
                end = next;
            }
        }
        visible |= covered < width;
    }
    Ok(visible)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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

    #[test]
    fn complete_union_is_not_mistaken_for_observable_target_pixels() {
        let mut pixels = vec![9; 4 * 3 * 4];
        assert!(!redact_bgra(&mut pixels, [0, 0, 4, 3], &[[0, 0, 3, 3], [2, 0, 2, 3]]).unwrap());
        assert!(pixels.chunks_exact(4).all(|p| p == [0, 0, 0, 255]));
    }

    #[test]
    fn unobscured_pixels_are_preserved_and_invalid_geometry_is_refused() {
        let mut pixels = vec![9; 16];
        assert!(redact_bgra(&mut pixels, [0, 0, 2, 2], &[]).unwrap());
        assert_eq!(pixels, vec![9; 16]);
        assert!(redact_bgra(&mut pixels, [0, 0, 3, 2], &[]).is_err());
        assert!(redact_bgra(&mut pixels, [0, 0, 2, 2], &[[i32::MAX, 0, 2, 2]]).is_err());
    }
}
