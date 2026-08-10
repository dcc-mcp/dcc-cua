use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::pending;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dcc_cua_indicator::{BannerActivity, IndicatorError};
use rstest::rstest;
use serde_json::{Value, json};

use super::*;
use crate::contracts::{
    DEFAULT_SNAPSHOT_MAX_DEPTH, DEFAULT_SNAPSHOT_MAX_ELEMENTS, MAX_SNAPSHOT_DEPTH,
    MAX_SNAPSHOT_ELEMENTS, MAX_TEXT_UTF16_UNITS, MOUSE_CURSOR_THEME,
};
use crate::driver_factory::{
    BUNDLED_CURSOR_THEME, UPSTREAM_CURSOR_RENDERER_ENABLED, driver_host_options,
};
use crate::interactive_desktop::{
    platform_managed_diagnostic, require_desktop_observation_from,
    require_exact_window_observation_from, require_input_available_from, windows_diagnostic,
};
use crate::live_observation::{
    CaptureFailureDisposition, LiveObservationFence, LiveObservationFrame, LiveObservationStatus,
    decode_png_to_bgra, live_capture_failure_disposition, observation_sequence_fence,
    post_action_sequence_fence, terminal_capture_error, wait_for_latest_frame,
};
use crate::policy::*;
use crate::runtime::application::{launch_arguments, validate_launch_request};
use crate::runtime::{
    ActionBannerPhase, CombinedDownDragAfterDown, CombinedDownDragCleanup, CombinedDownDragPrelude,
    CombinedDownInjection, LiveObservationStartDisposition, RawDragSequenceOutcome,
    RecordingHealth, RecordingKeepalive, SingleInputInjection, activation_completion_unknown,
    aggregate_recording_state, attach_banner_status, attach_indicator_motion_to_activation,
    await_input_call, banner_activity_for_action_phase, banner_activity_for_bound_tool,
    diagnostic_tool_check, ensure_target_available_for_action, gated_cursor_operation,
    gated_desktop_observation, gated_exact_window_observation, gated_exact_window_publication,
    held_coordinate_click_as_drag, input_backend_rejection_result,
    live_observation_start_disposition, map_indicator_error, preflight_live_observation_start,
    run_gated_preinvalidated_window_mutation, run_preinvalidated_window_mutation,
    run_windows_combined_down_drag_sequence, run_windows_fenced_absolute_path,
    run_windows_fenced_absolute_path_with_trace, run_windows_separated_raw_drag_sequence,
    tool_schema_from_inventory,
};
#[cfg(windows)]
use crate::runtime::{
    RELATIVE_DRAG_MAX_ATTEMPTS_PER_WAYPOINT, RelativeMoveInjection, WindowsForegroundDragBackend,
    WindowsPostButtonUpSnapshot, WindowsRawDragInputTrace,
    inject_windows_combined_input_batch_with, map_windows_window_mutation_error,
    run_windows_calibrated_relative_path, select_windows_foreground_drag_backend,
    uses_windows_foreground_fast_path, windows_combined_raw_drag_outcome,
    windows_combined_source_move_and_left_down_inputs, windows_raw_drag_delivery,
    windows_synthetic_touch_attempt, windows_synthetic_touch_result,
};
use crate::window_target::{WindowTarget, validate_target_policy};

macro_rules! run_combined_down_drag_sequence {
    (
        $inspect_pre:expr,
        $allow_pre:expr,
        $inject_batch:expr,
        $inspect_after:expr,
        $allow_path:expr,
        $move_path:expr,
        $settle:expr,
        $inject_up:expr,
        $inspect_post:expr,
        $allow_release:expr $(,)?
    ) => {
        run_windows_combined_down_drag_sequence(
            CombinedDownDragPrelude::new($inspect_pre, $allow_pre, $inject_batch),
            CombinedDownDragAfterDown::new($inspect_after, $allow_path),
            $move_path,
            $settle,
            CombinedDownDragCleanup::new($inject_up, $inspect_post, $allow_release),
        )
    };
}

#[rstest]
#[tokio::test]
async fn input_calls_have_a_hard_timeout() {
    let error = await_input_call(
        pending::<()>(),
        Duration::from_millis(1),
        "window activation",
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::InputFailed);
    assert!(error.message.contains("window activation timed out"));
    assert!(error.message.contains("window session was invalidated"));
}

#[test]
fn activation_timeout_is_typed_completion_unknown_without_blind_retry() {
    let error = activation_completion_unknown(ComputerUseError::new(
        ComputerUseErrorCode::InputFailed,
        "window activation timed out",
    ));

    assert_eq!(error.code, ComputerUseErrorCode::CompletionUnknown);
    assert!(error.message.contains("completion_unknown=true"));
    assert!(error.message.contains("automatic_input=false"));
    assert!(error.message.contains("blind_retry=false"));
    assert!(!error.message.contains("session was invalidated"));
}

#[rstest]
#[case("click", ActionBannerPhase::Preparing, BannerActivity::Operating)]
#[case("type", ActionBannerPhase::Preparing, BannerActivity::Operating)]
#[case("click", ActionBannerPhase::Injecting, BannerActivity::PointerInput)]
#[case("type", ActionBannerPhase::Injecting, BannerActivity::KeyboardInput)]
fn action_banner_activity_distinguishes_preparation_from_real_injection(
    #[case] action: &str,
    #[case] phase: ActionBannerPhase,
    #[case] expected: BannerActivity,
) {
    assert_eq!(
        banner_activity_for_action_phase(
            &ComputerUseAction {
                action: action.into(),
                ..Default::default()
            },
            phase,
        ),
        expected
    );
}

#[derive(Debug, PartialEq, Eq)]
enum RawDragEvent {
    InjectConsumableMoveAtSource,
    Settle(Duration),
    InjectSingleDown,
    SampleAfterDown,
    MovePath,
    InjectSingleUp,
}

#[derive(Debug, PartialEq, Eq)]
enum CombinedDownDragEvent {
    SamplePreBatchFence,
    InjectAbsoluteMoveAndLeftDownBatch,
    SampleAfterDown,
    MoveAbsolutePath,
    Settle(Duration),
    InjectBestEffortLeftUp,
    SamplePostUp,
}

#[test]
fn combined_down_drag_uses_one_fenced_batch_and_the_existing_drop_sequence() {
    let events = RefCell::new(Vec::new());

    let outcome = run_combined_down_drag_sequence!(
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::SamplePreBatchFence);
            Ok::<_, &'static str>(false)
        },
        |button_down| !*button_down,
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::InjectAbsoluteMoveAndLeftDownBatch);
            CombinedDownInjection::accepted()
        },
        |inserted| {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::SampleAfterDown);
            assert_eq!(inserted, 2);
            Ok::<_, &'static str>(true)
        },
        |button_down| *button_down,
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::MoveAbsolutePath);
            Ok::<_, &'static str>(())
        },
        |duration| {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::Settle(duration));
        },
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::InjectBestEffortLeftUp);
            SingleInputInjection::accepted()
        },
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::SamplePostUp);
            Ok::<_, &'static str>(false)
        },
        |button_down| !*button_down,
    );

    assert_eq!(outcome.batch_inserted, 2);
    assert!(outcome.path_sent);
    assert_eq!(outcome.failure_phase, None);
    assert_eq!(outcome.failure_error, None);
    assert_eq!(outcome.release_error, None);
    assert_eq!(
        events.into_inner(),
        vec![
            CombinedDownDragEvent::SamplePreBatchFence,
            CombinedDownDragEvent::InjectAbsoluteMoveAndLeftDownBatch,
            CombinedDownDragEvent::SampleAfterDown,
            CombinedDownDragEvent::Settle(Duration::from_millis(50)),
            CombinedDownDragEvent::MoveAbsolutePath,
            CombinedDownDragEvent::Settle(Duration::from_millis(75)),
            CombinedDownDragEvent::InjectBestEffortLeftUp,
            CombinedDownDragEvent::Settle(Duration::from_millis(50)),
            CombinedDownDragEvent::SamplePostUp,
        ]
    );
}

#[rstest]
#[case(0)]
#[case(1)]
fn combined_down_drag_does_not_release_without_batch_button_ownership(#[case] inserted: u32) {
    let events = RefCell::new(Vec::new());

    let outcome = run_combined_down_drag_sequence!(
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::SamplePreBatchFence);
            Ok::<_, &'static str>(false)
        },
        |button_down| !*button_down,
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::InjectAbsoluteMoveAndLeftDownBatch);
            CombinedDownInjection::incomplete(inserted, "SendInput batch incomplete")
        },
        |_| -> Result<bool, &'static str> {
            panic!("an incomplete ordered batch must not probe button state")
        },
        |_: &bool| panic!("an incomplete ordered batch cannot own a drag path"),
        || panic!("an incomplete ordered batch must not move the path"),
        |_| panic!("an incomplete ordered batch must not settle an unowned button"),
        || -> SingleInputInjection<&'static str> {
            panic!("MOVE-only insertion must not release an unowned button")
        },
        || -> Result<bool, &'static str> {
            panic!("MOVE-only insertion must not sample post-UP state")
        },
        |_: &bool| panic!("MOVE-only insertion cannot verify release"),
    );

    assert_eq!(outcome.batch_inserted, inserted);
    assert!(!outcome.path_sent);
    assert_eq!(outcome.failure_phase, Some("combined_move_down_batch"));
    assert_eq!(outcome.failure_error, Some("SendInput batch incomplete"));
    assert_eq!(outcome.release_error, None);
    assert_eq!(
        events.into_inner(),
        vec![
            CombinedDownDragEvent::SamplePreBatchFence,
            CombinedDownDragEvent::InjectAbsoluteMoveAndLeftDownBatch,
        ]
    );
}

#[test]
fn combined_down_drag_releases_owned_button_when_after_down_fence_rejects() {
    let events = RefCell::new(Vec::new());

    let outcome = run_combined_down_drag_sequence!(
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::SamplePreBatchFence);
            Ok::<_, &'static str>(false)
        },
        |button_down| !*button_down,
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::InjectAbsoluteMoveAndLeftDownBatch);
            CombinedDownInjection::accepted()
        },
        |_| {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::SampleAfterDown);
            Ok::<_, &'static str>(false)
        },
        |allows| *allows,
        || panic!("a rejected after-down fence must not move the path"),
        |duration| {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::Settle(duration));
        },
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::InjectBestEffortLeftUp);
            SingleInputInjection::accepted()
        },
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::SamplePostUp);
            Ok::<_, &'static str>(false)
        },
        |button_down| !*button_down,
    );

    assert!(!outcome.path_sent);
    assert_eq!(outcome.failure_phase, Some("after_button_down_fence"));
    assert_eq!(outcome.after_down, Some(false));
    assert_eq!(
        events.into_inner(),
        vec![
            CombinedDownDragEvent::SamplePreBatchFence,
            CombinedDownDragEvent::InjectAbsoluteMoveAndLeftDownBatch,
            CombinedDownDragEvent::SampleAfterDown,
            CombinedDownDragEvent::InjectBestEffortLeftUp,
            CombinedDownDragEvent::Settle(Duration::from_millis(50)),
            CombinedDownDragEvent::SamplePostUp,
        ]
    );
}

#[test]
fn combined_down_drag_releases_owned_button_when_absolute_path_fails() {
    let events = RefCell::new(Vec::new());

    let outcome = run_combined_down_drag_sequence!(
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::SamplePreBatchFence);
            Ok::<_, &'static str>(false)
        },
        |button_down| !*button_down,
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::InjectAbsoluteMoveAndLeftDownBatch);
            CombinedDownInjection::accepted()
        },
        |_| {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::SampleAfterDown);
            Ok::<_, &'static str>(true)
        },
        |button_down| *button_down,
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::MoveAbsolutePath);
            Err("exact target foreground fence was lost")
        },
        |duration| {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::Settle(duration));
        },
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::InjectBestEffortLeftUp);
            SingleInputInjection::accepted()
        },
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::SamplePostUp);
            Ok::<_, &'static str>(false)
        },
        |button_down| !*button_down,
    );

    assert!(!outcome.path_sent);
    assert_eq!(outcome.failure_phase, Some("absolute_path"));
    assert_eq!(
        outcome.failure_error,
        Some("exact target foreground fence was lost")
    );
    assert_eq!(
        events.into_inner(),
        vec![
            CombinedDownDragEvent::SamplePreBatchFence,
            CombinedDownDragEvent::InjectAbsoluteMoveAndLeftDownBatch,
            CombinedDownDragEvent::SampleAfterDown,
            CombinedDownDragEvent::Settle(Duration::from_millis(50)),
            CombinedDownDragEvent::MoveAbsolutePath,
            CombinedDownDragEvent::InjectBestEffortLeftUp,
            CombinedDownDragEvent::Settle(Duration::from_millis(50)),
            CombinedDownDragEvent::SamplePostUp,
        ]
    );
}

#[derive(Debug, PartialEq, Eq)]
enum FencedAbsolutePathEvent {
    ValidateExactTarget,
    SendMove(i32, i32),
    Settle(Duration),
}

#[test]
fn combined_down_drag_revalidates_the_exact_target_before_every_absolute_waypoint() {
    let events = RefCell::new(Vec::new());

    run_windows_fenced_absolute_path(
        (0, 0),
        (9, 6),
        3,
        30,
        || {
            events
                .borrow_mut()
                .push(FencedAbsolutePathEvent::ValidateExactTarget);
            Ok::<_, &'static str>(())
        },
        |x, y| {
            events
                .borrow_mut()
                .push(FencedAbsolutePathEvent::SendMove(x, y));
            Ok::<_, &'static str>(())
        },
        |duration| {
            events
                .borrow_mut()
                .push(FencedAbsolutePathEvent::Settle(duration));
        },
    )
    .unwrap();

    assert_eq!(
        events.into_inner(),
        vec![
            FencedAbsolutePathEvent::ValidateExactTarget,
            FencedAbsolutePathEvent::SendMove(3, 2),
            FencedAbsolutePathEvent::Settle(Duration::from_millis(10)),
            FencedAbsolutePathEvent::ValidateExactTarget,
            FencedAbsolutePathEvent::SendMove(6, 4),
            FencedAbsolutePathEvent::Settle(Duration::from_millis(10)),
            FencedAbsolutePathEvent::ValidateExactTarget,
            FencedAbsolutePathEvent::SendMove(9, 6),
            FencedAbsolutePathEvent::Settle(Duration::from_millis(10)),
        ]
    );
}

#[test]
fn combined_down_drag_stops_before_the_waypoint_whose_live_fence_fails() {
    let fence_checks = Cell::new(0_u32);
    let moves = RefCell::new(Vec::new());

    let outcome = run_windows_fenced_absolute_path_with_trace(
        (0, 0),
        (9, 6),
        3,
        30,
        || {
            let next = fence_checks.get() + 1;
            fence_checks.set(next);
            if next == 2 {
                Err("exact PID/HWND/foreground/button fence was lost")
            } else {
                Ok(())
            }
        },
        |x, y| {
            moves.borrow_mut().push((x, y));
            Ok(())
        },
        |_| {},
    );

    assert!(outcome.path_started());
    assert_eq!(outcome.moves_inserted(), 1);
    assert_eq!(outcome.waypoints_completed(), 1);
    assert_eq!(outcome.total_waypoints(), 3);
    assert_eq!(
        outcome.into_result(),
        Err("exact PID/HWND/foreground/button fence was lost")
    );
    assert_eq!(fence_checks.get(), 2);
    assert_eq!(moves.into_inner(), vec![(3, 2)]);
}

#[test]
fn combined_down_drag_pre_batch_fence_failure_injects_nothing() {
    let outcome = run_combined_down_drag_sequence!(
        || Err::<bool, _>("exact target is no longer foreground"),
        |_: &bool| panic!("a failed pre-batch sample has no evidence to inspect"),
        || panic!("a failed pre-batch fence must inject no batch"),
        |_| -> Result<bool, &'static str> {
            panic!("a failed pre-batch fence must not sample after DOWN")
        },
        |_: &bool| panic!("a failed pre-batch fence cannot own a path"),
        || panic!("a failed pre-batch fence must not move"),
        |_| panic!("a failed pre-batch fence must not settle input"),
        || -> SingleInputInjection<&'static str> {
            panic!("a failed pre-batch fence must not inject UP")
        },
        || -> Result<bool, &'static str> {
            panic!("a failed pre-batch fence must not sample post-UP")
        },
        |_: &bool| panic!("a failed pre-batch fence cannot verify release"),
    );

    assert_eq!(outcome.batch_inserted, 0);
    assert_eq!(outcome.failure_phase, Some("pre_batch_target_fence"));
    assert_eq!(
        outcome.failure_error,
        Some("exact target is no longer foreground")
    );
    assert!(!outcome.release_attempted);
    assert_eq!(outcome.release_inserted, None);
}

#[test]
fn combined_down_drag_refuses_a_preexisting_left_button_down_without_claiming_ownership() {
    let outcome = run_combined_down_drag_sequence!(
        || Ok::<_, &'static str>(true),
        |button_down| !*button_down,
        || panic!("preexisting LEFTDOWN must block combined injection"),
        |_| -> Result<bool, &'static str> {
            panic!("preexisting LEFTDOWN must block after-DOWN sampling")
        },
        |_: &bool| panic!("preexisting LEFTDOWN must not own a new path"),
        || panic!("preexisting LEFTDOWN must not move"),
        |_| panic!("preexisting LEFTDOWN must not settle input"),
        || -> SingleInputInjection<&'static str> {
            panic!("preexisting LEFTDOWN must not be released by this operation")
        },
        || -> Result<bool, &'static str> { panic!("preexisting LEFTDOWN must not sample post-UP") },
        |_: &bool| panic!("preexisting LEFTDOWN was never owned"),
    );

    assert_eq!(outcome.pre_batch, Some(true));
    assert_eq!(outcome.batch_inserted, 0);
    assert_eq!(outcome.failure_phase, Some("pre_batch_fence"));
    assert!(!outcome.release_attempted);
}

#[test]
fn combined_down_drag_after_down_probe_error_still_releases_owned_button() {
    let outcome = run_combined_down_drag_sequence!(
        || Ok::<_, &'static str>(false),
        |button_down| !*button_down,
        CombinedDownInjection::accepted,
        |_| Err::<bool, _>("after-down OS snapshot failed"),
        |_: &bool| panic!("a missing after-down snapshot cannot authorize a path"),
        || panic!("a missing after-down snapshot must not move"),
        |_| {},
        SingleInputInjection::accepted,
        || Ok(false),
        |button_down| !*button_down,
    );

    assert_eq!(outcome.failure_phase, Some("after_button_down_probe"));
    assert_eq!(outcome.failure_error, Some("after-down OS snapshot failed"));
    assert!(outcome.release_attempted);
    assert_eq!(outcome.release_inserted, Some(1));
    assert_eq!(outcome.post_up_released, Some(true));
}

#[test]
fn combined_down_drag_preserves_path_and_button_up_failures_independently() {
    let outcome = run_combined_down_drag_sequence!(
        || Ok::<_, &'static str>(false),
        |button_down| !*button_down,
        CombinedDownInjection::accepted,
        |_| Ok(true),
        |button_down| *button_down,
        || Err("waypoint target fence was lost"),
        |_| {},
        || SingleInputInjection::incomplete(0, "LEFTUP inserted 0/1"),
        || Ok(true),
        |button_down| !*button_down,
    );

    assert_eq!(outcome.failure_phase, Some("absolute_path"));
    assert_eq!(
        outcome.failure_error,
        Some("waypoint target fence was lost")
    );
    assert_eq!(outcome.release_error, Some("LEFTUP inserted 0/1"));
    assert_eq!(outcome.cleanup_failure_phase, Some("button_up_injection"));
    assert_eq!(outcome.post_up_released, Some(false));
}

#[test]
fn combined_down_drag_treats_left_still_down_after_up_as_cleanup_failure() {
    let outcome = run_combined_down_drag_sequence!(
        || Ok::<_, &'static str>(false),
        |button_down| !*button_down,
        CombinedDownInjection::accepted,
        |_| Ok(true),
        |button_down| *button_down,
        || Ok(()),
        |_| {},
        SingleInputInjection::accepted,
        || Ok(true),
        |button_down| !*button_down,
    );

    assert!(outcome.path_sent);
    assert_eq!(outcome.post_up, Some(true));
    assert_eq!(outcome.post_up_released, Some(false));
    assert_eq!(outcome.cleanup_failure_phase, Some("post_button_up_fence"));
}

#[test]
fn combined_down_drag_does_not_settle_after_rejected_up_before_post_sample() {
    let events = RefCell::new(Vec::new());
    let outcome = run_combined_down_drag_sequence!(
        || Ok::<_, &'static str>(false),
        |button_down| !*button_down,
        CombinedDownInjection::accepted,
        |_| Ok(true),
        |button_down| *button_down,
        || Ok(()),
        |duration| {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::Settle(duration));
        },
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::InjectBestEffortLeftUp);
            SingleInputInjection::incomplete(0, "LEFTUP inserted 0/1")
        },
        || {
            events
                .borrow_mut()
                .push(CombinedDownDragEvent::SamplePostUp);
            Ok(true)
        },
        |button_down| !*button_down,
    );

    assert_eq!(
        events.into_inner(),
        vec![
            CombinedDownDragEvent::Settle(Duration::from_millis(50)),
            CombinedDownDragEvent::Settle(Duration::from_millis(75)),
            CombinedDownDragEvent::InjectBestEffortLeftUp,
            CombinedDownDragEvent::SamplePostUp,
        ]
    );
    assert_eq!(outcome.release_inserted, Some(0));
    assert_eq!(outcome.post_up_released, Some(false));
}

#[test]
fn raw_drag_settles_at_the_drop_before_up_and_after_up() {
    let events = RefCell::new(Vec::new());

    run_windows_separated_raw_drag_sequence(
        || {
            events
                .borrow_mut()
                .push(RawDragEvent::InjectConsumableMoveAtSource);
            Ok::<_, &'static str>(())
        },
        |duration| events.borrow_mut().push(RawDragEvent::Settle(duration)),
        || {
            events.borrow_mut().push(RawDragEvent::InjectSingleDown);
            Ok(1)
        },
        |inserted| {
            events.borrow_mut().push(RawDragEvent::SampleAfterDown);
            assert_eq!(inserted, 1);
        },
        |_| true,
        || {
            events.borrow_mut().push(RawDragEvent::MovePath);
            Ok(())
        },
        || {
            events.borrow_mut().push(RawDragEvent::InjectSingleUp);
            Ok(())
        },
    )
    .expect("separated raw drag succeeds");

    assert_eq!(
        events.into_inner(),
        vec![
            RawDragEvent::InjectConsumableMoveAtSource,
            RawDragEvent::Settle(Duration::from_millis(50)),
            RawDragEvent::InjectSingleDown,
            RawDragEvent::SampleAfterDown,
            RawDragEvent::Settle(Duration::from_millis(50)),
            RawDragEvent::MovePath,
            RawDragEvent::Settle(Duration::from_millis(75)),
            RawDragEvent::InjectSingleUp,
            RawDragEvent::Settle(Duration::from_millis(50)),
        ]
    );
}

#[test]
fn raw_drag_releases_the_button_when_a_path_move_fails() {
    let events = RefCell::new(Vec::new());

    let error = run_windows_separated_raw_drag_sequence(
        || {
            events
                .borrow_mut()
                .push(RawDragEvent::InjectConsumableMoveAtSource);
            Ok::<_, &'static str>(())
        },
        |duration| events.borrow_mut().push(RawDragEvent::Settle(duration)),
        || {
            events.borrow_mut().push(RawDragEvent::InjectSingleDown);
            Ok(1)
        },
        |_| {
            events.borrow_mut().push(RawDragEvent::SampleAfterDown);
        },
        |_| true,
        || {
            events.borrow_mut().push(RawDragEvent::MovePath);
            Err("path move failed")
        },
        || {
            events.borrow_mut().push(RawDragEvent::InjectSingleUp);
            Ok(())
        },
    )
    .expect_err("failed path movement must fail the drag");

    assert_eq!(error, "path move failed");
    assert_eq!(
        events.into_inner(),
        vec![
            RawDragEvent::InjectConsumableMoveAtSource,
            RawDragEvent::Settle(Duration::from_millis(50)),
            RawDragEvent::InjectSingleDown,
            RawDragEvent::SampleAfterDown,
            RawDragEvent::Settle(Duration::from_millis(50)),
            RawDragEvent::MovePath,
            RawDragEvent::InjectSingleUp,
            RawDragEvent::Settle(Duration::from_millis(50)),
        ]
    );
}

#[test]
fn raw_drag_stops_before_movement_when_the_single_down_is_not_inserted() {
    let events = RefCell::new(Vec::new());

    let error = run_windows_separated_raw_drag_sequence(
        || {
            events
                .borrow_mut()
                .push(RawDragEvent::InjectConsumableMoveAtSource);
            Ok::<_, &'static str>(())
        },
        |duration| events.borrow_mut().push(RawDragEvent::Settle(duration)),
        || {
            events.borrow_mut().push(RawDragEvent::InjectSingleDown);
            Err("button down was not inserted")
        },
        |_| {
            events.borrow_mut().push(RawDragEvent::SampleAfterDown);
        },
        |_| true,
        || {
            events.borrow_mut().push(RawDragEvent::MovePath);
            Ok(())
        },
        || {
            events.borrow_mut().push(RawDragEvent::InjectSingleUp);
            Ok(())
        },
    )
    .expect_err("an unconfirmed button-down must stop the drag");

    assert_eq!(error, "button down was not inserted");
    assert_eq!(
        events.into_inner(),
        vec![
            RawDragEvent::InjectConsumableMoveAtSource,
            RawDragEvent::Settle(Duration::from_millis(50)),
            RawDragEvent::InjectSingleDown,
        ]
    );
}

#[test]
fn raw_drag_never_presses_when_source_positioning_fails() {
    let events = RefCell::new(Vec::new());

    let error = run_windows_separated_raw_drag_sequence(
        || {
            events
                .borrow_mut()
                .push(RawDragEvent::InjectConsumableMoveAtSource);
            Err::<(), _>("move failed")
        },
        |duration| events.borrow_mut().push(RawDragEvent::Settle(duration)),
        || {
            events.borrow_mut().push(RawDragEvent::InjectSingleDown);
            Ok(1)
        },
        |_| {
            events.borrow_mut().push(RawDragEvent::SampleAfterDown);
        },
        |_| true,
        || {
            events.borrow_mut().push(RawDragEvent::MovePath);
            Ok(())
        },
        || {
            events.borrow_mut().push(RawDragEvent::InjectSingleUp);
            Ok(())
        },
    )
    .expect_err("failed source positioning must stop before mouse-down");

    assert_eq!(error, "move failed");
    assert_eq!(
        events.into_inner(),
        vec![RawDragEvent::InjectConsumableMoveAtSource]
    );
}

#[test]
fn raw_drag_releases_without_moving_when_after_down_probe_rejects_delivery() {
    let events = RefCell::new(Vec::new());

    let error = run_windows_separated_raw_drag_sequence(
        || {
            events
                .borrow_mut()
                .push(RawDragEvent::InjectConsumableMoveAtSource);
            Ok::<_, &'static str>(())
        },
        |duration| events.borrow_mut().push(RawDragEvent::Settle(duration)),
        || {
            events.borrow_mut().push(RawDragEvent::InjectSingleDown);
            Ok(1)
        },
        |_| {
            events.borrow_mut().push(RawDragEvent::SampleAfterDown);
        },
        |_| false,
        || {
            events.borrow_mut().push(RawDragEvent::MovePath);
            Ok(())
        },
        || {
            events.borrow_mut().push(RawDragEvent::InjectSingleUp);
            Ok(())
        },
    )
    .expect("a rejected probe returns a structured attempt outcome");

    assert!(!error.path_sent);
    assert_eq!(error.release_error, None);
    assert_eq!(
        events.into_inner(),
        vec![
            RawDragEvent::InjectConsumableMoveAtSource,
            RawDragEvent::Settle(Duration::from_millis(50)),
            RawDragEvent::InjectSingleDown,
            RawDragEvent::SampleAfterDown,
            RawDragEvent::InjectSingleUp,
            RawDragEvent::Settle(Duration::from_millis(50)),
        ]
    );
}

#[test]
fn raw_drag_rejected_probe_keeps_best_effort_release_failure_in_the_outcome() {
    let outcome = run_windows_separated_raw_drag_sequence(
        || Ok::<_, &'static str>(()),
        |_| {},
        || Ok(1),
        |_| (),
        |_| false,
        || panic!("a rejected probe must never enter the path"),
        || Err("button-up was not inserted"),
    )
    .expect("the typed rejected outcome must survive cleanup failure");

    assert!(!outcome.path_sent);
    assert_eq!(outcome.release_error, Some("button-up was not inserted"));
}

#[cfg(windows)]
fn exact_raw_input_snapshot(
    async_button_down: bool,
) -> dcc_cua_platform_windows::WindowsRawInputSnapshot {
    use dcc_cua_platform_windows::{
        WindowsForegroundRelation, WindowsRawInputSnapshot, WindowsWindowIdentity,
    };

    WindowsRawInputSnapshot {
        async_button_down,
        target: WindowsWindowIdentity {
            window_handle: 0x1234,
            process_id: 77,
        },
        foreground: Some(WindowsWindowIdentity {
            window_handle: 0x1234,
            process_id: 77,
        }),
        foreground_relation: WindowsForegroundRelation::ExactTarget,
        target_thread_capture: None,
        capture_query_succeeded: true,
        capture_owned_by_target_process: false,
    }
}

#[cfg(windows)]
#[rstest]
#[case(0, false)]
#[case(1, true)]
fn combined_down_drag_partial_batch_trace_reports_input_without_claiming_release(
    #[case] inserted: u32,
    #[case] input_sent: bool,
) {
    let sequence = run_combined_down_drag_sequence!(
        || Ok::<_, String>(exact_raw_input_snapshot(false)),
        |snapshot| {
            !snapshot.async_button_down
                && snapshot.foreground_relation
                    == dcc_cua_platform_windows::WindowsForegroundRelation::ExactTarget
        },
        || CombinedDownInjection::incomplete(inserted, "ordered batch was incomplete".to_owned()),
        |_| -> Result<dcc_cua_platform_windows::WindowsRawInputSnapshot, String> {
            panic!("a partial batch must not probe after DOWN")
        },
        |_| panic!("a partial batch cannot authorize a path"),
        || panic!("a partial batch cannot move"),
        |_| panic!("a partial batch must not settle"),
        || -> SingleInputInjection<String> { panic!("MOVE-only insertion must not inject UP") },
        || -> Result<WindowsPostButtonUpSnapshot, String> {
            panic!("MOVE-only insertion must not sample post-UP")
        },
        |_| panic!("MOVE-only insertion has no release evidence"),
    );
    let outcome = windows_combined_raw_drag_outcome(sequence);
    let delivery = windows_raw_drag_delivery(&outcome, &test_window_target());

    assert_eq!(
        delivery["backend_id"],
        WINDOWS_COMBINED_DOWN_DRAG_BACKEND_ID
    );
    assert_eq!(delivery["api_accepted"], false);
    assert_eq!(delivery["consumer_effect_confirmed"], false);
    assert_eq!(delivery["verification_required"], true);
    assert_eq!(delivery["retry_safe"], false);
    assert_eq!(delivery["fallback_attempted"], false);
    assert_eq!(delivery["input_sent"], input_sent);
    assert_eq!(delivery["release_succeeded"], Value::Null);
    assert_eq!(delivery["failure_phase"], "combined_move_down_batch");
    assert_eq!(
        delivery["input_trace"]["send_input"],
        json!({"requested": 2, "inserted": inserted})
    );
    assert_eq!(
        delivery["input_trace"]["batch_events"],
        json!(["absolute_virtual_desktop_move", "button_only_left_down"])
    );
    assert_eq!(
        delivery["input_trace"]["pre_batch"]["async_button_down"],
        false
    );
    assert_eq!(delivery["input_trace"]["cleanup"], Value::Null);
}

#[cfg(windows)]
#[test]
fn combined_down_drag_success_trace_keeps_pre_after_and_post_fence_evidence() {
    let sequence = run_combined_down_drag_sequence!(
        || Ok::<_, String>(exact_raw_input_snapshot(false)),
        |snapshot| {
            !snapshot.async_button_down
                && snapshot.foreground_relation
                    == dcc_cua_platform_windows::WindowsForegroundRelation::ExactTarget
        },
        CombinedDownInjection::accepted,
        |_| Ok(exact_raw_input_snapshot(true)),
        dcc_cua_platform_windows::WindowsRawInputSnapshot::allows_drag_path,
        || Ok(()),
        |_| {},
        SingleInputInjection::accepted,
        || {
            Ok(WindowsPostButtonUpSnapshot::new(
                false,
                Some(exact_raw_input_snapshot(false)),
                None,
            ))
        },
        |snapshot| !snapshot.async_button_down,
    );
    let outcome = windows_combined_raw_drag_outcome(sequence);
    let delivery = windows_raw_drag_delivery(&outcome, &test_window_target());

    assert_eq!(delivery["api_accepted"], true);
    assert_eq!(delivery["release_succeeded"], true);
    assert_eq!(
        delivery["input_trace"]["after_down"]["async_button_down"],
        true
    );
    assert_eq!(
        delivery["input_trace"]["cleanup"]["post_up"]["async_button_down"],
        false
    );
    assert_eq!(
        delivery["input_trace"]["cleanup"]["send_input"],
        json!({"requested": 1, "inserted": 1})
    );
}

#[cfg(windows)]
#[test]
fn combined_down_drag_delivery_preserves_primary_path_and_cleanup_failures() {
    let mut sequence = run_combined_down_drag_sequence!(
        || Ok::<_, String>(exact_raw_input_snapshot(false)),
        |snapshot| {
            !snapshot.async_button_down
                && snapshot.foreground_relation
                    == dcc_cua_platform_windows::WindowsForegroundRelation::ExactTarget
        },
        CombinedDownInjection::accepted,
        |_| Ok(exact_raw_input_snapshot(true)),
        dcc_cua_platform_windows::WindowsRawInputSnapshot::allows_drag_path,
        || Err("waypoint target fence was lost".to_owned()),
        |_| {},
        || SingleInputInjection::incomplete(0, "LEFTUP inserted 0/1".to_owned()),
        || {
            Ok(WindowsPostButtonUpSnapshot::new(
                true,
                Some(exact_raw_input_snapshot(true)),
                None,
            ))
        },
        |snapshot| !snapshot.async_button_down,
    );
    sequence.path_started = true;
    sequence.path_moves_inserted = 1;
    sequence.waypoints_completed = 1;
    sequence.total_waypoints = 3;
    let outcome = windows_combined_raw_drag_outcome(sequence);
    let delivery = windows_raw_drag_delivery(&outcome, &test_window_target());

    assert_eq!(delivery["api_accepted"], false);
    assert_eq!(delivery["path_sent"], false);
    assert_eq!(delivery["failure_phase"], "absolute_path");
    assert_eq!(delivery["release_succeeded"], false);
    assert_eq!(delivery["cleanup_error"], "LEFTUP inserted 0/1");
    assert_eq!(
        delivery["input_trace"]["failure_detail"],
        "waypoint target fence was lost"
    );
    assert_eq!(
        delivery["input_trace"]["cleanup"]["failure_phase"],
        "button_up_injection"
    );
    assert_eq!(
        delivery["input_trace"]["cleanup"]["injection_error"],
        "LEFTUP inserted 0/1"
    );
    assert_eq!(
        delivery["input_trace"]["cleanup"]["post_up"]["async_button_down"],
        true
    );
    assert_eq!(
        delivery["input_trace"]["combined_path"],
        json!({
            "path_started": true,
            "path_moves_inserted": 1,
            "waypoints_completed": 1,
            "total_waypoints": 3,
            "complete": false,
        })
    );
}

#[cfg(windows)]
#[test]
fn combined_down_drag_delivery_uses_the_live_pre_batch_foreground_fence() {
    let mut foreign = exact_raw_input_snapshot(false);
    foreign.foreground_relation =
        dcc_cua_platform_windows::WindowsForegroundRelation::ForeignProcess;
    let sequence = run_combined_down_drag_sequence!(
        || Ok::<_, String>(foreign),
        |snapshot| {
            !snapshot.async_button_down
                && snapshot.foreground_relation
                    == dcc_cua_platform_windows::WindowsForegroundRelation::ExactTarget
        },
        || panic!("foreign foreground must block the ordered batch"),
        |_| -> Result<dcc_cua_platform_windows::WindowsRawInputSnapshot, String> {
            panic!("foreign foreground must block after-DOWN sampling")
        },
        |_| panic!("foreign foreground cannot authorize a path"),
        || panic!("foreign foreground must block movement"),
        |_| panic!("foreign foreground must inject no input to settle"),
        || -> SingleInputInjection<String> { panic!("foreign foreground owns no button") },
        || -> Result<WindowsPostButtonUpSnapshot, String> {
            panic!("foreign foreground owns no button to sample")
        },
        |_| panic!("foreign foreground has no release evidence"),
    );
    let outcome = windows_combined_raw_drag_outcome(sequence);
    let delivery = windows_raw_drag_delivery(&outcome, &test_window_target());

    assert_eq!(delivery["input_sent"], false);
    assert_eq!(delivery["failure_phase"], "pre_batch_fence");
    assert_eq!(delivery["target_fence"]["foreground_verified"], false);
    assert_eq!(
        delivery["input_trace"]["pre_batch"]["foreground_relation"],
        "foreign_process"
    );
}

#[cfg(windows)]
#[test]
fn raw_drag_delivery_is_unverified_and_carries_typed_after_down_trace() {
    use dcc_cua_platform_windows::{
        WindowsForegroundRelation, WindowsRawInputSnapshot, WindowsWindowIdentity,
    };

    let trace = WindowsRawDragInputTrace::new(
        "left",
        1,
        WindowsRawInputSnapshot {
            async_button_down: true,
            target: WindowsWindowIdentity {
                window_handle: 0x1234,
                process_id: 77,
            },
            foreground: Some(WindowsWindowIdentity {
                window_handle: 0x1234,
                process_id: 77,
            }),
            foreground_relation: WindowsForegroundRelation::ExactTarget,
            target_thread_capture: None,
            capture_query_succeeded: true,
            capture_owned_by_target_process: false,
        },
    );

    let outcome = RawDragSequenceOutcome::<_, String> {
        trace,
        path_sent: true,
        release_error: None,
    };
    let delivery = windows_raw_drag_delivery(&outcome, &test_window_target());
    assert_eq!(delivery["confirmed"], false);
    assert_eq!(delivery["backend_id"], WINDOWS_SEND_INPUT_BACKEND_ID);
    assert_eq!(delivery["api_accepted"], true);
    assert_eq!(delivery["consumer_effect_confirmed"], false);
    assert_eq!(delivery["completion_known"], false);
    assert_eq!(delivery["verification_required"], true);
    assert_eq!(delivery["input_sent"], true);
    assert_eq!(delivery["input_trace"]["schema_version"], 1);
    assert_eq!(delivery["input_trace"]["backend"], "windows_send_input");
    assert_eq!(
        delivery["input_trace"]["send_input"],
        json!({"requested": 1, "inserted": 1})
    );
    assert_eq!(
        delivery["input_trace"]["after_down"]["foreground_relation"],
        "exact_target"
    );
    assert_eq!(
        delivery["input_trace"]["after_down"]["async_button_down"],
        true
    );
    assert_eq!(
        delivery["target_fence"],
        json!({
            "process_id": 42,
            "window_handle": 7,
            "exact_window": true,
            "foreground_required": true,
            "foreground_verified": true
        })
    );
}

#[cfg(windows)]
#[test]
fn rejected_raw_drag_delivery_keeps_trace_and_reports_no_path_delivery() {
    use dcc_cua_platform_windows::{
        WindowsForegroundRelation, WindowsRawInputSnapshot, WindowsWindowIdentity,
    };

    let outcome = RawDragSequenceOutcome::<_, String> {
        trace: WindowsRawDragInputTrace::new(
            "left",
            1,
            WindowsRawInputSnapshot {
                async_button_down: false,
                target: WindowsWindowIdentity {
                    window_handle: 0x1234,
                    process_id: 77,
                },
                foreground: Some(WindowsWindowIdentity {
                    window_handle: 0x1234,
                    process_id: 77,
                }),
                foreground_relation: WindowsForegroundRelation::ExactTarget,
                target_thread_capture: None,
                capture_query_succeeded: true,
                capture_owned_by_target_process: false,
            },
        ),
        path_sent: false,
        release_error: None,
    };

    let delivery = windows_raw_drag_delivery(&outcome, &test_window_target());
    assert_eq!(delivery["delivered"], false);
    assert_eq!(delivery["path_sent"], false);
    assert_eq!(delivery["release_succeeded"], true);
    assert_eq!(delivery["failure_phase"], "after_button_down_probe");
    assert_eq!(delivery["input_trace"]["send_input"]["inserted"], 1);
    assert_eq!(
        delivery["input_trace"]["after_down"]["async_button_down"],
        false
    );
}

#[cfg(windows)]
#[test]
fn rejected_relative_drag_is_typed_unverified_and_never_falls_back() {
    use dcc_cua_platform_windows::{
        WindowsForegroundRelation, WindowsRawInputSnapshot, WindowsWindowIdentity,
    };

    let trace = WindowsRawDragInputTrace::new(
        "left",
        1,
        WindowsRawInputSnapshot {
            async_button_down: true,
            target: WindowsWindowIdentity {
                window_handle: 0x1234,
                process_id: 77,
            },
            foreground: Some(WindowsWindowIdentity {
                window_handle: 0x1234,
                process_id: 77,
            }),
            foreground_relation: WindowsForegroundRelation::ExactTarget,
            target_thread_capture: None,
            capture_query_succeeded: true,
            capture_owned_by_target_process: false,
        },
    )
    .with_relative_path(json!({
        "schema_version": 1,
        "backend": "windows_send_input_relative",
        "endpoint_reached": false,
        "failure": "endpoint_not_reached",
        "moves": [{"requested": 1, "inserted": 1}],
    }));
    let outcome = RawDragSequenceOutcome::<_, String> {
        trace,
        path_sent: false,
        release_error: None,
    };

    let delivery = windows_raw_drag_delivery(&outcome, &test_window_target());
    assert_eq!(
        delivery["backend_id"],
        WINDOWS_RELATIVE_SEND_INPUT_BACKEND_ID
    );
    assert_eq!(delivery["api_accepted"], false);
    assert_eq!(delivery["consumer_effect_confirmed"], false);
    assert_eq!(delivery["verification_required"], true);
    assert_eq!(delivery["fallback_attempted"], false);
    assert_eq!(delivery["failure_phase"], "relative_path_calibration");
    assert_eq!(
        delivery["input_trace"]["relative_path"]["failure"],
        "endpoint_not_reached"
    );
    assert_eq!(delivery["release_succeeded"], true);
}

#[rstest]
#[case(None, LiveObservationStartDisposition::StartedNew)]
#[case(Some(false), LiveObservationStartDisposition::StartedNew)]
#[case(Some(true), LiveObservationStartDisposition::ReuseExisting)]
fn live_observation_restart_ownership_follows_active_state(
    #[case] active: Option<bool>,
    #[case] expected: LiveObservationStartDisposition,
) {
    let state = active.map(|active| json!({"active": active}));
    assert_eq!(live_observation_start_disposition(state.as_ref()), expected);
}

#[rstest]
fn terminal_live_observation_stream_is_never_reused() {
    let state = json!({
        "active": true,
        "terminal_reason": {
            "code": "capture_failed",
            "message": "persistent WGC worker failed",
        },
    });

    assert_eq!(
        live_observation_start_disposition(Some(&state)),
        LiveObservationStartDisposition::StartedNew
    );
}

#[tokio::test]
async fn live_observation_reuse_rechecks_the_desktop_and_exact_target() {
    let reusable = json!({"active": true, "terminal_reason": null});
    let active_unknown_desktop =
        windows_diagnostic(Ok(0), Err("OpenInputDesktop: access denied"), Ok(()), false);
    let target_checks = Cell::new(0_u32);

    let (disposition, target) = preflight_live_observation_start(
        Some(&reusable),
        require_exact_window_observation_from(&active_unknown_desktop),
        || {
            target_checks.set(target_checks.get() + 1);
            async { Ok::<_, ComputerUseError>(77_u64) }
        },
    )
    .await
    .expect("active WTS plus unreadable InputDesktop may reuse after exact target validation");
    assert_eq!(disposition, LiveObservationStartDisposition::ReuseExisting);
    assert_eq!(target, 77);
    assert_eq!(target_checks.get(), 1);

    for denied_diagnostic in [
        windows_diagnostic(Ok(4), Ok(Some("Default")), Ok(()), false),
        windows_diagnostic(Ok(0), Ok(Some("Winlogon")), Ok(()), false),
    ] {
        let denied_target_checks = Cell::new(0_u32);
        let error = preflight_live_observation_start(
            Some(&reusable),
            require_exact_window_observation_from(&denied_diagnostic),
            || {
                denied_target_checks.set(denied_target_checks.get() + 1);
                async { Ok::<_, ComputerUseError>(77_u64) }
            },
        )
        .await
        .expect_err("disconnected or secure desktops must reject reuse");
        assert_eq!(
            error.code,
            ComputerUseErrorCode::InteractiveDesktopUnavailable
        );
        assert_eq!(denied_target_checks.get(), 0);
    }

    let target_error = preflight_live_observation_start(
        Some(&reusable),
        require_exact_window_observation_from(&active_unknown_desktop),
        || async {
            Err::<u64, _>(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                "the exact target identity changed",
            ))
        },
    )
    .await
    .expect_err("reuse must propagate exact target revalidation failure");
    assert_eq!(target_error.code, ComputerUseErrorCode::InvalidTarget);
}

#[rstest]
#[case("browser_navigate", BannerActivity::Navigating)]
#[case("get_browser_state", BannerActivity::Observing)]
#[case("browser_type", BannerActivity::Operating)]
#[case("unknown_extension", BannerActivity::Operating)]
fn opaque_input_tools_never_claim_a_specific_injection_state(
    #[case] name: &str,
    #[case] expected: BannerActivity,
) {
    assert_eq!(banner_activity_for_bound_tool(name), expected);
}

#[tokio::test]
async fn cursor_input_gate_blocks_only_real_pointer_movement_before_backend_execution() {
    let move_called = Cell::new(false);
    let denied = || {
        Err(ComputerUseError::new(
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
            "Windows input surface is unavailable",
        ))
    };
    let move_error = gated_cursor_operation(true, denied, || {
        move_called.set(true);
        async { Ok::<_, ComputerUseError>(()) }
    })
    .await
    .expect_err("move_cursor must stop at the input gate");
    assert_eq!(
        move_error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert!(!move_called.get());

    let visual_called = Cell::new(false);
    gated_cursor_operation(false, denied, || {
        visual_called.set(true);
        async { Ok::<_, ComputerUseError>(()) }
    })
    .await
    .expect("cursor marker/theme/state tools remain available without raw input");
    assert!(visual_called.get());
}

#[rstest]
#[case(
    IndicatorError::InvalidTarget("gone".into()),
    ComputerUseErrorCode::InvalidTarget
)]
#[case(
    IndicatorError::Backend("paint failed".into()),
    ComputerUseErrorCode::BackendUnavailable
)]
fn indicator_errors_keep_typed_core_failure_semantics(
    #[case] error: IndicatorError,
    #[case] expected: ComputerUseErrorCode,
) {
    assert_eq!(map_indicator_error("start banner", error).code, expected);
}

#[test]
fn banner_debug_state_is_attached_without_hiding_upstream_session_state() {
    let state = attach_banner_status(
        json!({"active": true, "capture_scope": "window"}),
        json!({
            "activity": "observing",
            "recording": true,
            "motion": {
                "requested": "auto",
                "resolved": "reduce",
                "motion_enabled": false,
                "source": "system_preference"
            }
        }),
    );

    assert_eq!(state["active"], true);
    assert_eq!(state["capture_scope"], "window");
    assert_eq!(state["banner"]["activity"], "observing");
    assert_eq!(state["banner"]["recording"], true);
    assert_eq!(state["banner"]["motion"]["requested"], "auto");
    assert_eq!(state["banner"]["motion"]["resolved"], "reduce");
    assert_eq!(state["banner"]["motion"]["motion_enabled"], false);
    assert_eq!(state["banner"]["motion"]["source"], "system_preference");
}

#[test]
fn banner_debug_state_wraps_non_object_upstream_values() {
    let state = attach_banner_status(json!(null), json!({"activity": "ready"}));

    assert_eq!(state["cua"], Value::Null);
    assert_eq!(state["banner"]["activity"], "ready");
}

#[test]
fn bootstrap_activation_evidence_reports_the_resolved_indicator_motion() {
    let activation = attach_indicator_motion_to_activation(
        json!({"success": true, "target": {"pid": 42, "window_id": 31337}}),
        &json!({
            "motion": {
                "requested": "animate",
                "resolved": "animate",
                "motion_enabled": true,
                "source": "session_override"
            }
        }),
    );

    assert_eq!(activation["indicator_motion"]["requested"], "animate");
    assert_eq!(activation["indicator_motion"]["resolved"], "animate");
    assert_eq!(activation["indicator_motion"]["motion_enabled"], true);
    assert_eq!(activation["indicator_motion"]["source"], "session_override");
}

#[cfg(windows)]
#[rstest]
fn windows_fast_route_is_bounded_to_foreground_raw_actions() {
    assert!(uses_windows_foreground_fast_path(&ComputerUseAction {
        action: "click".into(),
        delivery_mode: Some("foreground".into()),
        ..Default::default()
    }));
    assert!(!uses_windows_foreground_fast_path(&ComputerUseAction {
        action: "click".into(),
        delivery_mode: Some("background".into()),
        ..Default::default()
    }));
    assert!(!uses_windows_foreground_fast_path(&ComputerUseAction {
        action: "type".into(),
        delivery_mode: Some("foreground".into()),
        ..Default::default()
    }));
    assert!(uses_windows_foreground_fast_path(&ComputerUseAction {
        action: "keypress".into(),
        delivery_mode: Some("foreground".into()),
        ..Default::default()
    }));
}

#[rstest]
fn post_click_focus_loss_is_sent_but_never_retry_safe() {
    let outcome = windows_post_input_focus_loss(
        "foreground_unavailable: exact target HWND 0x3b61b48 or a verified same-process \
         post-action window was not foreground after the click \
         (actual foreground HWND 0x2792020)",
    )
    .expect("post-input focus loss must be classified");

    assert_eq!(
        outcome.actual_foreground_window.as_deref(),
        Some("0x2792020")
    );
    assert!(outcome.input_sent);
    assert!(!outcome.delivery_confirmed);
    assert!(!outcome.retry_safe);
    assert!(outcome.verification_required);
    assert_eq!(outcome.effect, "unverifiable");
}

#[rstest]
fn pre_input_activation_failure_remains_a_hard_error() {
    assert!(
        windows_post_input_focus_loss(
            "foreground_unavailable: Windows did not activate exact target HWND 0x3b61b48"
        )
        .is_none()
    );
}

#[cfg(windows)]
#[rstest]
#[case(
    "target_minimized: exact activation target must be visible",
    ComputerUseErrorCode::TargetMinimized
)]
#[case(
    "the exact activation target no longer exists or belongs to the granted process",
    ComputerUseErrorCode::TargetUnavailable
)]
fn window_mutation_identity_fences_keep_typed_target_failures(
    #[case] message: &str,
    #[case] expected: ComputerUseErrorCode,
) {
    let error = map_windows_window_mutation_error(
        "activate exact target",
        dcc_cua_platform_windows::UiaError::InvalidTarget(message.into()),
    );

    assert_eq!(error.code, expected);
}

#[cfg(windows)]
#[test]
fn late_synthetic_touch_input_gate_stays_typed_and_never_injects() {
    let injection_calls = Cell::new(0_u8);
    let error = windows_synthetic_touch_attempt(
        Err(ComputerUseError::new(
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
            "input_gate_stage=synthetic_touch_activation: secure desktop",
        )),
        || {
            injection_calls.set(injection_calls.get() + 1);
            Ok(())
        },
        &test_window_target(),
    )
    .expect_err("a late input gate must remain a typed hard error");

    assert_eq!(
        error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert_eq!(injection_calls.get(), 0);
}

#[rstest]
fn held_coordinate_click_reuses_the_drag_contract() {
    let drag = held_coordinate_click_as_drag(&ComputerUseAction {
        action: "click".into(),
        x: Some(12.0),
        y: Some(34.0),
        duration_ms: Some(320),
        ..Default::default()
    })
    .expect("held click becomes a stationary drag");

    assert_eq!(drag.action, "drag");
    assert_eq!(drag.path, vec![ComputerUsePoint { x: 12.0, y: 34.0 }; 2]);
    assert_eq!(drag.duration_ms, Some(320));
    assert_eq!(drag.steps, Some(20));
}

#[rstest]
fn host_runtime_uses_the_upstream_cursor_renderer_only_where_it_can_run() {
    let options = driver_host_options();
    assert_eq!(
        UPSTREAM_CURSOR_RENDERER_ENABLED,
        cfg!(any(windows, target_os = "linux"))
    );
    assert_eq!(options.cursor.enabled, UPSTREAM_CURSOR_RENDERER_ENABLED);
    assert!(options.host_owns_permission_ux);
    assert!(options.prepare_desktop_environment);
}

#[rstest]
fn bundled_cursor_theme_matches_runtime_contract() {
    let theme = cursor_overlay::decode_theme(BUNDLED_CURSOR_THEME).expect("valid bundled theme");
    assert_eq!(theme.id, MOUSE_CURSOR_THEME);
    assert_eq!(theme.actions.len(), 12);
}

#[rstest]
fn snapshot_bounds_use_agent_defaults_and_cap_context() {
    assert_eq!(bounded_snapshot_elements(0), DEFAULT_SNAPSHOT_MAX_ELEMENTS);
    assert_eq!(bounded_snapshot_depth(0), DEFAULT_SNAPSHOT_MAX_DEPTH);
    assert_eq!(bounded_snapshot_elements(u32::MAX), MAX_SNAPSHOT_ELEMENTS);
    assert_eq!(bounded_snapshot_depth(u32::MAX), MAX_SNAPSHOT_DEPTH);
}

#[rstest]
fn diagnostics_prefer_upstream_structured_content() {
    let check = diagnostic_tool_check(Ok(ComputerUseToolResult {
        value: json!({"structuredContent":{"overall":"ok"}}),
        text: "healthy".into(),
        images: Vec::new(),
        degraded: false,
    }));
    assert_eq!(check["success"], true);
    assert_eq!(check["result"]["overall"], "ok");
    assert_eq!(check["summary"], "healthy");

    let failed = diagnostic_tool_check(Err(ComputerUseError::new(
        ComputerUseErrorCode::BackendUnavailable,
        "screen capture unavailable",
    )));
    assert_eq!(failed["success"], false);
    assert_eq!(failed["code"], "backend_unavailable");
}

#[rstest]
fn platform_managed_desktop_preserves_the_portable_input_contract() {
    let diagnostic = platform_managed_diagnostic();

    assert!(require_input_available_from(&diagnostic).is_ok());
    assert_eq!(diagnostic["input_ready"], true);
}

#[rstest]
fn active_default_input_desktop_without_foreground_is_ready() {
    let diagnostic = windows_diagnostic(Ok(0), Ok(Some("Default")), Ok(()), false);

    assert_eq!(diagnostic["success"], true);
    assert_eq!(diagnostic["code"], "interactive_desktop_ready");
    assert_eq!(diagnostic["observation_ready"], true);
    assert_eq!(diagnostic["input_ready"], true);
    assert_eq!(diagnostic["input_surface_ready"], true);
    assert_eq!(diagnostic["input_desktop"], "Default");
    assert_eq!(diagnostic["foreground_present"], false);
    assert!(
        diagnostic["message"]
            .as_str()
            .expect("diagnostic message")
            .contains("no foreground window")
    );
}

#[rstest]
fn unreadable_input_surface_blocks_input_without_stopping_observation() {
    let diagnostic = windows_diagnostic(
        Ok(0),
        Ok(Some("Default")),
        Err("GetCursorPos failed: Access is denied. (os error 5)"),
        false,
    );

    assert_eq!(diagnostic["success"], true);
    assert_eq!(diagnostic["code"], "interactive_desktop_ready");
    assert_eq!(diagnostic["observation_ready"], true);
    assert_eq!(diagnostic["input_ready"], false);
    assert_eq!(
        diagnostic["input_code"],
        "interactive_input_surface_unavailable"
    );
    assert_eq!(diagnostic["input_desktop"], "Default");
    assert_eq!(diagnostic["input_surface_ready"], false);
    assert_eq!(
        diagnostic["input_surface_error"],
        "GetCursorPos failed: Access is denied. (os error 5)"
    );
    assert_eq!(diagnostic["foreground_present"], false);
}

#[rstest]
fn active_secure_input_desktop_fails_closed() {
    let diagnostic = windows_diagnostic(Ok(0), Ok(Some("Winlogon")), Ok(()), false);

    assert_eq!(diagnostic["success"], false);
    assert_eq!(diagnostic["code"], "interactive_desktop_unavailable");
    assert_eq!(diagnostic["observation_ready"], false);
    assert_eq!(diagnostic["input_ready"], false);
    assert_eq!(diagnostic["input_desktop"], "Winlogon");
    assert_eq!(diagnostic["foreground_present"], false);
}

#[rstest]
fn input_desktop_probe_error_fails_closed() {
    let diagnostic =
        windows_diagnostic(Ok(0), Err("OpenInputDesktop: access denied"), Ok(()), false);

    assert_eq!(diagnostic["success"], false);
    assert_eq!(diagnostic["observation_ready"], true);
    assert_eq!(diagnostic["input_ready"], false);
    assert_eq!(diagnostic["code"], "interactive_desktop_unknown");
    assert_eq!(diagnostic["input_desktop"], serde_json::Value::Null);
    assert_eq!(
        diagnostic["input_desktop_error"],
        "OpenInputDesktop: access denied"
    );
    assert_eq!(diagnostic["foreground_present"], false);
}

#[rstest]
fn unreadable_input_desktop_only_allows_an_exact_window_observation_attempt() {
    let diagnostic =
        windows_diagnostic(Ok(0), Err("OpenInputDesktop: access denied"), Ok(()), false);

    assert!(require_exact_window_observation_from(&diagnostic).is_ok());
    let desktop_error = require_desktop_observation_from(&diagnostic)
        .expect_err("desktop observation must retain the readable Default-desktop gate");
    assert_eq!(
        desktop_error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
}

#[rstest]
fn missing_input_desktop_identity_fails_closed() {
    let diagnostic = windows_diagnostic(Ok(0), Ok(None), Ok(()), false);

    assert_eq!(diagnostic["success"], false);
    assert_eq!(diagnostic["code"], "interactive_desktop_unknown");
    assert_eq!(diagnostic["input_desktop"], serde_json::Value::Null);
    assert_eq!(diagnostic["foreground_present"], false);
}

#[rstest]
#[case(Ok(0), true, true, "interactive_desktop_ready", "active")]
#[case(Ok(0), false, true, "interactive_desktop_ready", "active")]
#[case(Ok(4), true, false, "interactive_session_not_active", "disconnected")]
#[case(Ok(1), true, false, "interactive_session_not_active", "not_active")]
fn windows_session_state_fences_raw_input(
    #[case] state: Result<i32, String>,
    #[case] foreground: bool,
    #[case] success: bool,
    #[case] code: &str,
    #[case] state_name: &str,
) {
    let diagnostic = windows_diagnostic(state, Ok(Some("Default")), Ok(()), foreground);
    assert_eq!(diagnostic["success"], success);
    assert_eq!(diagnostic["code"], code);
    assert_eq!(diagnostic["observation_ready"], success);
    assert_eq!(diagnostic["input_ready"], success);
    assert_eq!(diagnostic["session_state"], state_name);
    assert_eq!(diagnostic["input_desktop"], "Default");
    assert_eq!(diagnostic["foreground_present"], foreground);
}

#[rstest]
fn windows_session_query_failure_is_not_ready() {
    let diagnostic = windows_diagnostic(
        Err("access denied".into()),
        Ok(Some("Default")),
        Ok(()),
        true,
    );
    assert_eq!(diagnostic["success"], false);
    assert_eq!(diagnostic["code"], "interactive_session_unknown");
    assert_eq!(diagnostic["input_desktop"], "Default");
    assert_eq!(diagnostic["foreground_present"], true);
    assert!(
        diagnostic["message"]
            .as_str()
            .unwrap()
            .contains("access denied")
    );
}

#[rstest]
fn scope_requires_exact_identity_and_action_rejects_unbounded_text() {
    assert!(ComputerUseTargetScope::default().validate().is_err());
    assert!(
        ComputerUseTargetScope {
            window_title: Some(String::new()),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    let action = ComputerUseAction {
        action: "type".into(),
        text: Some("x".repeat(MAX_TEXT_UTF16_UNITS + 1)),
        ..Default::default()
    };
    assert_eq!(
        validate_action(&action).unwrap_err().code,
        ComputerUseErrorCode::InvalidAction
    );
}

#[rstest]
fn bootstrap_activation_requires_exact_pid_and_window_handle() {
    let request = ComputerUseSessionStartRequest {
        activate_before: true,
        indicator_motion: IndicatorMotionPolicy::Auto,
    };
    for scope in [
        ComputerUseTargetScope {
            process_id: Some(42),
            ..Default::default()
        },
        ComputerUseTargetScope {
            window_handle: Some(31337),
            ..Default::default()
        },
        ComputerUseTargetScope {
            window_title: Some("The Bazaar".into()),
            ..Default::default()
        },
    ] {
        let error = request.validate_for_scope(&scope).unwrap_err();
        assert_eq!(error.code, ComputerUseErrorCode::InvalidTarget);
        assert!(error.message.contains("exact process_id and window_handle"));
    }

    assert!(
        request
            .validate_for_scope(&ComputerUseTargetScope {
                process_id: Some(42),
                window_handle: Some(31337),
                window_title: None,
            })
            .is_ok()
    );
    assert!(
        ComputerUseSessionStartRequest::default()
            .validate_for_scope(&ComputerUseTargetScope {
                window_title: Some("The Bazaar".into()),
                ..Default::default()
            })
            .is_ok()
    );
}

#[rstest]
fn session_start_motion_policy_is_explicit_non_persistent_and_defaults_accessibly() {
    let default_request: ComputerUseSessionStartRequest =
        serde_json::from_value(json!({})).expect("default request");
    let animated_request: ComputerUseSessionStartRequest =
        serde_json::from_value(json!({"indicator_motion": "animate"})).expect("animated request");
    let next_request: ComputerUseSessionStartRequest =
        serde_json::from_value(json!({})).expect("next default request");

    assert_eq!(
        default_request.indicator_motion,
        IndicatorMotionPolicy::Auto
    );
    assert_eq!(
        animated_request.indicator_motion,
        IndicatorMotionPolicy::Animate
    );
    assert_eq!(next_request.indicator_motion, IndicatorMotionPolicy::Auto);
    assert_eq!(
        serde_json::to_value(animated_request).expect("request serializes")["indicator_motion"],
        "animate"
    );
    assert!(
        serde_json::from_value::<ComputerUseSessionStartRequest>(
            json!({"indicator_motion": "sometimes"})
        )
        .is_err()
    );
}

#[rstest]
fn window_frame_accepts_fractional_multi_monitor_coordinates() {
    assert!(
        ComputerUseWindowFrameRequest {
            x: -1919.5,
            y: 42.25,
            width: 1280.5,
            height: 720.25,
        }
        .validate()
        .is_ok()
    );
}

#[rstest]
fn native_menu_paths_match_the_upstream_contract_bounds() {
    assert!(
        ComputerUseMenuRequest {
            path: ["Window", "Arrange", "Left"].map(str::to_owned).to_vec(),
        }
        .validate()
        .is_ok()
    );
    for path in [
        Vec::new(),
        vec![" ".into()],
        vec!["x".repeat(201)],
        vec!["item".into(); 17],
    ] {
        assert_eq!(
            ComputerUseMenuRequest { path }.validate().unwrap_err().code,
            ComputerUseErrorCode::InvalidAction
        );
    }
}

#[rstest]
#[case(f64::NAN, 0.0, 100.0, 100.0)]
#[case(0.0, f64::INFINITY, 100.0, 100.0)]
#[case(0.0, 0.0, 0.0, 100.0)]
#[case(0.0, 0.0, 100.0, -1.0)]
fn invalid_window_frames_fail_before_cua(
    #[case] x: f64,
    #[case] y: f64,
    #[case] width: f64,
    #[case] height: f64,
) {
    let error = ComputerUseWindowFrameRequest {
        x,
        y,
        width,
        height,
    }
    .validate()
    .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::InvalidAction);
}

#[rstest]
fn native_target_policy_denies_terminal_processes() {
    let mut target = test_window_target();
    target.app_name = "powershell.exe".into();
    assert_eq!(
        validate_target_policy(&target).unwrap_err().code,
        ComputerUseErrorCode::InvalidTarget
    );
}

#[rstest]
fn generic_application_labels_resolve_from_the_exact_window() {
    let target = WindowTarget {
        pid: 42,
        window_id: 84,
        title: "The Bazaar".into(),
        app_name: "TheBazaar.exe".into(),
        bounds: [0, 0, 1280, 720],
        is_on_screen: true,
        is_minimized: false,
        z_index: Some(0),
        is_foreground: true,
    };

    assert_eq!(
        crate::runtime::resolved_application_name("Application", &target),
        "The Bazaar",
    );
    assert_eq!(
        crate::runtime::resolved_application_name("Maya 2024", &target),
        "Maya 2024",
        "an explicit profile/application identity must remain authoritative",
    );
}

#[rstest]
fn window_target_accepts_fractional_platform_bounds() {
    let target = WindowTarget::from_value(&json!({
        "pid": 42,
        "window_id": 7,
        "title": "DCC",
        "app_name": "dcc",
        "bounds": {"x": 120.5, "y": -0.5, "width": 940.4, "height": 779.6}
    }))
    .expect("macOS floating-point bounds should parse");
    assert_eq!(target.bounds, [121, -1, 940, 780]);
}

#[rstest]
fn verify_state_bounds_are_rejected_before_backend_call() {
    assert!(validate_verify_state_request(&json!([]), None, None).is_err());
    assert!(
        validate_verify_state_request(&json!([{"window": {"exists": true}}]), Some(10_001), None,)
            .is_err()
    );
    assert!(
        validate_verify_state_request(&json!([{"window": {"exists": true}}]), None, Some(6),)
            .is_err()
    );
    assert!(
            validate_verify_state_request(
                &json!([{"window": {"exists": true}}]),
                Some(1_000),
                Some(2),
            )
            .is_ok()
        );
}

#[rstest]
fn zoom_is_fenced_to_the_latest_observation_and_bounds() {
    let observation = ComputerUseObservation {
        observation_id: "obs-1".into(),
        window_handle: 7,
        process_id: 42,
        window_title: "DCC".into(),
        width: 1280,
        height: 720,
        source_rect: [0, 0, 1280, 720],
        capture_backend: "test".into(),
        capture_provenance: json!({}),
        session_id: "session".into(),
    };
    let valid = ComputerUseZoomRequest {
        observation_id: "obs-1".into(),
        x1: 10.0,
        y1: 20.0,
        x2: 400.0,
        y2: 200.0,
    };
    assert!(validate_zoom_request(&valid, &observation).is_ok());
    assert_eq!(
        validate_zoom_request(
            &ComputerUseZoomRequest {
                observation_id: "old".into(),
                ..valid.clone()
            },
            &observation,
        )
        .unwrap_err()
        .code,
        ComputerUseErrorCode::StaleObservation
    );
    assert!(
        validate_zoom_request(
            &ComputerUseZoomRequest {
                x2: 511.0,
                ..valid.clone()
            },
            &observation,
        )
        .is_err()
    );
    assert!(
        validate_zoom_request(
            &ComputerUseZoomRequest {
                x2: 1_281.0,
                ..valid
            },
            &observation,
        )
        .is_err()
    );
}

#[rstest]
fn native_provider_timeout_is_backend_unavailable() {
    let error = map_driver_error(
        "capture CUA window state",
        "InputFailed: get_window_state timed out (UIA provider unresponsive)",
    );
    assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
}

#[rstest]
fn tool_provider_timeout_is_backend_unavailable() {
    let result = cua_driver_sdk::ToolResult {
        is_error: true,
        error_code: Some("input_failed".into()),
        raw_json: "{}".into(),
        text: "get_window_state timed out: UIA provider unresponsive".into(),
        structured_json: None,
        images: Vec::new(),
        degraded: false,
        action: None,
        verification: None,
    };
    assert_eq!(
        ensure_tool_ok("capture CUA window", &result)
            .unwrap_err()
            .code,
        ComputerUseErrorCode::BackendUnavailable
    );
}

#[rstest]
#[case("target_minimized", ComputerUseErrorCode::TargetMinimized)]
#[case("target_unavailable", ComputerUseErrorCode::TargetUnavailable)]
#[case("missing_window", ComputerUseErrorCode::TargetUnavailable)]
#[case(
    "interactive_desktop_unavailable",
    ComputerUseErrorCode::InteractiveDesktopUnavailable
)]
#[case(
    "input_gate_stage=foreground_dispatch",
    ComputerUseErrorCode::InteractiveDesktopUnavailable
)]
fn exact_status_driver_markers_override_browser_and_uia_classification(
    #[case] marker: &str,
    #[case] expected: ComputerUseErrorCode,
) {
    let result = cua_driver_sdk::ToolResult {
        is_error: true,
        error_code: Some(marker.into()),
        raw_json: "{}".into(),
        text: "browser UIA operation rejected the exact target".into(),
        structured_json: None,
        images: Vec::new(),
        degraded: false,
        action: None,
        verification: None,
    };
    assert_eq!(
        ensure_tool_ok("perform browser operation", &result)
            .unwrap_err()
            .code,
        expected
    );
    assert_eq!(
        map_driver_error(
            "perform browser operation",
            format!("browser UIA failure: {marker}")
        )
        .code,
        expected
    );
}

#[tokio::test]
async fn denied_desktop_fallback_never_reaches_the_capture_backend() {
    let capture_called = Cell::new(false);
    let denied = Err(ComputerUseError::new(
        ComputerUseErrorCode::InteractiveDesktopUnavailable,
        "Windows input desktop could not be read",
    ));

    let error = gated_desktop_observation(denied, || {
        capture_called.set(true);
        async { Ok::<_, ComputerUseError>(()) }
    })
    .await
    .expect_err("desktop fallback must stop at its desktop-observation gate");

    assert_eq!(
        error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert!(!capture_called.get());
}

#[tokio::test]
async fn exact_observation_publication_rechecks_readiness_after_the_capture_await() {
    let gate_calls = Cell::new(0_u8);
    let capture_calls = Cell::new(0_u8);

    let error = gated_exact_window_observation(
        || {
            let call = gate_calls.get() + 1;
            gate_calls.set(call);
            if call == 1 {
                Ok(())
            } else {
                Err(ComputerUseError::new(
                    ComputerUseErrorCode::InteractiveDesktopUnavailable,
                    "workstation locked while capture was in flight",
                ))
            }
        },
        || async {
            capture_calls.set(capture_calls.get() + 1);
            Ok::<_, ComputerUseError>("captured evidence")
        },
    )
    .await
    .expect_err("post-lock evidence must not be published");

    assert_eq!(
        error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert_eq!(gate_calls.get(), 2);
    assert_eq!(capture_calls.get(), 1);
}

#[tokio::test]
async fn exact_observation_publication_rechecks_after_the_final_async_stage() {
    let gate_calls = Cell::new(0_u8);
    let publish_calls = Cell::new(0_u8);

    let error = gated_exact_window_publication(
        || {
            let call = gate_calls.get() + 1;
            gate_calls.set(call);
            if call < 3 {
                Ok(())
            } else {
                Err(ComputerUseError::new(
                    ComputerUseErrorCode::InteractiveDesktopUnavailable,
                    "the desktop locked during final image processing",
                ))
            }
        },
        || async { Ok::<_, ComputerUseError>("captured") },
        |captured| async move { Ok::<_, ComputerUseError>(format!("{captured}-encoded")) },
        |_| {
            publish_calls.set(publish_calls.get() + 1);
        },
    )
    .await
    .expect_err("the last readiness fence must run immediately before publication");

    assert_eq!(
        error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert_eq!(gate_calls.get(), 3);
    assert_eq!(publish_calls.get(), 0);
}

#[tokio::test]
async fn exact_observation_publication_rejects_a_target_minimized_during_finalization() {
    let gate_calls = Cell::new(0_u8);
    let publish_calls = Cell::new(0_u8);

    let error = gated_exact_window_publication(
        || {
            let call = gate_calls.get() + 1;
            gate_calls.set(call);
            if call < 3 {
                Ok(())
            } else {
                Err(ComputerUseError::new(
                    ComputerUseErrorCode::TargetMinimized,
                    "target_minimized: the exact target changed during finalization",
                ))
            }
        },
        || async { Ok::<_, ComputerUseError>("captured") },
        |captured| async move { Ok::<_, ComputerUseError>(format!("{captured}-finalized")) },
        |_| {
            publish_calls.set(publish_calls.get() + 1);
        },
    )
    .await
    .expect_err("a newly minimized target must not publish the completed frame");

    assert_eq!(error.code, ComputerUseErrorCode::TargetMinimized);
    assert_eq!(gate_calls.get(), 3);
    assert_eq!(publish_calls.get(), 0);
}

#[rstest]
fn desktop_fallback_crops_an_8_bit_rgba_png() {
    let mut source = Vec::new();
    let mut encoder = png::Encoder::new(&mut source, 4, 3);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(&(0..48).map(|value| value as u8).collect::<Vec<_>>())
        .unwrap();
    writer.finish().unwrap();

    let cropped = crop_png_to_bounds(&source, [1, 1, 2, 2]).unwrap();
    assert_eq!(png_dimensions(&cropped), Some((2, 2)));
    let mut reader = png::Decoder::new(Cursor::new(cropped)).read_info().unwrap();
    let mut bytes = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut bytes).unwrap();
    assert_eq!(
        &bytes[..info.buffer_size()],
        &(20..28).chain(36..44).map(|v| v as u8).collect::<Vec<_>>()
    );
}

#[rstest]
fn desktop_fallback_maps_per_monitor_dpi_bounds_to_capture_pixels() {
    assert_eq!(
        scale_bounds_for_dpi([1, 1, 3118, 1982], 192).unwrap(),
        [0, 0, 1560, 992]
    );
    assert_eq!(
        scale_bounds_for_dpi([10, 20, 300, 400], 96).unwrap(),
        [10, 20, 300, 400]
    );
}

#[rstest]
fn foreground_fallback_uses_the_highest_known_z_index() {
    let mut back = test_window_target();
    back.window_id = 1;
    back.z_index = Some(3);
    back.is_foreground = false;
    let mut front = test_window_target();
    front.window_id = 2;
    front.z_index = Some(7);
    front.is_foreground = false;
    let mut windows = vec![back, front];
    mark_foreground_by_z_index(&mut windows);
    assert!(!windows[0].is_foreground);
    assert!(windows[1].is_foreground);
}

#[rstest]
fn native_tool_boundary_rejects_reserved_and_dedicated_routes() {
    assert!(validate_native_tool_request("debug_window_info", &json!({})).is_ok());
    assert!(validate_native_tool_request("bad-name", &json!({})).is_err());
    assert!(validate_native_tool_request("debug_window_info", &json!({"_tool":"x"})).is_err());
    for name in cua_driver_contract::ACTION_RESULT_TOOLS {
        assert!(
            !native_tool_allowed_in_window_session(name),
            "canonical action tool {name} bypassed its dedicated route"
        );
    }
    assert!(!native_tool_allowed_in_window_session("list_windows"));
    assert!(!native_tool_allowed_in_window_session("get_browser_state"));
    assert!(!native_tool_allowed_in_window_session("page"));
    assert!(!native_tool_allowed_in_window_session("get_session_state"));
    assert!(!native_tool_allowed_in_window_session("browser_navigate"));
    assert!(native_tool_allowed_in_window_session("debug_window_info"));
    assert!(native_tool_allowed_globally("health_report"));
    assert!(native_tool_allowed_globally("get_accessibility_tree"));
    assert!(!native_tool_allowed_globally("launch_app"));
    assert!(validate_escalation_request("other", Some("reason")).is_ok());
    assert!(validate_escalation_request("unknown", None).is_err());
    assert!(cursor_tool_allowed("get_agent_cursor_state"));
    assert!(cursor_tool_allowed("move_cursor"));
}

#[rstest]
fn tool_schema_lookup_uses_exact_inventory_names() {
    let inventory = json!({
        "tools": [{
            "name": "debug_window_info",
            "inputSchema": {"type": "object"}
        }]
    });
    assert_eq!(
        tool_schema_from_inventory(&inventory, "debug_window_info").unwrap()["type"],
        "object"
    );
    assert!(tool_schema_from_inventory(&inventory, "debug_window_info_extra").is_err());
}

#[rstest]
fn window_cursor_move_is_bounded_to_window_scope() {
    let mut valid = serde_json::json!({"x": 10, "y": 20});
    let target = WindowTarget {
        pid: 7,
        window_id: 9,
        title: "target".into(),
        app_name: "app".into(),
        bounds: [100, 200, 800, 600],
        is_on_screen: true,
        is_minimized: false,
        z_index: Some(0),
        is_foreground: true,
    };
    map_window_cursor_move(valid.as_object_mut().unwrap(), &target).unwrap();
    assert_eq!(valid["x"], 110.0);
    assert_eq!(valid["y"], 220.0);
    assert_eq!(valid["scope"], "window");

    let desktop = serde_json::json!({"x": 10, "y": 20, "scope": "desktop"});
    assert!(validate_window_cursor_move(desktop.as_object().unwrap()).is_err());

    let negative = serde_json::json!({"x": -1, "y": 20});
    assert!(validate_window_cursor_move(negative.as_object().unwrap()).is_err());

    let mut outside = serde_json::json!({"x": 800, "y": 20});
    assert!(map_window_cursor_move(outside.as_object_mut().unwrap(), &target).is_err());
}

#[rstest]
fn png_dimensions_reads_the_png_header() {
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    data.extend_from_slice(&[0; 8]);
    data.extend_from_slice(&1280_u32.to_be_bytes());
    data.extend_from_slice(&720_u32.to_be_bytes());
    assert_eq!(png_dimensions(&data), Some((1280, 720)));
}

#[rstest]
fn semantic_element_actions_replace_pixel_coordinates() {
    let action = ComputerUseAction {
        action: "click".into(),
        element_index: Some(7),
        x: Some(12.0),
        y: Some(13.0),
        ..Default::default()
    };
    let args = action_arguments(
        &action,
        "session",
        &WindowTarget {
            pid: 42,
            window_id: 7,
            title: "DCC".into(),
            app_name: "dcc".into(),
            bounds: [0, 0, 100, 100],
            is_on_screen: true,
            is_minimized: false,
            z_index: Some(1),
            is_foreground: true,
        },
    );
    assert_eq!(args["element_index"], 7);
    assert!(args.get("x").is_none());
    assert!(args.get("y").is_none());
}

#[rstest]
fn semantic_tokens_and_background_delivery_reach_cua() {
    let action = ComputerUseAction {
        action: "click".into(),
        element_index: Some(7),
        element_token: Some("element-token".into()),
        delivery_mode: Some("background".into()),
        x: Some(12.0),
        y: Some(13.0),
        ..Default::default()
    };
    let args = action_arguments(
        &action,
        "session",
        &WindowTarget {
            pid: 42,
            window_id: 7,
            title: "DCC".into(),
            app_name: "dcc".into(),
            bounds: [0, 0, 100, 100],
            is_on_screen: true,
            is_minimized: false,
            z_index: Some(1),
            is_foreground: true,
        },
    );
    assert_eq!(args["element_token"], "element-token");
    assert!(args.get("element_index").is_none());
    assert_eq!(args["delivery_mode"], "background");
    assert!(args.get("x").is_none());
    assert!(args.get("y").is_none());
}

#[rstest]
fn windows_uia_tokens_route_only_supported_semantic_actions() {
    let observation = ComputerUseObservation {
        observation_id: "observation".into(),
        window_handle: 7,
        process_id: 42,
        window_title: "DCC".into(),
        width: 100,
        height: 100,
        source_rect: [0, 0, 100, 100],
        capture_backend: "native".into(),
        capture_provenance: json!({"accessibility_backend":"windows_uia"}),
        session_id: "session".into(),
    };
    assert!(is_windows_uia_semantic_action(
        &ComputerUseAction {
            action: "click".into(),
            element_token: Some("dcc-wuia:snapshot:2".into()),
            ..Default::default()
        },
        &observation,
    ));
    assert!(is_windows_uia_semantic_action(
        &ComputerUseAction {
            action: "set_value".into(),
            element_index: Some(2),
            ..Default::default()
        },
        &observation,
    ));
    assert!(!is_windows_uia_semantic_action(
        &ComputerUseAction {
            action: "keypress".into(),
            element_token: Some("dcc-wuia:snapshot:2".into()),
            ..Default::default()
        },
        &observation,
    ));
}

#[rstest]
fn semantic_only_observations_reject_unscoped_pixel_actions() {
    let observation = ComputerUseObservation {
        observation_id: "observation".into(),
        window_handle: 7,
        process_id: 42,
        window_title: "DCC".into(),
        width: 100,
        height: 100,
        source_rect: [0, 0, 100, 100],
        capture_backend: "windows_uia".into(),
        capture_provenance: json!({"pixels_captured":false}),
        session_id: "session".into(),
    };
    let error = validate_action_observation(
        &ComputerUseAction {
            action: "click".into(),
            x: Some(10.0),
            y: Some(20.0),
            ..Default::default()
        },
        &observation,
    )
    .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
    assert!(
        validate_action_observation(
            &ComputerUseAction {
                action: "click".into(),
                element_index: Some(2),
                ..Default::default()
            },
            &observation,
        )
        .is_ok()
    );
}

#[rstest]
fn action_rejects_unknown_delivery_mode_and_unbounded_token() {
    assert_eq!(
        validate_action(&ComputerUseAction {
            action: "click".into(),
            delivery_mode: Some("guess".into()),
            ..Default::default()
        })
        .unwrap_err()
        .code,
        ComputerUseErrorCode::InvalidAction
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "click".into(),
            element_token: Some("x".repeat(MAX_ELEMENT_TOKEN_CHARS + 1)),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "drag".into(),
            button: Some("side".into()),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "keypress".into(),
            modifiers: vec!["x".repeat(MAX_MODIFIER_CHARS + 1)],
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "drag".into(),
            duration_ms: Some(10_001),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "click".into(),
            x: Some(10.0),
            y: Some(20.0),
            duration_ms: Some(100),
            ..Default::default()
        })
        .is_ok()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "click".into(),
            element_index: Some(1),
            duration_ms: Some(100),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_action(&ComputerUseAction {
            action: "drag".into(),
            steps: Some(201),
            ..Default::default()
        })
        .is_err()
    );
}

#[test]
fn action_input_backend_id_is_a_bounded_wire_identifier() {
    let action: ComputerUseAction = serde_json::from_value(json!({
        "action": "drag",
        "input_backend_id": "windows.synthetic_touch.v1",
        "delivery_mode": "foreground",
        "path": [{"x": 1.0, "y": 2.0}, {"x": 3.0, "y": 4.0}]
    }))
    .unwrap();
    assert_eq!(
        action.input_backend_id.as_deref(),
        Some("windows.synthetic_touch.v1")
    );
    validate_action(&action).unwrap();

    for invalid in [
        "",
        ".windows.send_input.v1",
        "windows/send_input/v1",
        "WINDOWS.send_input.v1",
    ] {
        assert!(
            validate_action(&ComputerUseAction {
                action: "drag".into(),
                input_backend_id: Some(invalid.into()),
                delivery_mode: Some("foreground".into()),
                path: vec![
                    ComputerUsePoint { x: 1.0, y: 2.0 },
                    ComputerUsePoint { x: 3.0, y: 4.0 },
                ],
                ..Default::default()
            })
            .is_err(),
            "accepted invalid input backend id {invalid:?}"
        );
    }
    assert!(
        validate_action(&ComputerUseAction {
            action: "drag".into(),
            input_backend_id: Some("x".repeat(65)),
            delivery_mode: Some("foreground".into()),
            path: vec![
                ComputerUsePoint { x: 1.0, y: 2.0 },
                ComputerUsePoint { x: 3.0, y: 4.0 },
            ],
            ..Default::default()
        })
        .is_err()
    );
}

#[cfg(windows)]
#[test]
fn scoped_foreground_drag_without_an_explicit_backend_uses_the_combined_down_route() {
    let action: ComputerUseAction = serde_json::from_value(json!({
        "action": "drag",
        "delivery_mode": "foreground",
        "button": "left",
        "path": [{"x": 1.0, "y": 2.0}, {"x": 3.0, "y": 4.0}]
    }))
    .unwrap();
    validate_action(&action).unwrap();

    assert_eq!(
        select_windows_foreground_drag_backend(&action).unwrap(),
        WindowsForegroundDragBackend::CombinedDownDrag
    );
}

#[cfg(windows)]
#[test]
fn implicit_combined_down_drag_rejects_unsupported_shapes_without_legacy_fallback() {
    let action = || ComputerUseAction {
        action: "drag".into(),
        delivery_mode: Some("foreground".into()),
        path: vec![
            ComputerUsePoint { x: 1.0, y: 2.0 },
            ComputerUsePoint { x: 3.0, y: 4.0 },
        ],
        ..Default::default()
    };

    let mut right_button = action();
    right_button.button = Some("right".into());
    assert!(
        select_windows_foreground_drag_backend(&right_button)
            .unwrap_err()
            .contains("left-button")
    );

    let mut modified = action();
    modified.modifiers = vec!["shift".into()];
    assert!(
        select_windows_foreground_drag_backend(&modified)
            .unwrap_err()
            .contains("modifiers")
    );

    let mut multi_segment = action();
    multi_segment.path.push(ComputerUsePoint { x: 5.0, y: 6.0 });
    assert!(
        select_windows_foreground_drag_backend(&multi_segment)
            .unwrap_err()
            .contains("exactly two path points")
    );
}

#[cfg(windows)]
#[test]
fn explicit_windows_drag_backend_selection_never_falls_back() {
    let action = |backend: Option<&str>, button: Option<&str>, delivery: &str| ComputerUseAction {
        action: "drag".into(),
        delivery_mode: Some(delivery.into()),
        input_backend_id: backend.map(str::to_owned),
        button: button.map(str::to_owned),
        path: vec![
            ComputerUsePoint { x: 1.0, y: 2.0 },
            ComputerUsePoint { x: 3.0, y: 4.0 },
        ],
        ..Default::default()
    };

    assert_eq!(
        select_windows_foreground_drag_backend(&action(
            Some(WINDOWS_SEND_INPUT_BACKEND_ID),
            None,
            "foreground"
        ))
        .unwrap(),
        WindowsForegroundDragBackend::SendInput
    );
    assert_eq!(
        select_windows_foreground_drag_backend(&action(
            Some("windows.synthetic_touch.v1"),
            Some("left"),
            "foreground"
        ))
        .unwrap(),
        WindowsForegroundDragBackend::SyntheticTouch
    );
    assert_eq!(
        select_windows_foreground_drag_backend(&action(
            Some(WINDOWS_RELATIVE_SEND_INPUT_BACKEND_ID),
            Some("left"),
            "foreground"
        ))
        .unwrap(),
        WindowsForegroundDragBackend::RelativeSendInput
    );
    assert!(
        select_windows_foreground_drag_backend(&action(
            Some("windows.synthetic_touch.v1"),
            Some("right"),
            "foreground"
        ))
        .unwrap_err()
        .contains("left-button")
    );
    assert!(
        select_windows_foreground_drag_backend(&action(
            Some("windows.synthetic_touch.v1"),
            None,
            "background"
        ))
        .is_err()
    );
    assert!(
        select_windows_foreground_drag_backend(&action(
            Some("windows.unknown.v1"),
            None,
            "foreground"
        ))
        .unwrap_err()
        .contains("unsupported")
    );
    let mut semantic_drag = action(Some("windows.synthetic_touch.v1"), None, "foreground");
    semantic_drag.element_token = Some("element-token".into());
    assert!(
        select_windows_foreground_drag_backend(&semantic_drag)
            .unwrap_err()
            .contains("screenshot coordinates")
    );
}

#[cfg(windows)]
#[test]
fn combined_down_drag_route_is_left_foreground_and_two_point_only() {
    let action: ComputerUseAction = serde_json::from_value(json!({
        "action": "drag",
        "delivery_mode": "foreground",
        "input_backend_id": "windows.send_input.combined_down_drag.v1",
        "button": "left",
        "path": [{"x": 1.0, "y": 2.0}, {"x": 3.0, "y": 4.0}]
    }))
    .unwrap();
    validate_action(&action).unwrap();

    assert_eq!(
        format!(
            "{:?}",
            select_windows_foreground_drag_backend(&action).unwrap()
        ),
        "CombinedDownDrag"
    );

    let mut legacy_action = action.clone();
    legacy_action.input_backend_id = Some(WINDOWS_SEND_INPUT_BACKEND_ID.into());
    assert_eq!(
        select_windows_foreground_drag_backend(&legacy_action).unwrap(),
        WindowsForegroundDragBackend::SendInput
    );

    for (button, delivery) in [("right", "foreground"), ("left", "background")] {
        let mut rejected = action.clone();
        rejected.button = Some(button.into());
        rejected.delivery_mode = Some(delivery.into());
        assert!(select_windows_foreground_drag_backend(&rejected).is_err());
    }

    let mut modified = action.clone();
    modified.modifiers = vec!["shift".into()];
    assert!(
        select_windows_foreground_drag_backend(&modified)
            .unwrap_err()
            .contains("modifiers")
    );

    let mut multi_segment = action.clone();
    multi_segment.path.push(ComputerUsePoint { x: 5.0, y: 6.0 });
    assert!(
        select_windows_foreground_drag_backend(&multi_segment)
            .unwrap_err()
            .contains("exactly two path points")
    );
}

#[cfg(windows)]
#[test]
fn combined_down_drag_batch_keeps_move_and_button_records_separate_and_ordered() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_MOVE,
        MOUSEEVENTF_MOVE_NOCOALESCE, MOUSEEVENTF_VIRTUALDESK,
    };

    let inputs =
        windows_combined_source_move_and_left_down_inputs((960, 540), (0, 0, 1_920, 1_080));
    let source_move = unsafe { inputs[0].Anonymous.mi };
    let left_down = unsafe { inputs[1].Anonymous.mi };
    let expected_source =
        platform_windows::virtualdesk::to_virtualdesk_absolute(960, 540, 0, 0, 1_920, 1_080);

    assert_eq!(inputs[0].r#type, INPUT_MOUSE);
    assert_eq!(
        source_move.dwFlags,
        MOUSEEVENTF_MOVE
            | MOUSEEVENTF_MOVE_NOCOALESCE
            | MOUSEEVENTF_ABSOLUTE
            | MOUSEEVENTF_VIRTUALDESK
    );
    assert_eq!(inputs[1].r#type, INPUT_MOUSE);
    assert_eq!((source_move.dx, source_move.dy), expected_source);
    assert_eq!(left_down.dwFlags, MOUSEEVENTF_LEFTDOWN);
    assert_eq!((left_down.dx, left_down.dy), (0, 0));
}

#[cfg(windows)]
#[test]
fn combined_down_drag_submits_the_two_record_prelude_in_exactly_one_send_input_call() {
    let calls = Cell::new(0_u32);
    let requested = Cell::new(0_usize);

    let injection = inject_windows_combined_input_batch_with(
        (960, 540),
        (0, 0, 1_920, 1_080),
        |inputs| {
            calls.set(calls.get() + 1);
            requested.set(inputs.len());
            2
        },
        || "unused OS error".to_owned(),
    );

    assert_eq!(calls.get(), 1);
    assert_eq!(requested.get(), 2);
    assert_eq!(injection.inserted(), 2);
    assert!(injection.was_accepted());
}

#[cfg(windows)]
#[test]
fn relative_drag_calibrates_accelerated_cursor_motion_to_the_exact_waypoint() {
    let positions = RefCell::new(VecDeque::from([(100, 100), (120, 100), (110, 100)]));
    let requested_deltas = RefCell::new(Vec::new());
    let settled_waypoints = RefCell::new(0_u32);

    let trace = run_windows_calibrated_relative_path(
        &[(110, 100)],
        4,
        0,
        || Ok(()),
        || {
            positions
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| "missing cursor sample".to_owned())
        },
        |dx, dy| {
            requested_deltas.borrow_mut().push((dx, dy));
            RelativeMoveInjection::accepted()
        },
        || *settled_waypoints.borrow_mut() += 1,
    );

    assert_eq!(requested_deltas.into_inner(), [(10, 0), (-5, 0)]);
    assert_eq!(trace["endpoint_reached"], true);
    assert_eq!(trace["endpoint_exact"], true);
    assert_eq!(trace["waypoints_reached"], 1);
    assert_eq!(trace["moves"].as_array().map(Vec::len), Some(2));
    assert_eq!(trace["moves"][0]["requested"], 1);
    assert_eq!(trace["moves"][0]["inserted"], 1);
    assert_eq!(trace["moves"][1]["residual"], json!([-10, 0]));
    assert_eq!(trace["moves"][1]["damping_applied"], json!([true, false]));
    assert_eq!(trace["waypoint_completions"][0]["rule"], "exact");
    assert_eq!(settled_waypoints.into_inner(), 1);
}

#[cfg(windows)]
#[test]
fn relative_drag_replays_r10_sign_flip_with_half_gain_damping() {
    // r10 oscillated around this exact waypoint because Windows applied roughly 2x motion:
    // [1434,831] + [8,-5] -> [1450,821], then [-8,5] -> [1434,831].
    let cursor = RefCell::new((1434_i32, 831_i32));
    let requested_deltas = RefCell::new(Vec::new());

    let trace = run_windows_calibrated_relative_path(
        &[(1442, 826)],
        4,
        0,
        || Ok(()),
        || Ok(*cursor.borrow()),
        |dx, dy| {
            requested_deltas.borrow_mut().push((dx, dy));
            let accelerated = |delta: i32| {
                if delta.unsigned_abs() > 1 {
                    delta * 2
                } else {
                    delta
                }
            };
            let mut position = cursor.borrow_mut();
            position.0 += accelerated(dx);
            position.1 += accelerated(dy);
            RelativeMoveInjection::accepted()
        },
        || {},
    );

    assert_eq!(requested_deltas.into_inner(), [(8, -5), (-4, 2), (0, 1)]);
    assert_eq!(*cursor.borrow(), (1442, 826));
    assert_eq!(trace["endpoint_reached"], true);
    assert_eq!(trace["endpoint_exact"], true);
    assert_eq!(trace["failure"], Value::Null);
    assert_eq!(trace["moves"][1]["residual"], json!([-8, 5]));
    assert_eq!(trace["moves"][1]["damping_applied"], json!([true, true]));
}

#[cfg(windows)]
#[test]
fn relative_drag_replays_r11_unit_stall_as_bounded_quantized_completion() {
    let cursor = RefCell::new((1462_i32, 814_i32));
    let requested_deltas = RefCell::new(Vec::new());

    let trace = run_windows_calibrated_relative_path(
        &[(1452, 820)],
        4,
        0,
        || Ok(()),
        || Ok(*cursor.borrow()),
        |dx, dy| {
            requested_deltas.borrow_mut().push((dx, dy));
            let next = match (dx, dy) {
                (-10, 6) => (1441, 826),
                (5, -3) => (1449, 822),
                (3, -2) => (1453, 819),
                (-1, 1) => (1453, 819),
                command => panic!("unexpected r11 replay command {command:?}"),
            };
            *cursor.borrow_mut() = next;
            RelativeMoveInjection::accepted()
        },
        || {},
    );

    assert_eq!(
        requested_deltas.into_inner(),
        [(-10, 6), (5, -3), (3, -2), (-1, 1)]
    );
    assert_eq!(*cursor.borrow(), (1453, 819));
    assert_eq!(trace["endpoint_reached"], true);
    assert_eq!(trace["endpoint_exact"], false);
    assert_eq!(trace["schema_version"], 4);
    assert_eq!(trace["all_waypoints_exact"], false);
    assert_eq!(trace["move_attempt_budget"], 4);
    assert_eq!(trace["move_attempts_used"], 4);
    assert_eq!(
        trace["completion_policy"],
        "configured_tolerance_or_intermediate_physical_pixel_or_observed_unit_stall"
    );
    assert_eq!(trace["quantized_stall_tolerance_px"], 1);
    assert_eq!(
        trace["waypoint_completions"][0],
        json!({
            "waypoint_index": 0,
            "target": [1452, 820],
            "actual": [1453, 819],
            "rule": "quantized_unit_stall",
            "max_error_px": 1,
            "attempt_budget": 4,
            "attempts_used": 4,
            "remaining_attempts": 0,
        })
    );
    assert_eq!(trace["moves"][3]["cursor_moved"], false);
}

#[cfg(windows)]
#[test]
fn relative_drag_replays_r12_with_a_bounded_fifth_attempt() {
    let cursor = RefCell::new((1810_i32, 836_i32));
    let requested_deltas = RefCell::new(Vec::new());

    let trace = run_windows_calibrated_relative_path(
        &[(1786, 845)],
        RELATIVE_DRAG_MAX_ATTEMPTS_PER_WAYPOINT,
        0,
        || Ok(()),
        || Ok(*cursor.borrow()),
        |dx, dy| {
            requested_deltas.borrow_mut().push((dx, dy));
            let next = match (dx, dy) {
                (-24, 9) => (1762, 854),
                (12, -4) => (1790, 846),
                (-2, -1) => (1788, 845),
                (-2, 0) => (1785, 845),
                (1, 0) => (1786, 845),
                command => panic!("unexpected r12 replay command {command:?}"),
            };
            *cursor.borrow_mut() = next;
            RelativeMoveInjection::accepted()
        },
        || {},
    );

    assert_eq!(
        requested_deltas.into_inner(),
        [(-24, 9), (12, -4), (-2, -1), (-2, 0), (1, 0)]
    );
    assert_eq!(*cursor.borrow(), (1786, 845));
    assert_eq!(trace["endpoint_reached"], true);
    assert_eq!(trace["endpoint_exact"], true);
    assert_eq!(trace["schema_version"], 4);
    assert_eq!(trace["all_waypoints_exact"], true);
    assert_eq!(trace["max_attempts_per_waypoint"], 6);
    assert_eq!(trace["move_attempt_budget"], 6);
    assert_eq!(trace["move_attempts_used"], 5);
    assert_eq!(
        trace["waypoint_completions"][0],
        json!({
            "waypoint_index": 0,
            "target": [1786, 845],
            "actual": [1786, 845],
            "rule": "exact",
            "max_error_px": 0,
            "attempt_budget": 6,
            "attempts_used": 5,
            "remaining_attempts": 1,
        })
    );
}

#[cfg(windows)]
#[test]
fn relative_drag_replays_r13_without_damping_residual_two_into_the_unit_deadzone() {
    let cursor = RefCell::new((1050_i32, 1258_i32));
    let requested_deltas = RefCell::new(Vec::new());

    let trace = run_windows_calibrated_relative_path(
        &[(1025, 1258), (1024, 1258)],
        RELATIVE_DRAG_MAX_ATTEMPTS_PER_WAYPOINT,
        0,
        || Ok(()),
        || Ok(*cursor.borrow()),
        |dx, dy| {
            requested_deltas.borrow_mut().push((dx, dy));
            let next = match (dx, dy) {
                (-25, 0) => (983, 1258),
                (21, 0) => (1053, 1258),
                (-14, 0) => (1021, 1258),
                (2, 0) => (1022, 1258),
                (3, 0) => (1027, 1258),
                (-1, 0) => (1027, 1258),
                (-2, 0) => (1024, 1258),
                command => panic!("unexpected r13 replay command {command:?}"),
            };
            *cursor.borrow_mut() = next;
            RelativeMoveInjection::accepted()
        },
        || {},
    );

    assert_eq!(
        requested_deltas.into_inner(),
        [(-25, 0), (21, 0), (-14, 0), (2, 0), (3, 0), (-2, 0)]
    );
    assert_eq!(*cursor.borrow(), (1024, 1258));
    assert_eq!(trace["endpoint_reached"], true);
    assert_eq!(trace["endpoint_exact"], true);
    assert_eq!(trace["all_waypoints_exact"], false);
    assert_eq!(trace["schema_version"], 4);
    assert_eq!(trace["intermediate_tolerance_px"], 1);
    assert_eq!(trace["damping_min_effective_command_px"], 2);
    assert_eq!(trace["stagnation_escape_max_residual_px"], 3);
    assert_eq!(trace["stagnation_escape_max_command_px"], 4);
    assert_eq!(
        trace["waypoint_completions"][0]["rule"],
        "intermediate_physical_pixel"
    );
    assert_eq!(trace["waypoint_completions"][0]["max_error_px"], 1);
    assert_eq!(trace["waypoint_completions"][0]["attempts_used"], 6);
    assert_eq!(trace["waypoint_completions"][1]["rule"], "exact");
    assert_eq!(trace["moves"][5]["delta"], json!([-2, 0]));
    assert_eq!(trace["moves"][5]["damping_applied"], json!([true, false]));
    assert_eq!(
        trace["moves"][5]["stagnation_escape_applied"],
        json!([false, false])
    );
}

#[cfg(windows)]
#[test]
fn relative_drag_replays_r15_nonlinear_gain_without_command_divergence() {
    // r15's 4K RDP session amplified the same waypoint correction more on every move:
    // 1337 -29-> 1245 +31-> 1399 -45-> 1151 +78-> 1621 -156-> 627 +340-> 2857.
    // Scale arbitrary replacement commands by those measured per-attempt gains so the test
    // remains about convergence instead of prescribing the controller's implementation.
    let measured_gain_trace = [
        (29_i32, 92_i32),
        (31, 154),
        (45, 248),
        (78, 470),
        (156, 994),
        (340, 2_230),
    ];
    let cursor = RefCell::new((1_337_i32, 630_i32));
    let cursor_reads = RefCell::new(0_usize);
    let requested_deltas = RefCell::new(Vec::new());

    let trace = run_windows_calibrated_relative_path(
        &[(1_308, 630)],
        RELATIVE_DRAG_MAX_ATTEMPTS_PER_WAYPOINT,
        0,
        || Ok(()),
        || {
            *cursor_reads.borrow_mut() += 1;
            Ok(*cursor.borrow())
        },
        |dx, dy| {
            assert_eq!(dy, 0);
            let attempt = requested_deltas.borrow().len();
            requested_deltas.borrow_mut().push((dx, dy));
            let displacement = if dx.unsigned_abs() <= 2 {
                dx
            } else {
                let (measured_command, measured_displacement) = measured_gain_trace[attempt];
                let magnitude = (i64::from(dx).abs() * i64::from(measured_displacement)
                    + i64::from(measured_command) / 2)
                    / i64::from(measured_command);
                i64::from(dx.signum())
                    .saturating_mul(magnitude)
                    .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
            };
            cursor.borrow_mut().0 += displacement;
            RelativeMoveInjection::accepted()
        },
        || {},
    );

    let requested_deltas = requested_deltas.into_inner();
    assert_eq!(requested_deltas.first(), Some(&(-29, 0)));
    assert!(
        requested_deltas
            .iter()
            .all(|(dx, dy)| dx.unsigned_abs() <= 29 && *dy == 0),
        "adaptive commands escaped the initial 29px trust region: {requested_deltas:?}"
    );
    assert_eq!(*cursor.borrow(), (1_308, 630));
    assert_eq!(trace["endpoint_reached"], true);
    assert_eq!(trace["endpoint_exact"], true);
    assert_eq!(trace["failure"], Value::Null);
    assert_eq!(
        trace["fence_checks"].as_u64(),
        trace["move_attempts_used"].as_u64()
    );
    assert_eq!(
        *cursor_reads.borrow(),
        trace["move_attempts_used"].as_u64().unwrap() as usize + 1
    );
    assert!(trace["moves"].as_array().unwrap().iter().all(|movement| {
        movement["requested"] == 1 && movement["inserted"] == 1 && movement["after"].is_array()
    }));
}

#[cfg(windows)]
#[test]
fn relative_drag_escapes_a_small_observed_deadzone_without_extra_attempts() {
    let cursor = RefCell::new((102_i32, 100_i32));
    let requested_deltas = RefCell::new(Vec::new());

    let trace = run_windows_calibrated_relative_path(
        &[(100, 100), (99, 100)],
        2,
        0,
        || Ok(()),
        || Ok(*cursor.borrow()),
        |dx, dy| {
            requested_deltas.borrow_mut().push((dx, dy));
            match (dx, dy) {
                (-2, 0) => {}
                (-3, 0) => *cursor.borrow_mut() = (99, 100),
                command => panic!("unexpected deadzone escape command {command:?}"),
            }
            RelativeMoveInjection::accepted()
        },
        || {},
    );

    assert_eq!(requested_deltas.into_inner(), [(-2, 0), (-3, 0)]);
    assert_eq!(trace["endpoint_reached"], true);
    assert_eq!(trace["endpoint_exact"], true);
    assert_eq!(trace["all_waypoints_exact"], false);
    assert_eq!(
        trace["moves"][1]["stagnation_escape_applied"],
        json!([true, false])
    );
    assert_eq!(
        trace["waypoint_completions"][0]["rule"],
        "intermediate_physical_pixel"
    );
}

#[cfg(windows)]
#[test]
fn relative_drag_does_not_treat_a_moving_cursor_with_one_pixel_error_as_quantized() {
    let positions = RefCell::new(VecDeque::from([(101, 100), (99, 100)]));

    let trace = run_windows_calibrated_relative_path(
        &[(100, 100)],
        1,
        0,
        || Ok(()),
        || {
            positions
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| "missing cursor sample".to_owned())
        },
        |dx, dy| {
            assert_eq!((dx, dy), (-1, 0));
            RelativeMoveInjection::accepted()
        },
        || panic!("a moving one-pixel miss must not settle"),
    );

    assert_eq!(trace["endpoint_reached"], false);
    assert_eq!(trace["endpoint_exact"], false);
    assert_eq!(trace["failure"], "endpoint_not_reached");
    assert_eq!(trace["moves"][0]["cursor_moved"], true);
    assert_eq!(trace["waypoint_completions"], json!([]));
}

#[cfg(windows)]
#[test]
fn relative_drag_fails_closed_when_cursor_calibration_does_not_converge() {
    let positions = RefCell::new(VecDeque::from([(100, 100), (100, 100), (100, 100)]));
    let requested_deltas = RefCell::new(Vec::new());

    let trace = run_windows_calibrated_relative_path(
        &[(108, 100)],
        2,
        0,
        || Ok(()),
        || {
            positions
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| "missing cursor sample".to_owned())
        },
        |dx, dy| {
            requested_deltas.borrow_mut().push((dx, dy));
            RelativeMoveInjection::accepted()
        },
        || {},
    );

    assert_eq!(trace["endpoint_reached"], false);
    assert_eq!(trace["waypoints_reached"], 0);
    assert_eq!(trace["failure"], "endpoint_not_reached");
    assert_eq!(trace["moves"].as_array().map(Vec::len), Some(2));
    assert_eq!(trace["move_attempt_budget"], 2);
    assert_eq!(trace["move_attempts_used"], 2);
    assert_eq!(requested_deltas.into_inner(), [(8, 0), (8, 0)]);
    assert_eq!(trace["moves"][1]["damping_applied"], json!([false, false]));
}

#[cfg(windows)]
#[test]
fn relative_drag_stops_on_the_first_incomplete_send_input_move() {
    let cursor_samples = RefCell::new(VecDeque::from([(100, 100)]));
    let injections = RefCell::new(0_u32);

    let trace = run_windows_calibrated_relative_path(
        &[(110, 100)],
        4,
        0,
        || Ok(()),
        || {
            cursor_samples
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| "unexpected cursor read after rejected injection".to_owned())
        },
        |_, _| {
            *injections.borrow_mut() += 1;
            RelativeMoveInjection::incomplete(0, "SendInput inserted 0/1")
        },
        || panic!("a rejected waypoint must not settle"),
    );

    assert_eq!(injections.into_inner(), 1);
    assert_eq!(trace["endpoint_reached"], false);
    assert_eq!(trace["failure"], "send_input_incomplete");
    assert_eq!(trace["moves"][0]["requested"], 1);
    assert_eq!(trace["moves"][0]["inserted"], 0);
}

#[cfg(windows)]
#[test]
fn relative_drag_stops_before_move_when_the_target_fence_is_lost() {
    let injections = RefCell::new(0_u32);

    let trace = run_windows_calibrated_relative_path(
        &[(110, 100)],
        4,
        0,
        || Err("exact target HWND lost foreground".to_owned()),
        || Ok((100, 100)),
        |_, _| {
            *injections.borrow_mut() += 1;
            RelativeMoveInjection::accepted()
        },
        || panic!("a fenced-out waypoint must not settle"),
    );

    assert_eq!(injections.into_inner(), 0);
    assert_eq!(trace["endpoint_reached"], false);
    assert_eq!(trace["failure"], "target_fence_lost");
    assert_eq!(trace["failure_detail"], "exact target HWND lost foreground");
    assert_eq!(trace["moves"], json!([]));
}

#[cfg(windows)]
#[test]
fn synthetic_touch_result_reports_api_acceptance_without_claiming_effect() {
    let accepted = windows_synthetic_touch_result(Ok(()), &test_window_target(), true);
    assert_eq!(accepted.value["success"], true);
    assert!(!accepted.degraded);
    assert_eq!(
        accepted.value["delivery"],
        json!({
            "mode": "foreground",
            "backend_id": WINDOWS_SYNTHETIC_TOUCH_BACKEND_ID,
            "api_accepted": true,
            "consumer_effect_confirmed": false,
            "completion_known": false,
            "verification_required": true,
            "retry_safe": false,
            "target_fence": {
                "process_id": 42,
                "window_handle": 7,
                "exact_window": true,
                "foreground_required": true,
                "foreground_verified": true
            }
        })
    );
    assert_eq!(accepted.value["effect"], "unverifiable");

    let rejected = windows_synthetic_touch_result(
        Err("InjectSyntheticPointerInput failed".into()),
        &test_window_target(),
        true,
    );
    assert_eq!(rejected.value["success"], false);
    assert_eq!(rejected.value["delivery"]["api_accepted"], false);
    assert_eq!(
        rejected.value["delivery"]["backend_error"],
        "InjectSyntheticPointerInput failed"
    );
    assert!(rejected.degraded);

    let foreground_lost = windows_synthetic_touch_result(
        Err("exact foreground target was lost".into()),
        &test_window_target(),
        false,
    );
    assert_eq!(
        foreground_lost.value["delivery"]["target_fence"]["foreground_verified"],
        false
    );
}

#[test]
fn unsupported_input_backend_is_a_structured_non_fallback_result() {
    let result = input_backend_rejection_result(
        "windows.unknown.v1",
        "unsupported input backend",
        &test_window_target(),
    );
    assert_eq!(result.value["success"], false);
    assert_eq!(result.value["route"], "input_backend_selection");
    assert_eq!(
        result.value["delivery"],
        json!({
            "mode": "foreground",
            "backend_id": "windows.unknown.v1",
            "api_accepted": false,
            "consumer_effect_confirmed": false,
            "completion_known": false,
            "verification_required": true,
            "retry_safe": false,
            "fallback_attempted": false,
            "rejection_reason": "unsupported input backend",
            "target_fence": {
                "process_id": 42,
                "window_handle": 7,
                "exact_window": true,
                "foreground_required": true,
                "foreground_verified": true
            }
        })
    );
    assert_eq!(result.value["effect"], "not_attempted");
    assert!(result.degraded);
}

#[rstest]
fn semantic_value_actions_require_and_encode_element_values() {
    let action = ComputerUseAction {
        action: "set_text".into(),
        element_index: Some(11),
        text: Some("Hero".into()),
        ..Default::default()
    };
    let args = action_arguments(
        &action,
        "session",
        &WindowTarget {
            pid: 42,
            window_id: 7,
            title: "DCC".into(),
            app_name: "dcc".into(),
            bounds: [0, 0, 100, 100],
            is_on_screen: true,
            is_minimized: false,
            z_index: Some(1),
            is_foreground: true,
        },
    );
    assert_eq!(args["_tool"], "set_value");
    assert_eq!(args["element_index"], 11);
    assert_eq!(args["value"], "Hero");
    assert_eq!(
        validate_action(&ComputerUseAction {
            action: "set_value".into(),
            text: Some("Hero".into()),
            ..Default::default()
        })
        .unwrap_err()
        .code,
        ComputerUseErrorCode::InvalidAction
    );
}

#[rstest]
fn coordinate_text_and_key_actions_forward_cua_focus_arguments() {
    let type_action = ComputerUseAction {
        action: "type".into(),
        x: Some(20.0),
        y: Some(30.0),
        text: Some("Fab".into()),
        ..Default::default()
    };
    assert!(validate_action(&type_action).is_ok());
    let type_args = action_arguments(&type_action, "session", &test_window_target());
    assert_eq!(type_args["_tool"], "type_text");
    assert_eq!(type_args["x"], 20.0);
    assert_eq!(type_args["y"], 30.0);

    let press_action = ComputerUseAction {
        action: "keypress".into(),
        x: Some(20.0),
        y: Some(30.0),
        keys: vec!["S".into()],
        modifiers: vec!["CTRL".into(), "SHIFT".into()],
        ..Default::default()
    };
    let press_args = action_arguments(&press_action, "session", &test_window_target());
    assert_eq!(press_args["x"], 20.0);
    assert_eq!(press_args["modifiers"], json!(["CTRL", "SHIFT"]));

    let drag_action = ComputerUseAction {
        action: "drag".into(),
        path: vec![
            ComputerUsePoint { x: 1.0, y: 2.0 },
            ComputerUsePoint { x: 3.0, y: 4.0 },
        ],
        button: Some("middle".into()),
        modifiers: vec!["ALT".into()],
        duration_ms: Some(750),
        steps: Some(32),
        ..Default::default()
    };
    let drag_args = action_arguments(&drag_action, "session", &test_window_target());
    assert_eq!(drag_args["button"], "middle");
    assert_eq!(drag_args["modifier"], json!(["ALT"]));
    assert_eq!(drag_args["duration_ms"], 750);
    assert_eq!(drag_args["steps"], 32);
}

#[rstest]
#[case(-4, 0, "left", 4)]
#[case(4, 0, "right", 4)]
#[case(0, -5, "up", 5)]
#[case(0, 5, "down", 5)]
fn scroll_axes_reach_window_and_desktop_cua(
    #[case] scroll_x: i32,
    #[case] scroll_y: i32,
    #[case] direction: &str,
    #[case] amount: u32,
) {
    let action = ComputerUseAction {
        action: "scroll".into(),
        scroll_x: Some(scroll_x),
        scroll_y: Some(scroll_y),
        scroll_by: Some("page".into()),
        ..Default::default()
    };
    validate_action(&action).unwrap();
    for arguments in [
        action_arguments(&action, "session", &test_window_target()),
        desktop_action_arguments(&action, "session"),
    ] {
        assert_eq!(arguments["direction"], direction);
        assert_eq!(arguments["amount"], amount);
        assert_eq!(arguments["by"], "page");
    }
}

#[rstest]
fn scroll_rejects_diagonal_unbounded_and_unscoped_requests() {
    for action in [
        ComputerUseAction {
            action: "scroll".into(),
            scroll_x: Some(1),
            scroll_y: Some(1),
            ..Default::default()
        },
        ComputerUseAction {
            action: "scroll".into(),
            scroll_y: Some(51),
            ..Default::default()
        },
        ComputerUseAction {
            action: "scroll".into(),
            ..Default::default()
        },
        ComputerUseAction {
            action: "scroll".into(),
            element_index: Some(1),
            scroll_by: Some("pixel".into()),
            ..Default::default()
        },
    ] {
        assert!(validate_action(&action).is_err());
    }

    let element_scroll = ComputerUseAction {
        action: "scroll".into(),
        element_index: Some(1),
        ..Default::default()
    };
    validate_action(&element_scroll).unwrap();
    let arguments = action_arguments(&element_scroll, "session", &test_window_target());
    assert_eq!(arguments["direction"], "down");
    assert!(arguments.get("amount").is_none());
}

#[rstest]
fn semantic_type_uses_cua_canonical_type_text() {
    let action = ComputerUseAction {
        action: "type".into(),
        element_index: Some(8),
        text: Some("hello".into()),
        ..Default::default()
    };
    let arguments = action_arguments(&action, "session", &test_window_target());
    assert_eq!(arguments["_tool"], "type_text");
    assert_eq!(arguments["element_index"], 8);
    assert_eq!(arguments["text"], "hello");
}

fn test_window_target() -> WindowTarget {
    WindowTarget {
        pid: 42,
        window_id: 7,
        title: "DCC".into(),
        app_name: "dcc".into(),
        bounds: [0, 0, 100, 100],
        is_on_screen: true,
        is_minimized: false,
        z_index: Some(1),
        is_foreground: true,
    }
}

#[test]
fn minimized_exact_target_pauses_every_action_before_any_input_path() {
    let mut target = test_window_target();
    target.is_minimized = true;
    target.is_on_screen = false;
    target.is_foreground = false;

    let error = ensure_target_available_for_action(&target).unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::TargetMinimized);
    assert!(error.message.contains("target_minimized"));
    assert!(error.message.contains("automatic_input=false"));
    assert!(error.message.contains("restore_activate"));
}

#[test]
fn failed_window_restore_still_invalidates_action_cache_before_mutation() {
    let invalidated = Cell::new(false);

    let result = run_preinvalidated_window_mutation(
        || invalidated.set(true),
        || {
            assert!(invalidated.get(), "cache must be stale before mutation");
            Err::<(), _>("foreground denied")
        },
    );

    assert_eq!(result, Err("foreground denied"));
    assert!(invalidated.get());
}

#[test]
fn restore_input_gate_failure_never_reaches_mutation_or_cache_invalidation() {
    let invalidations = Cell::new(0);
    let mutations = Cell::new(0);

    let result = run_gated_preinvalidated_window_mutation(
        || Err::<(), _>("desktop locked"),
        || invalidations.set(invalidations.get() + 1),
        || {
            mutations.set(mutations.get() + 1);
            Ok(())
        },
    );

    assert_eq!(result, Err("desktop locked"));
    assert_eq!(invalidations.get(), 0);
    assert_eq!(mutations.get(), 0);
}

#[rstest]
fn type_chars_forwards_delivery_mode_to_cua_character_input() {
    let action = ComputerUseAction {
        action: "type_chars".into(),
        element_index: Some(4),
        text: Some("Fab".into()),
        delay_ms: Some(20),
        delivery_mode: Some("foreground".into()),
        ..Default::default()
    };
    assert!(validate_action(&action).is_ok());
    let args = action_arguments(
        &action,
        "session",
        &WindowTarget {
            pid: 42,
            window_id: 7,
            title: "DCC".into(),
            app_name: "dcc".into(),
            bounds: [0, 0, 100, 100],
            is_on_screen: true,
            is_minimized: false,
            z_index: Some(1),
            is_foreground: true,
        },
    );
    assert_eq!(args["_tool"], "type_text");
    assert_eq!(args["text"], "Fab");
    assert_eq!(args["delay_ms"], 20);
    assert_eq!(args["element_index"], 4);
    assert_eq!(args["delivery_mode"], "foreground");
    assert!(args.get("type_chars_only").is_none());

    let focused = ComputerUseAction {
        action: "type_chars".into(),
        text: Some("UE".into()),
        type_chars_only: true,
        ..Default::default()
    };
    assert!(validate_action(&focused).is_ok());
    assert!(
        validate_action(&ComputerUseAction {
            action: "type_chars".into(),
            text: Some("UE".into()),
            ..Default::default()
        })
        .is_err()
    );
}

#[rstest]
fn desktop_actions_are_screen_scoped_and_observation_bound() {
    let action = ComputerUseAction {
        action: "click".into(),
        x: Some(100.0),
        y: Some(200.0),
        ..Default::default()
    };
    let args = desktop_action_arguments(&action, "desktop-session");
    assert_eq!(args["_tool"], "click");
    assert_eq!(args["scope"], "desktop");
    assert_eq!(args["session"], "desktop-session");
    assert_eq!(args["x"], 100.0);
    assert!(args.get("pid").is_none());

    let toggle = ComputerUseAction {
        action: "toggle".into(),
        x: Some(100.0),
        y: Some(200.0),
        ..Default::default()
    };
    let toggle_args = desktop_action_arguments(&toggle, "desktop-session");
    assert_eq!(toggle_args["_tool"], "click");
    assert_eq!(toggle_args["count"], 1);

    let type_chars = ComputerUseAction {
        action: "type_chars".into(),
        text: Some("Fab".into()),
        delay_ms: Some(20),
        type_chars_only: true,
        ..Default::default()
    };
    let type_args = desktop_action_arguments(&type_chars, "desktop-session");
    assert_eq!(type_args["_tool"], "type_text");
    assert_eq!(type_args["delay_ms"], 20);
    assert!(type_args.get("type_chars_only").is_none());
}

#[rstest]
fn window_visual_fallback_maps_capture_pixels_to_the_exact_target() {
    let observation = ComputerUseObservation {
        observation_id: "obs".into(),
        window_handle: 7,
        process_id: 42,
        window_title: "UE".into(),
        width: 1560,
        height: 992,
        source_rect: [1, 1, 3120, 1984],
        capture_backend: "cua-driver-sdk-desktop-crop".into(),
        capture_provenance: json!({"desktop_crop_bounds": [20, 30, 1560, 992]}),
        session_id: "session".into(),
    };
    let target = test_window_target();
    let validated = action_for_window_visual_fallback(
        &ComputerUseAction {
            action: "move".into(),
            x: Some(780.0),
            y: Some(120.0),
            ..Default::default()
        },
        &observation,
    )
    .unwrap();
    assert_eq!((validated.x, validated.y), (Some(1560.0), Some(240.0)));
    let args = action_arguments(&validated, "session", &target);
    assert_eq!(args["pid"], 42);
    assert_eq!(args["window_id"], 7);
    assert!(args.get("scope").is_none());
    assert!(
        action_for_window_visual_fallback(
            &ComputerUseAction {
                action: "click".into(),
                element_index: Some(1),
                ..Default::default()
            },
            &observation,
        )
        .is_err()
    );
}

#[rstest]
fn launch_requires_one_safe_application_selector() {
    assert!(validate_launch_request(&ComputerUseLaunchRequest::default()).is_err());
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            name: Some("Calculator".into()),
            ..Default::default()
        })
        .is_ok()
    );
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            name: Some("Calculator".into()),
            bundle_id: Some("com.example.Calculator".into()),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            urls: vec!["com.epicgames.launcher://fab/plugins/egl".into()],
            ..Default::default()
        })
        .is_ok()
    );
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            urls: vec!["file:///C:/Windows/System32/cmd.exe".into()],
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        validate_launch_request(&ComputerUseLaunchRequest {
            launch_path: Some("powershell.exe".into()),
            ..Default::default()
        })
        .is_err()
    );
    let json = serde_json::to_value(ComputerUseLaunchRequest {
        name: Some("Calculator".into()),
        ..Default::default()
    })
    .expect("launch request should serialize");
    assert!(json.get("bundle_id").is_none());
    let scoped = launch_arguments(
        &ComputerUseLaunchRequest {
            name: Some("Calculator".into()),
            ..Default::default()
        },
        Some("private-runtime-session"),
    )
    .expect("session-scoped launch arguments");
    assert_eq!(scoped["session"], "private-runtime-session");
}

#[rstest]
fn clipboard_and_recording_requests_are_bounded() {
    assert!(
        validate_clipboard_write_request(&ComputerUseClipboardWriteRequest::default()).is_err()
    );
    assert!(
        validate_clipboard_write_request(&ComputerUseClipboardWriteRequest {
            text: Some("hello".into()),
            image_path: Some("C:\\image.png".into()),
            file_path: None,
        })
        .is_err()
    );
    assert!(
        validate_recording_start_request(&ComputerUseRecordingStartRequest {
            output_dir: "relative/output".into(),
            record_video: false,
        })
        .is_err()
    );
    assert!(
        validate_recording_start_request(&ComputerUseRecordingStartRequest {
            output_dir: "~/cua-recordings".into(),
            record_video: false,
        })
        .is_ok()
    );
}

#[test]
fn recording_state_aggregates_a_lost_trajectory_lease_without_hiding_raw_state() {
    let trajectory = json!({
        "recording": true,
        "enabled": false,
        "owner": "recording-session",
        "output_dir": "C:\\recordings\\showcase",
    });

    let state =
        aggregate_recording_state(true, false, &trajectory, None, &["trajectory_lease_lost"]);

    assert_eq!(state["status"], "degraded");
    assert_eq!(state["healthy"], false);
    assert_eq!(state["expected_components"], json!(["trajectory"]));
    assert_eq!(state["issues"], json!(["trajectory_lease_lost"]));
    assert_eq!(state["trajectory"], trajectory);
    assert_eq!(state["video"], Value::Null);
}

#[test]
fn recording_state_reports_healthy_video_and_clean_terminal_states() {
    let trajectory = json!({"structuredContent": {"enabled": true}});
    let video = json!({"active": true, "path": "C:\\recordings\\showcase.mp4"});
    let active = aggregate_recording_state(true, true, &trajectory, Some(&video), &[]);
    assert_eq!(active["status"], "active");
    assert_eq!(active["healthy"], true);
    assert_eq!(
        active["expected_components"],
        json!(["trajectory", "video"])
    );
    assert_eq!(active["video"], video);

    let stopped = aggregate_recording_state(false, true, &trajectory, None, &[]);
    assert_eq!(stopped["status"], "stopped");
    assert_eq!(stopped["healthy"], true);
    assert_eq!(stopped["video"], Value::Null);
}

#[test]
fn recording_state_preserves_finalized_video_terminal_reason() {
    let trajectory = json!({
        "structuredContent": {
            "enabled": true,
            "owner": "recording-session",
        }
    });
    let video = json!({
        "active": false,
        "backend": "embedded-openh264",
        "path": "C:\\recordings\\showcase.mp4",
        "finalized": true,
        "frames": 3_528,
        "terminal_reason": {
            "code": "interactive_desktop_unavailable",
            "message": "Windows interactive session disconnected",
            "timestamp_ms": 1_786_286_360_894_u64,
            "last_sequence": 3_528,
        }
    });

    let state =
        aggregate_recording_state(true, true, &trajectory, Some(&video), &["video_stopped"]);

    assert_eq!(state["status"], "degraded");
    assert_eq!(state["issues"], json!(["video_stopped"]));
    assert_eq!(state["video"], video);
}

#[test]
fn recording_health_latches_owner_and_component_failures() {
    let health = RecordingHealth::new("recording-session");
    health.observe_trajectory(&json!({"enabled": true, "owner": "other-session"}));
    health.observe_video(Some(&json!({"active": false})), true);
    health.observe_trajectory(&json!({"enabled": true, "owner": "recording-session"}));

    assert_eq!(
        health.issue_names(),
        vec!["owner_mismatch", "video_stopped"]
    );
}

#[test]
fn recording_health_reads_the_upstream_structured_content_envelope() {
    let health = RecordingHealth::new("recording-session");

    assert!(health.observe_trajectory(&json!({
        "content": [{"type": "text", "text": "recording: enabled"}],
        "structuredContent": {
            "enabled": true,
            "owner": "recording-session",
        },
    })));
    assert_eq!(health.issue_names(), Vec::<&str>::new());
}

#[tokio::test(start_paused = true)]
async fn recording_keepalive_probes_only_the_same_session_and_stops_cleanly() {
    let health = RecordingHealth::new("recording-session");
    let probed_sessions = Arc::new(Mutex::new(Vec::new()));
    let probe_count = Arc::new(Mutex::new(0_u32));
    let sessions = Arc::clone(&probed_sessions);
    let counts = Arc::clone(&probe_count);
    let mut keepalive = RecordingKeepalive::spawn(
        "recording-session".into(),
        Duration::from_secs(30),
        health.clone(),
        move |session_id| {
            let sessions = Arc::clone(&sessions);
            let counts = Arc::clone(&counts);
            async move {
                sessions.lock().unwrap().push(session_id);
                let mut count = counts.lock().unwrap();
                *count += 1;
                let enabled = *count == 1;
                Ok::<_, ()>(json!({
                    "enabled": enabled,
                    "owner": "recording-session",
                }))
            }
        },
    );

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    tokio::task::yield_now().await;
    assert_eq!(health.issue_names(), Vec::<&str>::new());

    tokio::time::advance(Duration::from_secs(30)).await;
    tokio::task::yield_now().await;
    assert_eq!(health.issue_names(), vec!["trajectory_lease_lost"]);
    let state = aggregate_recording_state(
        true,
        false,
        &json!({"enabled": false, "owner": "recording-session"}),
        None,
        &health.issue_names(),
    );
    assert_eq!(state["status"], "degraded");
    assert_eq!(
        probed_sessions.lock().unwrap().as_slice(),
        ["recording-session", "recording-session"]
    );

    tokio::time::advance(Duration::from_secs(120)).await;
    tokio::task::yield_now().await;
    assert_eq!(probed_sessions.lock().unwrap().len(), 2);

    keepalive.stop().await;
    tokio::time::advance(Duration::from_secs(120)).await;
    tokio::task::yield_now().await;
    assert_eq!(probed_sessions.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn dropping_recording_keepalive_prevents_future_probes() {
    let probes = Arc::new(Mutex::new(0_u32));
    let recorded = Arc::clone(&probes);
    let keepalive = RecordingKeepalive::spawn(
        "recording-session".into(),
        Duration::from_secs(30),
        RecordingHealth::new("recording-session"),
        move |_| {
            let recorded = Arc::clone(&recorded);
            async move {
                *recorded.lock().unwrap() += 1;
                Ok::<_, ()>(json!({
                    "enabled": true,
                    "owner": "recording-session",
                }))
            }
        },
    );

    tokio::task::yield_now().await;
    drop(keepalive);
    tokio::time::advance(Duration::from_secs(120)).await;
    tokio::task::yield_now().await;
    assert_eq!(*probes.lock().unwrap(), 0);
}

#[rstest]
fn window_queries_require_a_selector_and_match_native_rows() {
    assert!(ComputerUseWindowQuery::default().validate().is_err());
    assert!(
        ComputerUseWindowQuery {
            app: Some(String::new()),
            ..Default::default()
        }
        .validate()
        .is_err()
    );

    let query = ComputerUseWindowQuery {
        app: Some("ue5editor.exe".into()),
        window_title: Some("PCG Fab".into()),
        ..Default::default()
    };
    assert!(query.matches_window(
        &json!({"app_name":"UE5Editor.exe", "title":"PCG Fab", "pid":42, "window_id":7})
    ));
    assert!(!query.matches_window(
        &json!({"app_name":"UE5Editor.exe", "title":"Other", "pid":42, "window_id":7})
    ));
    assert!(query.validate_selectors().is_ok());
}

#[rstest]
fn live_observation_fps_is_bounded() {
    assert_eq!(
        ComputerUseLiveObservationStartRequest::default(),
        ComputerUseLiveObservationStartRequest {
            fps: 10,
            max_dimension: 1_568,
        }
    );
    for fps in [0, 31] {
        assert!(
            ComputerUseLiveObservationStartRequest {
                fps,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
    for max_dimension in [255, 4_097] {
        assert!(
            ComputerUseLiveObservationStartRequest {
                max_dimension,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
}

#[rstest]
fn live_observation_stops_on_terminal_capture_errors() {
    assert!(terminal_capture_error(&ComputerUseError::new(
        ComputerUseErrorCode::InvalidTarget,
        "window identity changed",
    )));
    assert!(terminal_capture_error(&ComputerUseError::new(
        ComputerUseErrorCode::InteractiveDesktopUnavailable,
        "desktop disconnected",
    )));
    assert!(!terminal_capture_error(&ComputerUseError::new(
        ComputerUseErrorCode::CaptureFailed,
        "transient WGC failure",
    )));
}

#[rstest]
fn windows_capture_failure_policy_distinguishes_retry_from_terminal_fences() {
    let capture_failed =
        || ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, "transient WGC failure");
    let active_unknown_desktop =
        windows_diagnostic(Ok(0), Err("OpenInputDesktop: access denied"), Ok(()), false);
    let disconnected = windows_diagnostic(Ok(4), Ok(Some("Default")), Ok(()), false);
    let secure_desktop = windows_diagnostic(Ok(0), Ok(Some("Winlogon")), Ok(()), false);

    let cases = [
        (
            capture_failed(),
            require_exact_window_observation_from(&active_unknown_desktop),
            false,
            ComputerUseErrorCode::CaptureFailed,
        ),
        (
            capture_failed(),
            require_exact_window_observation_from(&disconnected),
            true,
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
        ),
        (
            capture_failed(),
            require_exact_window_observation_from(&secure_desktop),
            true,
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
        ),
        (
            ComputerUseError::new(ComputerUseErrorCode::MissingWindow, "window closed"),
            require_exact_window_observation_from(&active_unknown_desktop),
            true,
            ComputerUseErrorCode::MissingWindow,
        ),
        (
            ComputerUseError::new(ComputerUseErrorCode::InvalidTarget, "owner changed"),
            require_exact_window_observation_from(&active_unknown_desktop),
            true,
            ComputerUseErrorCode::InvalidTarget,
        ),
    ];

    for (capture_error, observation_gate, should_terminate, expected_code) in cases {
        let decision = live_capture_failure_disposition(capture_error, observation_gate);
        match (decision, should_terminate) {
            (CaptureFailureDisposition::Retry(error), false)
            | (CaptureFailureDisposition::Terminal(error), true) => {
                assert_eq!(error.code, expected_code);
            }
            (unexpected, _) => panic!("unexpected failure disposition: {unexpected:?}"),
        }
    }
}

#[rstest]
fn live_observation_png_converts_bgra_to_rgba() {
    let png = encode_bgra_to_png(&[3, 2, 1, 4], 1, 1).unwrap();
    let mut reader = png::Decoder::new(Cursor::new(&png)).read_info().unwrap();
    let mut bytes = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut bytes).unwrap();
    assert_eq!(&bytes[..info.buffer_size()], &[1, 2, 3, 4]);
    assert_eq!(decode_png_to_bgra(&png).unwrap(), (vec![3, 2, 1, 4], 1, 1));
}

#[rstest]
fn live_observation_keeps_only_the_latest_frame() {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1], 1, 1, Instant::now()),
        Duration::ZERO,
        "test_capture",
    );
    status.publish_frame(
        LiveObservationFrame::new(2, vec![2], 1, 1, Instant::now()),
        Duration::ZERO,
        "test_capture",
    );

    assert_eq!(status.latest().expect("latest frame").sequence(), 2);
    assert_eq!(status.as_json(true, 10)["frames_captured"], 2);
    assert_eq!(status.as_json(true, 10)["frames_replaced"], 1);
}

#[rstest]
fn live_observation_state_reports_recent_rate_and_capture_cost() {
    let started = Instant::now();
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1], 1, 1, started),
        Duration::from_millis(6),
        "test_capture",
    );
    status.publish_frame(
        LiveObservationFrame::new(2, vec![2], 1, 1, started + Duration::from_millis(100)),
        Duration::from_millis(8),
        "test_capture",
    );

    let state = status.as_json(true, 10);
    assert_eq!(state["active"], true);
    assert_eq!(state["target_fps"], 10);
    assert_eq!(state["recent_effective_fps"], 10.0);
    assert_eq!(state["last_capture_duration_ms"], 8);
    assert_eq!(state["max_capture_duration_ms"], 8);
    assert_eq!(state["capture_mode"], "test_capture");
}

#[rstest]
#[tokio::test]
async fn live_observation_first_frame_wait_preserves_the_terminal_error() {
    let mut status = LiveObservationStatus::default();
    status.record_terminal_error(&ComputerUseError::new(
        ComputerUseErrorCode::InvalidTarget,
        "live observation target process identity changed",
    ));
    let (_sender, mut receiver) = tokio::sync::watch::channel(status);

    let error = wait_for_latest_frame(&mut receiver, None, Duration::from_millis(10))
        .await
        .expect_err("terminal target loss must end the first-frame wait");

    assert_eq!(error.code, ComputerUseErrorCode::InvalidTarget);
    assert_eq!(
        error.message,
        "live observation target process identity changed"
    );
}

#[rstest]
#[tokio::test]
async fn live_observation_fresh_frame_wait_preserves_terminal_error_after_an_old_frame() {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(9, vec![9], 1, 1, Instant::now()),
        Duration::ZERO,
        "test_capture",
    );
    status.record_terminal_error(&ComputerUseError::new(
        ComputerUseErrorCode::InteractiveDesktopUnavailable,
        "Windows session is no longer actively connected",
    ));
    let (_sender, mut receiver) = tokio::sync::watch::channel(status);

    let error = wait_for_latest_frame(&mut receiver, Some(9), Duration::from_millis(10))
        .await
        .expect_err("terminal disconnect must end a wait for a frame newer than the old one");

    assert_eq!(
        error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert_eq!(
        error.message,
        "Windows session is no longer actively connected"
    );
}

#[rstest]
#[tokio::test]
async fn live_observation_never_returns_a_cached_frame_from_a_terminal_stream() {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(9, vec![9], 1, 1, Instant::now()),
        Duration::ZERO,
        "test_capture",
    );
    status.record_terminal_error(&ComputerUseError::new(
        ComputerUseErrorCode::InvalidTarget,
        "live observation target process identity changed",
    ));
    let (_sender, mut receiver) = tokio::sync::watch::channel(status);

    let error = wait_for_latest_frame(&mut receiver, None, Duration::from_millis(10))
        .await
        .expect_err("a cached pre-terminal frame must not masquerade as a fresh screenshot");

    assert_eq!(error.code, ComputerUseErrorCode::InvalidTarget);
}

#[rstest]
#[tokio::test]
async fn live_observation_returns_a_frame_newer_than_the_decision_frame() {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1], 1, 1, Instant::now()),
        Duration::ZERO,
        "test_capture",
    );
    let (sender, mut receiver) = tokio::sync::watch::channel(status);
    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(2, vec![2], 1, 1, Instant::now()),
            Duration::from_millis(7),
            "test_capture",
        );
    });

    let frame = wait_for_latest_frame(&mut receiver, Some(1), Duration::from_millis(10))
        .await
        .expect("fresh latest frame");
    assert_eq!(frame.sequence(), 2);
}

#[rstest]
#[tokio::test]
async fn live_observation_post_action_capture_skips_frames_available_at_action_completion() {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![1], 1, 1, Instant::now()),
        Duration::ZERO,
        "test_capture",
    );
    let (sender, mut receiver) = tokio::sync::watch::channel(status);
    sender.send_modify(|status| {
        status.publish_frame(
            LiveObservationFrame::new(2, vec![2], 1, 1, Instant::now()),
            Duration::ZERO,
            "test_capture",
        );
    });

    let after_sequence = post_action_sequence_fence(
        7,
        Some(LiveObservationFence::new(7, 1)),
        Some(LiveObservationFence::new(7, 2)),
    );
    let publish_after_action = tokio::spawn(async move {
        tokio::task::yield_now().await;
        sender.send_modify(|status| {
            status.publish_frame(
                LiveObservationFrame::new(3, vec![3], 1, 1, Instant::now()),
                Duration::ZERO,
                "test_capture",
            );
        });
    });

    let frame = wait_for_latest_frame(&mut receiver, after_sequence, Duration::from_millis(100))
        .await
        .expect("capture_after frame strictly newer than action completion");
    publish_after_action.await.unwrap();
    assert_eq!(frame.sequence(), 3);
}

#[rstest]
#[case("input_resumed")]
#[case("target_restored")]
#[tokio::test]
async fn live_observation_transition_fence_skips_frames_cached_before_safe_resume(
    #[case] _transition: &str,
) {
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(2, vec![2], 1, 1, Instant::now()),
        Duration::ZERO,
        "suspended_capture",
    );
    let (sender, mut receiver) = tokio::sync::watch::channel(status);
    let after_sequence =
        observation_sequence_fence(7, None, None, Some(LiveObservationFence::new(7, 2)));
    let publish_after_resume = tokio::spawn(async move {
        tokio::task::yield_now().await;
        sender.send_modify(|status| {
            status.publish_frame(
                LiveObservationFrame::new(3, vec![3], 1, 1, Instant::now()),
                Duration::ZERO,
                "resumed_capture",
            );
        });
    });

    let frame = wait_for_latest_frame(&mut receiver, after_sequence, Duration::from_millis(100))
        .await
        .expect("resume/restore must wait for a frame newer than its transition fence");
    publish_after_resume.await.unwrap();
    assert_eq!(frame.sequence(), 3);
}

#[rstest]
fn live_observation_restart_drops_fences_from_the_previous_stream() {
    let after_sequence = post_action_sequence_fence(
        8,
        Some(LiveObservationFence::new(7, 18_591)),
        Some(LiveObservationFence::new(7, 18_590)),
    );

    assert_eq!(after_sequence, None);
}
