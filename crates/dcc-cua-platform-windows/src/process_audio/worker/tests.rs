use super::*;
use rstest::rstest;

#[rstest]
#[case(START, 9, false, Ok(true))]
#[case(START, 10, true, Ok(false))]
#[case(START, 11, false, Err(Failure::AuthorizationExpired))]
#[case(INVALID, 11, true, Err(Failure::ProtocolFailed))]
#[case(STOP, 11, true, Ok(false))]
fn deadline_stop_never_extends_sampling_or_masks_a_protocol_fault(
    #[case] state: u8,
    #[case] now: u64,
    #[case] duration_limited: bool,
    #[case] expected: Result<bool, Failure>,
) {
    assert_eq!(check_state_at(state, now, 10, duration_limited), expected);
}

#[rstest]
fn silent_null_is_never_dereferenced() {
    // SAFETY: SILENT explicitly permits null data; no OS source is started.
    let copied = unsafe {
        copy_packet(
            std::ptr::null(),
            2,
            AUDCLNT_BUFFERFLAGS_SILENT.0 as u32,
            1,
            None,
        )
    }
    .unwrap();
    assert_eq!(copied, vec![0; 4]);
}

#[rstest]
#[case(0, 0, 1)]
#[case(2401, 0, 1)]
#[case(1, 0, 0)]
#[case(1, 4, 1)]
#[case(1, 8, 1)]
fn invalid_packet_is_rejected_before_pointer_access(
    #[case] frames: u32,
    #[case] flags: u32,
    #[case] timestamp: u64,
) {
    // SAFETY: invalid metadata rejects before pointer access.
    assert!(unsafe { copy_packet(std::ptr::null(), frames, flags, timestamp, None) }.is_err());
}

#[rstest]
fn first_discontinuity_preserves_pcm_but_later_gap_fails() {
    let data = [123_i16, -123];
    // SAFETY: owned two-sample buffer matches one stereo frame.
    assert_eq!(
        unsafe { copy_packet(data.as_ptr(), 1, 1, 1, None) }.unwrap(),
        data
    );
    // SAFETY: later discontinuity is refused before copying.
    let previous = AudioWorkerPacketClock {
        qpc_100ns: 1,
        frames: 1,
    };
    assert!(unsafe { copy_packet(data.as_ptr(), 1, 1, 209, Some(previous)) }.is_err());
}

#[rstest]
#[case(100_000, true)]
#[case(100_208, true)]
#[case(99_792, true)]
#[case(100_209, true)]
#[case(99_791, false)]
#[case(200_000, true)]
#[case(0, false)]
fn qpc_order_preserves_forward_gaps_but_rejects_overlap_and_rollback(
    #[case] delta: u64,
    #[case] accepted: bool,
) {
    let previous = AudioWorkerPacketClock {
        qpc_100ns: 1_000_000,
        frames: 480,
    };
    assert_eq!(
        qpc_packet_has_valid_order(previous, previous.qpc_100ns + delta),
        accepted
    );
    assert!(!qpc_packet_has_valid_order(
        previous,
        previous.qpc_100ns - 1
    ));
}

#[rstest]
fn rejected_packet_retains_exact_metadata_without_changing_continuity_policy() {
    let packet = AudioWorkerPacketMetadata {
        frames: 480,
        flags: 0,
        qpc_100ns: 1_050_000,
        device_position: 0,
        expected_device_position: Some(480),
        previous_clock: Some(AudioWorkerPacketClock {
            qpc_100ns: 1_000_000,
            frames: 480,
        }),
        null_data: true,
    };
    let mut diagnostic = AudioWorkerFailureDiagnostic::default();
    // SAFETY: QPC overlap rejects before pointer access.
    let result =
        unsafe { copy_packet_with_diagnostic(std::ptr::null(), packet.clone(), &mut diagnostic) };
    assert_eq!(result.unwrap_err(), Failure::ContinuityLost);
    assert_eq!(
        diagnostic.stage,
        Some(AudioWorkerFailureStage::PacketValidation)
    );
    assert_eq!(diagnostic.packet, Some(packet));
}

#[rstest]
fn zero_device_positions_with_exact_qpc_step_do_not_prove_a_packet_gap() {
    let data = [0_i16; 960];
    let previous = AudioWorkerPacketClock {
        qpc_100ns: 3746414039782,
        frames: 480,
    };
    let mut diagnostic = AudioWorkerFailureDiagnostic::default();
    let packet = AudioWorkerPacketMetadata {
        frames: 480,
        flags: 0,
        qpc_100ns: 3746414139782,
        device_position: 0,
        expected_device_position: Some(480),
        previous_clock: Some(previous),
        null_data: false,
    };
    // SAFETY: exact owned 480-frame stereo buffer; reproduces native metadata.
    let second = unsafe { copy_packet_with_diagnostic(data.as_ptr(), packet, &mut diagnostic) };
    assert!(
        second.is_ok(),
        "device counter0 is not a gap when QPC advances exactly480frames"
    );
    assert!(diagnostic.stage.is_none());
}
