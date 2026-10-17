use super::*;
use crate::ExactWindowPixelInstanceEvidence;
use rstest::rstest;
use std::cell::Cell;

fn state() -> ExactWindowNativeState {
    ExactWindowNativeState {
        process_id: 42,
        window_handle: 99,
        bounds: Some([100, 200, 3000, 2070]),
        visible_bounds: Some([100, 200, 3000, 2070]),
        dpi: 240,
        visible: true,
        minimized: false,
        maximized: false,
        foreground: false,
        instance: ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: 7,
            window_thread_id: 8,
            window_class_hash: 9,
            owner_window_handle: 0,
        },
    }
}

#[rstest]
#[case(false, false, 0)]
#[case(true, false, 1)]
#[case(false, true, 1)]
#[case(true, true, 1)]
fn explicit_restore_normalizes_minimized_or_maximized_before_activation(
    #[case] minimized: bool,
    #[case] maximized: bool,
    #[case] expected_restores: u32,
) {
    let current = Cell::new(ExactWindowNativeState {
        minimized,
        maximized,
        ..state()
    });
    let restores = Cell::new(0);
    let activations = Cell::new(0);
    let result = run_exact_restore_activate_sequence(
        UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        || Ok(()),
        || Ok(()),
        || Ok(current.get()),
        || {
            restores.set(restores.get() + 1);
            let mut value = current.get();
            value.minimized = false;
            value.maximized = false;
            current.set(value);
            Ok(())
        },
        || {
            assert!(!current.get().minimized && !current.get().maximized);
            activations.set(activations.get() + 1);
            let mut value = current.get();
            value.foreground = true;
            current.set(value);
            Ok(())
        },
        || panic!("synchronous mock restoration needs no wait"),
    );
    assert!(
        result.is_ok(),
        "explicit restore must normalize both states: {result:?}"
    );
    assert_eq!((restores.get(), activations.get()), (expected_restores, 1));
}

#[rstest]
#[case("creation")]
#[case("thread")]
#[case("class")]
#[case("owner")]
#[case("pid")]
#[case("hwnd")]
fn restore_readback_rejects_instance_drift_before_activation(#[case] drift: &str) {
    let current = Cell::new(ExactWindowNativeState {
        minimized: true,
        ..state()
    });
    let restores = Cell::new(0);
    let result = run_exact_restore_activate_sequence(
        UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        || Ok(()),
        || Ok(()),
        || Ok(current.get()),
        || {
            restores.set(restores.get() + 1);
            let mut value = state();
            match drift {
                "creation" => value.instance.process_creation_time_100ns += 1,
                "thread" => value.instance.window_thread_id += 1,
                "class" => value.instance.window_class_hash += 1,
                "owner" => value.instance.owner_window_handle += 1,
                "pid" => value.process_id += 1,
                "hwnd" => value.window_handle += 1,
                _ => unreachable!(),
            }
            current.set(value);
            Ok(())
        },
        || panic!("changed instance must never be activated"),
        || {},
    );
    assert!(matches!(result, Err(UiaError::InvalidTarget(_))));
    assert_eq!(restores.get(), 1);
}

#[rstest]
#[case(1, 0)]
#[case(2, 0)]
#[case(3, 0)]
#[case(4, 1)]
fn restore_stop_gate_prevents_remaining_mutations(#[case] fail_at: u32, #[case] restores: u32) {
    let checks = Cell::new(0);
    let dispatches = Cell::new(0);
    let result = run_exact_restore_activate_sequence(
        UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        || {
            checks.set(checks.get() + 1);
            if checks.get() == fail_at {
                Err(UiaError::PermissionDenied("stopped".into()))
            } else {
                Ok(())
            }
        },
        || panic!("no activation gate after stopped restoration"),
        || {
            Ok(ExactWindowNativeState {
                minimized: true,
                ..state()
            })
        },
        || {
            dispatches.set(dispatches.get() + 1);
            Ok(())
        },
        || panic!("no activation after stop"),
        || {},
    );
    assert!(matches!(&result, Err(UiaError::PermissionDenied(_))));
    assert_eq!(dispatches.get(), restores);
    let classified = classify_restore_result(result, dispatches.get() > 0);
    assert_eq!(
        matches!(classified, Err(ExactWindowRestoreError::AfterDispatch(_))),
        restores > 0
    );
}

#[rstest]
#[case(true, false)]
#[case(false, true)]
fn restore_deadline_is_bounded_without_repeat_dispatch_or_activation(
    #[case] minimized: bool,
    #[case] maximized: bool,
) {
    let reads = Cell::new(0);
    let dispatches = Cell::new(0);
    let waits = Cell::new(0);
    let result = run_exact_restore_activate_sequence(
        UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        || Ok(()),
        || Ok(()),
        || {
            reads.set(reads.get() + 1);
            Ok(ExactWindowNativeState {
                minimized,
                maximized,
                ..state()
            })
        },
        || {
            dispatches.set(dispatches.get() + 1);
            Ok(())
        },
        || panic!("unrestored window must not be activated"),
        || waits.set(waits.get() + 1),
    );
    assert!(matches!(result, Err(UiaError::OperationFailed(_))));
    assert_eq!((reads.get(), dispatches.get(), waits.get()), (42, 1, 39));
}

#[rstest]
#[case(1, 0)]
#[case(2, 0)]
#[case(3, 1)]
fn activation_stop_gate_prevents_remaining_mutations(
    #[case] fail_at: u32,
    #[case] activations: u32,
) {
    let checks = Cell::new(0);
    let dispatches = Cell::new(0);
    let current = Cell::new(state());
    let result = run_exact_restore_activate_sequence(
        UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        || Ok(()),
        || {
            checks.set(checks.get() + 1);
            if checks.get() == fail_at {
                Err(UiaError::PermissionDenied("stopped".into()))
            } else {
                Ok(())
            }
        },
        || Ok(current.get()),
        || panic!("normal window needs no restore"),
        || {
            dispatches.set(dispatches.get() + 1);
            let mut value = current.get();
            value.foreground = true;
            current.set(value);
            Ok(())
        },
        || panic!("normal window needs no wait"),
    );
    assert!(matches!(result, Err(UiaError::PermissionDenied(_))));
    assert_eq!(dispatches.get(), activations);
}

#[rstest]
fn replacement_during_final_restore_gate_is_refused_before_dispatch() {
    let reads = Cell::new(0);
    let result = run_exact_restore_activate_sequence(
        UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        || Ok(()),
        || Ok(()),
        || {
            reads.set(reads.get() + 1);
            let mut value = ExactWindowNativeState {
                maximized: true,
                ..state()
            };
            if reads.get() > 1 {
                value.instance.window_thread_id += 1;
            }
            Ok(value)
        },
        || panic!("replacement must not receive restore"),
        || panic!("replacement must not activate"),
        || {},
    );
    assert!(matches!(result, Err(UiaError::InvalidTarget(_))));
    assert_eq!(reads.get(), 2);
}

#[rstest]
fn minimized_from_maximized_requires_two_distinct_restore_stages_before_activation() {
    let current = Cell::new(ExactWindowNativeState {
        minimized: true,
        ..state()
    });
    let dispatches = Cell::new(0);
    let result = run_exact_restore_activate_sequence(
        UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        || Ok(()),
        || Ok(()),
        || Ok(current.get()),
        || {
            dispatches.set(dispatches.get() + 1);
            let mut value = current.get();
            if value.minimized {
                value.minimized = false;
                value.maximized = true;
            } else {
                assert!(value.maximized);
                value.maximized = false;
            }
            current.set(value);
            Ok(())
        },
        || {
            assert_eq!(dispatches.get(), 2);
            assert!(!current.get().minimized && !current.get().maximized);
            let mut value = current.get();
            value.foreground = true;
            current.set(value);
            Ok(())
        },
        || panic!("completed stages must not spin or redispatch"),
    );
    assert!(result.is_ok());
    assert_eq!(dispatches.get(), 2);
}

#[rstest]
#[case(1, 0)]
#[case(2, 0)]
#[case(3, 1)]
fn single_foreground_request_is_stoppable_without_fallback_mutations(
    #[case] fail_at: u32,
    #[case] expected_requests: u32,
) {
    let checks = Cell::new(0);
    let requests = Cell::new(0);
    let before = state();
    let result = run_restored_foreground_request(
        UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        before.instance,
        || {
            checks.set(checks.get() + 1);
            if checks.get() == fail_at {
                Err(UiaError::PermissionDenied("stopped".into()))
            } else {
                Ok(())
            }
        },
        || Ok(before),
        || {
            requests.set(requests.get() + 1);
            Ok(())
        },
        || {},
    );
    assert!(matches!(&result, Err(UiaError::PermissionDenied(_))));
    assert_eq!(requests.get(), expected_requests);
    assert_eq!(
        matches!(
            classify_restore_result(result, requests.get() > 0),
            Err(ExactWindowRestoreError::AfterDispatch(_))
        ),
        expected_requests > 0
    );
}

#[rstest]
fn foreground_refusal_is_bounded_to_one_request_and_no_fallback() {
    let reads = Cell::new(0);
    let requests = Cell::new(0);
    let waits = Cell::new(0);
    let before = state();
    let result = run_restored_foreground_request(
        UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        before.instance,
        || Ok(()),
        || {
            reads.set(reads.get() + 1);
            Ok(before)
        },
        || {
            requests.set(requests.get() + 1);
            Ok(())
        },
        || waits.set(waits.get() + 1),
    );
    assert!(!result.unwrap());
    assert_eq!((reads.get(), requests.get(), waits.get()), (21, 1, 19));
}

#[rstest]
fn already_foreground_normal_target_needs_no_native_mutation() {
    let before = ExactWindowNativeState {
        foreground: true,
        ..state()
    };
    let attempted = Cell::new(false);
    let result = run_restored_foreground_request(
        UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        before.instance,
        || Ok(()),
        || Ok(before),
        || {
            attempted.set(true);
            panic!("normal foreground target must not receive any native request")
        },
        || panic!("no polling without a foreground request"),
    );
    assert!(result.unwrap());
    assert!(!attempted.get());
}

#[rstest]
#[case("maximized")]
#[case("instance")]
fn activation_final_race_never_confirms_normal_state(#[case] change: &str) {
    let current = Cell::new(state());
    let requests = Cell::new(0);
    let result = run_exact_restore_activate_sequence(
        UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        || Ok(()),
        || Ok(()),
        || Ok(current.get()),
        || panic!("normal window needs no restore"),
        || {
            requests.set(requests.get() + 1);
            let mut value = current.get();
            value.foreground = true;
            if change == "maximized" {
                value.maximized = true;
            } else {
                value.instance.window_thread_id += 1;
            }
            current.set(value);
            Ok(())
        },
        || {},
    );
    assert!(result.is_err());
    assert!(matches!(
        classify_restore_result(result, requests.get() > 0),
        Err(ExactWindowRestoreError::AfterDispatch(_))
    ));
}

#[rstest]
#[case(5, 1)]
#[case(6, 1)]
#[case(7, 2)]
fn stop_between_minimized_and_maximized_stages_preserves_actual_dispatch_receipt(
    #[case] fail_at: u32,
    #[case] expected_dispatches: u32,
) {
    let current = Cell::new(ExactWindowNativeState {
        minimized: true,
        ..state()
    });
    let checks = Cell::new(0);
    let dispatches = Cell::new(0);
    let result = run_exact_restore_activate_sequence(
        UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        || {
            checks.set(checks.get() + 1);
            if checks.get() == fail_at {
                Err(UiaError::PermissionDenied("stopped".into()))
            } else {
                Ok(())
            }
        },
        || panic!("stop forbids activation"),
        || Ok(current.get()),
        || {
            dispatches.set(dispatches.get() + 1);
            let mut value = current.get();
            value.maximized = value.minimized;
            value.minimized = false;
            current.set(value);
            Ok(())
        },
        || panic!("stop forbids activation"),
        || {},
    );
    assert_eq!(dispatches.get(), expected_dispatches);
    assert!(matches!(
        classify_restore_result(result, dispatches.get() > 0),
        Err(ExactWindowRestoreError::AfterDispatch(
            UiaError::PermissionDenied(_)
        ))
    ));
}
