use rstest::rstest;

use super::*;

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

#[rstest]
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

#[rstest]
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

#[rstest]
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

#[rstest]
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

#[rstest]
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

#[rstest]
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

#[rstest]
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

#[rstest]
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

#[rstest]
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

#[rstest]
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

#[rstest]
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

#[rstest]
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

#[rstest]
fn raw_drag_releases_the_button_when_a_path_move_fails() {
    let events = RefCell::new(Vec::new());

    let outcome = run_windows_separated_raw_drag_sequence(
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
    .expect("failed path movement must preserve the typed cleanup outcome");

    assert!(!outcome.path_sent);
    assert_eq!(outcome.primary_error, Some("path move failed"));
    assert_eq!(outcome.release_error, None);
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

#[rstest]
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

#[rstest]
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

#[rstest]
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

#[rstest]
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

#[rstest]
fn raw_drag_path_and_release_failures_are_both_preserved() {
    let outcome = run_windows_separated_raw_drag_sequence(
        || Ok::<_, &'static str>(()),
        |_| {},
        || Ok(1),
        |_| (),
        |_| true,
        || Err("path move failed"),
        || Err("button-up was not inserted"),
    )
    .expect("post-button-down failures must return a structured attempt outcome");

    assert!(!outcome.path_sent);
    assert_eq!(outcome.primary_error, Some("path move failed"));
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
#[rstest]
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
#[rstest]
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
#[rstest]
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
#[rstest]
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
        primary_error: None,
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
#[rstest]
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
        primary_error: None,
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
#[rstest]
fn separated_raw_drag_delivery_preserves_primary_and_cleanup_failures() {
    let outcome = RawDragSequenceOutcome::<_, String> {
        trace: WindowsRawDragInputTrace::new("left", 1, exact_raw_input_snapshot(true)),
        path_sent: false,
        primary_error: Some("absolute path injection failed".into()),
        release_error: Some("button-up was not inserted".into()),
    };

    let delivery = windows_raw_drag_delivery(&outcome, &test_window_target());
    assert_eq!(delivery["api_accepted"], false);
    assert_eq!(delivery["failure_phase"], "absolute_path");
    assert_eq!(delivery["primary_error"], "absolute path injection failed");
    assert_eq!(delivery["cleanup_error"], "button-up was not inserted");
    assert_eq!(delivery["release_succeeded"], false);
    assert_eq!(delivery["retry_safe"], false);
}

#[cfg(windows)]
#[rstest]
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
        primary_error: None,
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
