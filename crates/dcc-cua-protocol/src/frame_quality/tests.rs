use super::*;
use rstest::rstest;

#[rstest]
#[case([0, 0, 0, 0], [0, 0, 0])]
#[case([255, 255, 255, 255], [255_000, 255_000, 255_000])]
#[case([0, 0, 255, 127], [255_000, 0, 0])]
#[case([37, 111, 249, 0], [249_000, 111_000, 37_000])]
fn every_uniform_color_is_blank(#[case] bgra: [u8; 4], #[case] mean: [u32; 3]) {
    let assessment = assess_bgra(CaptureMode::InProcessQt, &bgra.repeat(6), 3, 2).unwrap();
    let FrameAssessment::BlankCapture {
        attempted_mode,
        statistics,
    } = assessment
    else {
        panic!("uniform image was published as informative")
    };
    assert_eq!(attempted_mode, CaptureMode::InProcessQt);
    assert_eq!(statistics.pixel_count, 6);
    assert_eq!(statistics.mean_rgb_milli, mean);
    assert_eq!(statistics.stddev_rgb_milli, [0; 3]);
    assert_eq!(statistics.entropy_rgb_millibits, [0; 3]);
}

#[rstest]
fn high_byte_noise_does_not_hide_blank_rgb() {
    assert!(matches!(
        assess_bgra(
            CaptureMode::WindowContent,
            &[9, 9, 9, 0, 9, 9, 9, 255],
            2,
            1
        ),
        Ok(FrameAssessment::BlankCapture { .. })
    ));
}

#[rstest]
fn two_tone_frame_has_known_population_statistics() {
    let result = assess_bgra(
        CaptureMode::VisibleDesktopCrop,
        &[0, 0, 0, 0, 255, 255, 255, 255],
        2,
        1,
    )
    .unwrap();
    let FrameAssessment::Informative {
        attempted_mode,
        statistics,
    } = result
    else {
        panic!("two-tone frame was rejected")
    };
    assert_eq!(attempted_mode, CaptureMode::VisibleDesktopCrop);
    assert_eq!(statistics.mean_rgb_milli, [127_500; 3]);
    assert_eq!(statistics.stddev_rgb_milli, [127_500; 3]);
    assert_eq!(statistics.entropy_rgb_millibits, [1000; 3]);
}

#[rstest]
fn a_change_in_one_rgb_channel_is_informative() {
    let result = assess_bgra(
        CaptureMode::RedactedDesktopCrop,
        &[0, 0, 0, 255, 1, 0, 0, 255],
        2,
        1,
    )
    .unwrap();
    let FrameAssessment::Informative { statistics, .. } = result else {
        panic!("nonuniform blue channel was ignored")
    };
    assert_eq!(statistics.mean_rgb_milli, [0, 0, 500]);
    assert_eq!(statistics.stddev_rgb_milli, [0, 0, 500]);
    assert_eq!(statistics.entropy_rgb_millibits, [0, 0, 1000]);
}

#[rstest]
#[case(0, 1, &[], FrameDataError::Empty)]
#[case(1, 0, &[], FrameDataError::Empty)]
#[case(1, 1, &[0, 0, 0], FrameDataError::InvalidLength)]
#[case(1, 1, &[0, 0, 0, 0, 0], FrameDataError::InvalidLength)]
#[case(u32::MAX, u32::MAX, &[], FrameDataError::TooLarge)]
#[case(4097, 4096, &[], FrameDataError::TooLarge)]
fn invalid_buffers_are_never_assessed_as_frames(
    #[case] width: u32,
    #[case] height: u32,
    #[case] bytes: &[u8],
    #[case] expected: FrameDataError,
) {
    assert_eq!(
        assess_bgra(CaptureMode::InProcessQt, bytes, width, height),
        Err(expected)
    );
}

#[rstest]
fn all_gray_levels_have_eight_bits_of_entropy() {
    let bytes: Vec<u8> = (0..=255).flat_map(|v| [v, v, v, 0]).collect();
    let FrameAssessment::Informative { statistics, .. } =
        assess_bgra(CaptureMode::WindowContent, &bytes, 256, 1).unwrap()
    else {
        panic!("gradient was rejected")
    };
    assert_eq!(statistics.entropy_rgb_millibits, [8000; 3]);
    assert_eq!(statistics.mean_rgb_milli, [127_500; 3]);
}

#[rstest]
fn blank_wire_outcome_retains_mode_and_numeric_evidence_only() {
    let outcome = assess_bgra(CaptureMode::InProcessQt, &[255; 4], 1, 1).unwrap();
    let value = serde_json::to_value(&outcome).unwrap();
    assert_eq!(value["status"], "blank_capture");
    assert_eq!(value["attempted_mode"], "in_process_qt");
    assert_eq!(value["statistics"]["uniform_rgb"], true);
    assert_eq!(value.as_object().unwrap().len(), 3);
    assert_eq!(
        serde_json::from_value::<FrameAssessment>(value).unwrap(),
        outcome
    );
}
