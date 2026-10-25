use super::*;
use rstest::rstest;

#[rstest]
fn silent_null_is_never_dereferenced() {
    // SAFETY: SILENT explicitly permits null data; no OS source is started.
    let copied = unsafe {
        copy_packet(
            std::ptr::null(),
            2,
            AUDCLNT_BUFFERFLAGS_SILENT.0 as u32,
            1,
            0,
            None,
        )
    }
    .unwrap();
    assert_eq!(copied, vec![0; 4]);
}

#[rstest]
#[case(0, 0, 1, 0, None)]
#[case(2401, 0, 1, 0, None)]
#[case(1, 0, 0, 0, None)]
#[case(1, 4, 1, 0, None)]
#[case(1, 8, 1, 0, None)]
#[case(1, 1, 1, 2, Some(2))]
#[case(1, 0, 1, 3, Some(2))]
fn invalid_packet_is_rejected_before_pointer_access(
    #[case] frames: u32,
    #[case] flags: u32,
    #[case] timestamp: u64,
    #[case] position: u64,
    #[case] previous: Option<u64>,
) {
    // SAFETY: invalid metadata causes rejection before any pointer access.
    assert!(
        unsafe {
            copy_packet(
                std::ptr::null(),
                frames,
                flags,
                timestamp,
                position,
                previous,
            )
        }
        .is_err()
    );
}

#[rstest]
fn first_discontinuity_preserves_pcm_but_later_gap_fails() {
    let data = [123_i16, -123];
    // SAFETY: owned two-sample buffer matches one stereo frame.
    assert_eq!(
        unsafe { copy_packet(data.as_ptr(), 1, 1, 1, 0, None) }.unwrap(),
        data
    );
    // SAFETY: owned buffer remains alive; discontinuity is refused before copying.
    assert!(unsafe { copy_packet(data.as_ptr(), 1, 1, 2, 1, Some(1)) }.is_err());
}
