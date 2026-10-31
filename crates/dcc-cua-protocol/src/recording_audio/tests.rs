use super::*;
use rstest::rstest;

fn target() -> ProcessAudioTarget {
    ProcessAudioTarget {
        process_id: 42,
        process_creation_time_100ns: 123,
        window_handle: 7,
        window_thread_id: 9,
        window_class_hash: 11,
    }
}

fn grant() -> ProcessAudioGrant {
    ProcessAudioGrant {
        target: target(),
        max_duration_ms: 30_000,
        max_output_bytes: 32 * 1024 * 1024,
    }
}

#[rstest]
fn video_permission_does_not_imply_process_audio() {
    assert_eq!(RecordingAudioMode::default(), RecordingAudioMode::None);
    assert!(
        ProcessAudioGrant::validate_start(RecordingAudioMode::None, None, target(), None)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        ProcessAudioGrant::validate_start(
            RecordingAudioMode::TargetProcessTree,
            None,
            target(),
            Some(20348)
        ),
        Err(ProcessAudioFailure::MissingGrant)
    );
}

#[rstest]
#[case("\"microphone\"")]
#[case("\"desktop\"")]
#[case("{\"source\":\"target_process_tree\",\"process_id\":99}")]
fn audio_mode_rejects_arbitrary_sources_and_caller_pid(#[case] input: &str) {
    assert!(serde_json::from_str::<RecordingAudioMode>(input).is_err());
}

#[rstest]
#[case(None)]
#[case(Some(19041))]
#[case(Some(20347))]
fn process_loopback_rejects_unverified_or_older_windows(#[case] build: Option<u32>) {
    assert_eq!(
        ProcessAudioGrant::validate_start(
            RecordingAudioMode::TargetProcessTree,
            Some(&grant()),
            target(),
            build
        ),
        Err(ProcessAudioFailure::UnsupportedPlatform)
    );
}

#[rstest]
fn exact_birth_and_window_identity_cannot_be_substituted() {
    let g = grant();
    for t in [
        ProcessAudioTarget {
            process_id: 99,
            ..target()
        },
        ProcessAudioTarget {
            process_creation_time_100ns: 124,
            ..target()
        },
        ProcessAudioTarget {
            window_handle: 8,
            ..target()
        },
        ProcessAudioTarget {
            window_thread_id: 10,
            ..target()
        },
        ProcessAudioTarget {
            window_class_hash: 12,
            ..target()
        },
    ] {
        assert_eq!(
            ProcessAudioGrant::validate_start(
                RecordingAudioMode::TargetProcessTree,
                Some(&g),
                t,
                Some(20348)
            ),
            Err(ProcessAudioFailure::InvalidTarget)
        );
    }
    assert!(
        ProcessAudioGrant::validate_start(
            RecordingAudioMode::TargetProcessTree,
            Some(&g),
            target(),
            Some(20348)
        )
        .unwrap()
        .is_some()
    );
}

#[rstest]
fn zero_identity_or_unbounded_budgets_are_refused() {
    for g in [
        ProcessAudioGrant {
            max_duration_ms: 0,
            ..grant()
        },
        ProcessAudioGrant {
            max_duration_ms: MAX_PROCESS_AUDIO_DURATION_MS + 1,
            ..grant()
        },
        ProcessAudioGrant {
            max_output_bytes: 0,
            ..grant()
        },
        ProcessAudioGrant {
            max_output_bytes: MAX_PROCESS_AUDIO_OUTPUT_BYTES + 1,
            ..grant()
        },
    ] {
        assert_eq!(g.validate(), Err(ProcessAudioFailure::InvalidBudget));
    }
    let g = ProcessAudioGrant {
        target: ProcessAudioTarget {
            process_creation_time_100ns: 0,
            ..target()
        },
        ..grant()
    };
    assert_eq!(g.validate(), Err(ProcessAudioFailure::InvalidTarget));
}
