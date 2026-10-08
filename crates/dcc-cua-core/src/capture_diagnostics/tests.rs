use rstest::rstest;

use super::*;

#[rstest]
fn diagnostics_describe_raw_bytes_and_decoded_png_without_alpha_normalization() {
    let bgra = [3, 2, 1, 141, 83, 42, 7, 0, 17, 122, 138, 255];
    let encoded = encode_exact_frame(&bgra, 3, 1, true, Duration::from_micros(17)).unwrap();
    let diagnostics = encoded.diagnostics.unwrap();
    let decoder = png::Decoder::new(encoded.data.as_slice());
    let mut reader = decoder.read_info().unwrap();
    let mut rgba = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut rgba).unwrap();
    rgba.truncate(info.buffer_size());
    assert_eq!(rgba, [1, 2, 3, 141, 7, 42, 83, 0, 138, 122, 17, 255]);
    assert_eq!(
        diagnostics.raw_bgra_sha256,
        format!("{:x}", Sha256::digest(bgra))
    );
    assert_eq!(
        diagnostics.canonical_rgba_sha256,
        format!("{:x}", Sha256::digest(&rgba))
    );
    assert_ne!(
        diagnostics.raw_bgra_sha256,
        diagnostics.canonical_rgba_sha256
    );
    assert_eq!(diagnostics.capture_to_raw_elapsed_micros, 17);
    let serialized = serde_json::to_value(&diagnostics).unwrap();
    assert_eq!(serialized["capture_to_raw_elapsed_micros"], 17);
    assert!(serialized.get("raw_readback_elapsed_micros").is_none());
    for (channel, histogram) in diagnostics.bgra_channel_histograms.iter().enumerate() {
        assert_eq!(histogram.len(), 256);
        assert_eq!(histogram.iter().sum::<u64>(), 3);
        for pixel in bgra.as_chunks::<4>().0 {
            assert_eq!(histogram[usize::from(pixel[channel])], 1);
        }
    }
}

#[rstest]
fn opt_in_does_not_change_png_bytes_and_default_omits_summaries() {
    let bgra = [83, 42, 7, 255, 17, 122, 138, 255];
    let default = encode_exact_frame(&bgra, 2, 1, false, Duration::ZERO).unwrap();
    let diagnostic = encode_exact_frame(&bgra, 2, 1, true, Duration::ZERO).unwrap();
    assert!(default.diagnostics.is_none());
    assert_eq!(default.data, diagnostic.data);
}

#[rstest]
fn incomplete_or_empty_frames_fail_without_a_diagnostic_summary() {
    for (data, width, height) in [
        (&[1_u8, 2, 3][..], 1, 1),
        (&[][..], 0, 1),
        (&[][..], u32::MAX, u32::MAX),
    ] {
        assert!(encode_exact_frame(data, width, height, true, Duration::ZERO).is_err());
    }
}
