use super::*;
use rstest::rstest;
use std::cell::Cell;

fn state() -> ExactWindowNativeState {
    ExactWindowNativeState {
        process_id: 42,
        window_handle: 99,
        bounds: Some([100, 200, 1200, 800]),
        visible_bounds: Some([100, 200, 1200, 800]),
        dpi: 96,
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

fn target() -> UiaTarget {
    UiaTarget {
        process_id: 42,
        window_handle: 99,
    }
}

#[rstest]
#[case(0)]
#[case(1)]
#[case(-1)]
fn posted_request_is_single_and_never_substitutes_for_foreground(#[case] api_return: i32) {
    let requests = Cell::new(0);
    let reads = Cell::new(0);
    let waits = Cell::new(0);
    let result = run_queued_activation(
        target(),
        state().instance,
        || {
            reads.set(reads.get() + 1);
            Ok(state())
        },
        || {
            requests.set(requests.get() + 1);
            api_return
        },
        || waits.set(waits.get() + 1),
    );
    assert!(!result.unwrap(), "a posted request is not foreground proof");
    assert_eq!(requests.get(), 1, "dispatch once, without blind retries");
    assert_eq!((reads.get(), waits.get()), (21, 19));
}

#[rstest]
#[case(false)]
#[case(true)]
fn success_requires_exact_foreground_readback_and_preserves_maximized_state(
    #[case] maximized: bool,
) {
    let current = Cell::new(ExactWindowNativeState {
        maximized,
        ..state()
    });
    let requests = Cell::new(0);
    let result = run_queued_activation(
        target(),
        state().instance,
        || Ok(current.get()),
        || {
            requests.set(requests.get() + 1);
            current.set(ExactWindowNativeState {
                foreground: true,
                ..current.get()
            });
            1
        },
        || {},
    );
    assert!(result.unwrap());
    assert_eq!(requests.get(), 1);
    assert_eq!(current.get().maximized, maximized);
    assert_eq!(current.get().bounds, state().bounds);
}

#[rstest]
fn already_foreground_does_not_dispatch_another_request() {
    let result = run_queued_activation(
        target(),
        state().instance,
        || {
            Ok(ExactWindowNativeState {
                foreground: true,
                ..state()
            })
        },
        || panic!("already foreground must not dispatch"),
        || panic!("already foreground needs no polling"),
    );
    assert!(result.unwrap());
}

fn drift(mut value: ExactWindowNativeState, field: &str) -> ExactWindowNativeState {
    match field {
        "creation" => value.instance.process_creation_time_100ns += 1,
        "thread" => value.instance.window_thread_id += 1,
        "class" => value.instance.window_class_hash += 1,
        "owner" => value.instance.owner_window_handle += 1,
        "pid" => value.process_id += 1,
        "hwnd" => value.window_handle += 1,
        _ => unreachable!(),
    }
    value
}

#[rstest]
#[case("creation")]
#[case("thread")]
#[case("class")]
#[case("owner")]
#[case("pid")]
#[case("hwnd")]
fn instance_drift_before_dispatch_refuses_the_native_request(#[case] field: &str) {
    let result = run_queued_activation(
        target(),
        state().instance,
        || Ok(drift(state(), field)),
        || panic!("changed instance must not receive a request"),
        || panic!("changed instance must not be polled"),
    );
    assert!(matches!(result, Err(UiaError::InvalidTarget(_))));
}

#[rstest]
#[case("creation")]
#[case("thread")]
#[case("class")]
#[case("owner")]
#[case("pid")]
#[case("hwnd")]
fn instance_drift_after_dispatch_never_reports_activation_success(#[case] field: &str) {
    let current = Cell::new(state());
    let requests = Cell::new(0);
    let result = run_queued_activation(
        target(),
        state().instance,
        || Ok(current.get()),
        || {
            requests.set(requests.get() + 1);
            current.set(drift(
                ExactWindowNativeState {
                    foreground: true,
                    ..state()
                },
                field,
            ));
            1
        },
        || {},
    );
    assert!(matches!(result, Err(UiaError::InvalidTarget(_))));
    assert_eq!(requests.get(), 1);
}

#[rstest]
#[case(false, false)]
#[case(true, true)]
fn hidden_or_minimized_targets_are_not_restored_implicitly(
    #[case] visible: bool,
    #[case] minimized: bool,
) {
    let result = run_queued_activation(
        target(),
        state().instance,
        || {
            Ok(ExactWindowNativeState {
                visible,
                minimized,
                ..state()
            })
        },
        || panic!("hidden or minimized target must not receive SW_SHOW"),
        || panic!("unavailable target must not be polled"),
    );
    assert!(result.is_err());
}

#[rstest]
fn rejected_input_gate_prevents_native_read_and_dispatch() {
    let requests = Cell::new(0);
    let result = crate::windows::input_gated_window_mutation(
        || Err(UiaError::PermissionDenied("stopped".into())),
        || {
            run_queued_activation(
                target(),
                state().instance,
                || panic!("no native read after gate rejection"),
                || {
                    requests.set(requests.get() + 1);
                    1
                },
                || panic!("no polling after gate rejection"),
            )
        },
    );
    assert!(matches!(result, Err(UiaError::PermissionDenied(_))));
    assert_eq!(requests.get(), 0);
}

#[rstest]
fn queued_phase_is_closed_serializable_metadata_without_undocumented_os_error() {
    let phase = serde_json::from_value::<crate::WindowsForegroundActivationPhase>(
        serde_json::json!("queued_show_window_request"),
    )
    .expect("the queued API needs an explicit diagnostic phase");
    for api_return in [0, 1, -1] {
        let attempt =
            crate::WindowsForegroundActivationAttempt::new(phase, api_return, None, Some(5));
        assert_eq!(attempt.api_return, api_return);
        assert_eq!(attempt.os_error, None);
        let value = serde_json::to_value(&attempt).unwrap();
        assert_eq!(value["phase"], "queued_show_window_request");
        assert_eq!(value.get("os_error"), None);
    }
}
