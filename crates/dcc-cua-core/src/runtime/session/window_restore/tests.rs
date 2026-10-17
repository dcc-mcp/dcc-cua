#![cfg(windows)]
use super::*;
use rstest::rstest;

#[rstest]
#[case(false, false, true)]
#[case(true, false, false)]
#[case(false, true, false)]
fn restoration_checks_original_generation_and_latched_session_stop(
    #[case] generation_changed: bool,
    #[case] interrupted: bool,
    #[case] allowed: bool,
) {
    let fence = RestoreCancellation {
        generation: dcc_cua_interrupt::interrupt_generation()
            .wrapping_add(u64::from(generation_changed)),
        interrupted,
        presentation: None,
    };
    let result = fence.check();
    assert_eq!(result.is_ok(), allowed);
    if let Err(error) = result {
        assert_eq!(error.code, ComputerUseErrorCode::UserInterrupted);
    }
}

#[rstest]
#[case(false)]
#[case(true)]
fn cancellation_after_window_mutation_never_claims_predispatch_no_effect(#[case] attempted: bool) {
    let source = super::super::super::windows_pixel_input::validate_pixel_interrupt(1, 2, false)
        .unwrap_err();
    assert_eq!(
        source.details.as_ref().unwrap().phase,
        Some(ComputerUseErrorPhase::PreDispatch)
    );
    let error = restoration_failure(source, attempted);
    assert_eq!(error.code, ComputerUseErrorCode::UserInterrupted);
    let details = error.details.unwrap();
    assert_eq!(
        details.phase,
        Some(if attempted {
            ComputerUseErrorPhase::LocalMutationDispatch
        } else {
            ComputerUseErrorPhase::PreDispatch
        })
    );
    assert_eq!(details.action_attempted, Some(attempted));
    assert_eq!(details.effect_unknown, Some(attempted));
    assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
    assert_eq!(
        details.completion,
        Some(if attempted {
            ComputerUseCompletionState::Unknown
        } else {
            ComputerUseCompletionState::Known
        })
    );
}

#[rstest]
#[case(false, "stop")]
#[case(true, "stop")]
#[case(false, "unavailable")]
#[case(true, "unavailable")]
#[tokio::test]
async fn postawait_failure_preserves_actual_native_dispatch_receipt(
    #[case] attempted: bool,
    #[case] failure: &str,
) {
    let interrupted = std::cell::Cell::new(false);
    let error = finalize_restoration(
        attempted,
        async {
            tokio::task::yield_now().await;
            if failure == "unavailable" {
                return Err(pre_dispatch_failure(ComputerUseError::new(
                    ComputerUseErrorCode::TargetUnavailable,
                    "hidden after refresh",
                )));
            }
            interrupted.set(true);
            Ok(())
        },
        || {
            super::super::super::windows_pixel_input::validate_pixel_interrupt(
                1,
                if interrupted.get() { 2 } else { 1 },
                false,
            )
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.code,
        if failure == "stop" {
            ComputerUseErrorCode::UserInterrupted
        } else {
            ComputerUseErrorCode::TargetUnavailable
        }
    );
    let details = error.details.unwrap();
    assert_eq!(details.action_attempted, Some(attempted));
    assert_eq!(details.effect_unknown, Some(attempted));
    assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
    assert_eq!(
        details.phase,
        Some(if attempted {
            ComputerUseErrorPhase::LocalMutationDispatch
        } else {
            ComputerUseErrorPhase::PreDispatch
        })
    );
}

#[rstest]
#[case("normal", true)]
#[case("instance", false)]
#[case("thread", false)]
#[case("class", false)]
#[case("owner", false)]
#[case("pid", false)]
#[case("hwnd", false)]
#[case("maximized", false)]
#[case("minimized", false)]
#[case("hidden", false)]
#[case("foreground", false)]
fn postawait_native_readback_retains_full_instance_and_normal_state(
    #[case] mutation: &str,
    #[case] allowed: bool,
) {
    let expected = dcc_cua_platform_windows::ExactWindowNativeState {
        process_id: 42,
        window_handle: 99,
        bounds: Some([0, 0, 800, 600]),
        visible_bounds: Some([0, 0, 800, 600]),
        dpi: 96,
        visible: true,
        minimized: false,
        maximized: false,
        foreground: true,
        instance: dcc_cua_platform_windows::ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: 7,
            window_thread_id: 8,
            window_class_hash: 9,
            owner_window_handle: 0,
        },
    };
    let mut actual = expected;
    match mutation {
        "normal" => {}
        "instance" => actual.instance.process_creation_time_100ns += 1,
        "thread" => actual.instance.window_thread_id += 1,
        "class" => actual.instance.window_class_hash += 1,
        "owner" => actual.instance.owner_window_handle += 1,
        "pid" => actual.process_id += 1,
        "hwnd" => actual.window_handle += 1,
        "maximized" => actual.maximized = true,
        "minimized" => actual.minimized = true,
        "hidden" => actual.visible = false,
        "foreground" => actual.foreground = false,
        _ => unreachable!(),
    }
    assert_eq!(
        validate_restored_native_state(&expected, &actual).is_ok(),
        allowed
    );
}
