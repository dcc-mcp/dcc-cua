use rstest::rstest;

use super::*;

#[rstest]
fn diagnostics_describe_raw_bytes_and_decoded_png_without_alpha_normalization() {
    let bgra = [3, 2, 1, 141, 83, 42, 7, 0, 17, 122, 138, 255];
    let encoded = prepare_exact_frame(
        &bgra,
        3,
        1,
        true,
        Duration::from_micros(17),
        FramePurpose::Publication,
    )
    .unwrap();
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
    assert!(diagnostics.png_encoded);
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
    let default = prepare_exact_frame(
        &bgra,
        2,
        1,
        false,
        Duration::ZERO,
        FramePurpose::Publication,
    )
    .unwrap();
    let diagnostic =
        prepare_exact_frame(&bgra, 2, 1, true, Duration::ZERO, FramePurpose::Publication).unwrap();
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
        for purpose in [FramePurpose::Validation, FramePurpose::Publication] {
            for diagnostics in [false, true] {
                assert!(
                    prepare_exact_frame(data, width, height, diagnostics, Duration::ZERO, purpose)
                        .is_err()
                );
            }
        }
    }
}

#[rstest]
#[case(false)]
#[case(true)]
fn validation_frame_retains_raw_evidence_without_encoding_an_unused_png(#[case] diagnostics: bool) {
    let bgra = [83, 42, 7, 255, 17, 122, 138, 0];
    let first = prepare_exact_frame(
        &bgra,
        2,
        1,
        diagnostics,
        Duration::ZERO,
        FramePurpose::Validation,
    )
    .unwrap();
    let last = prepare_exact_frame(
        &bgra,
        2,
        1,
        diagnostics,
        Duration::ZERO,
        FramePurpose::Publication,
    )
    .unwrap();
    assert!(first.data.is_empty());
    assert_eq!(last.data, encode_bgra_to_png(&bgra, 2, 1).unwrap());
    if diagnostics {
        let first = first.diagnostics.unwrap();
        let last = last.diagnostics.unwrap();
        assert!(!first.png_encoded);
        assert_eq!(first.png_encode_elapsed_micros, 0);
        assert_eq!(first.raw_bgra_sha256, last.raw_bgra_sha256);
        assert_eq!(first.canonical_rgba_sha256, last.canonical_rgba_sha256);
        assert_eq!(first.bgra_channel_histograms, last.bgra_channel_histograms);
    } else {
        assert!(first.diagnostics.is_none());
    }
}

#[rstest]
#[ignore = "manual CPU encoding benchmark; does not measure native capture or transport"]
fn snapshot_encoding_latency() {
    for (width, height) in [(1280_u32, 720_u32), (3120, 1984)] {
        let mut seed = 0x6d2b79f5_u32;
        let bgra: Vec<u8> = (0..width as usize * height as usize * 4)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        let mut samples = Vec::new();
        for _ in 0..8 {
            let started = Instant::now();
            let first = prepare_exact_frame(
                &bgra,
                width,
                height,
                false,
                Duration::ZERO,
                FramePurpose::Validation,
            )
            .unwrap();
            let last = prepare_exact_frame(
                &bgra,
                width,
                height,
                false,
                Duration::ZERO,
                FramePurpose::Publication,
            )
            .unwrap();
            std::hint::black_box((&first, &last));
            samples.push(started.elapsed().as_micros());
        }
        samples.sort_unstable();
        println!(
            "snapshot_encoding width={width} height={height} samples=8 p50_us={} p95_us={}",
            samples[4], samples[7]
        );
    }
}
