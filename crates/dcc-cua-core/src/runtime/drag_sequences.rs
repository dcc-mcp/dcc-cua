use super::*;

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RawDragSequenceOutcome<T, E> {
    pub trace: T,
    pub path_sent: bool,
    pub release_error: Option<E>,
}

#[cfg(any(windows, test))]
impl<T, E> RawDragSequenceOutcome<T, E> {
    fn completed(trace: T) -> Self {
        Self {
            trace,
            path_sent: true,
            release_error: None,
        }
    }
}

#[cfg(any(windows, test))]
pub(crate) fn run_windows_separated_raw_drag_sequence<E, T>(
    inject_consumable_source_move: impl FnOnce() -> Result<(), E>,
    mut settle: impl FnMut(Duration),
    inject_single_down: impl FnOnce() -> Result<u32, E>,
    inspect_after_down: impl FnOnce(u32) -> T,
    allows_drag_path: impl FnOnce(&T) -> bool,
    move_path: impl FnOnce() -> Result<(), E>,
    inject_single_up: impl FnOnce() -> Result<(), E>,
) -> Result<RawDragSequenceOutcome<T, E>, E> {
    inject_consumable_source_move()?;
    settle(RAW_DRAG_PRE_DOWN_SETTLE);
    let inserted = inject_single_down()?;
    let trace = inspect_after_down(inserted);
    if !allows_drag_path(&trace) {
        let release_result = inject_single_up();
        if release_result.is_ok() {
            settle(RAW_DRAG_POST_UP_SETTLE);
        }
        return Ok(RawDragSequenceOutcome {
            trace,
            path_sent: false,
            release_error: release_result.err(),
        });
    }
    settle(RAW_DRAG_PRE_DOWN_SETTLE);

    let path_result = move_path();
    if path_result.is_ok() {
        settle(RAW_DRAG_DROP_SETTLE);
    }
    let release_result = inject_single_up();
    if release_result.is_ok() {
        settle(RAW_DRAG_POST_UP_SETTLE);
    }
    match (path_result, release_result) {
        (Ok(()), Ok(())) => Ok(RawDragSequenceOutcome::completed(trace)),
        (Err(error), _) | (Ok(()), Err(error)) => Err(error),
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CombinedDownInjection<E> {
    pub(super) inserted: u32,
    pub(super) error: Option<E>,
}

#[cfg(any(windows, test))]
impl<E> CombinedDownInjection<E> {
    #[allow(dead_code)]
    pub(crate) fn accepted() -> Self {
        Self {
            inserted: 2,
            error: None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn incomplete(inserted: u32, error: E) -> Self {
        Self {
            inserted,
            error: Some(error),
        }
    }

    pub(crate) fn was_accepted(&self) -> bool {
        self.inserted == 2 && self.error.is_none()
    }

    #[allow(dead_code)]
    pub(crate) fn inserted(&self) -> u32 {
        self.inserted
    }

    fn owns_button_down(&self) -> bool {
        self.inserted == 2
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SingleInputInjection<E> {
    pub(super) inserted: u32,
    pub(super) error: Option<E>,
}

#[cfg(any(windows, test))]
impl<E> SingleInputInjection<E> {
    #[allow(dead_code)]
    pub(crate) fn accepted() -> Self {
        Self {
            inserted: 1,
            error: None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn incomplete(inserted: u32, error: E) -> Self {
        Self {
            inserted,
            error: Some(error),
        }
    }

    fn was_accepted(&self) -> bool {
        self.inserted == 1 && self.error.is_none()
    }
}

#[cfg(any(windows, test))]
pub(crate) struct CombinedDownDragPrelude<I, A, B> {
    inspect_pre_batch: I,
    allows_batch_injection: A,
    inject_absolute_move_and_left_down: B,
}

#[cfg(any(windows, test))]
impl<I, A, B> CombinedDownDragPrelude<I, A, B> {
    pub(crate) fn new<E, T>(
        inspect_pre_batch: I,
        allows_batch_injection: A,
        inject_absolute_move_and_left_down: B,
    ) -> Self
    where
        I: FnOnce() -> Result<T, E>,
        A: FnOnce(&T) -> bool,
        B: FnOnce() -> CombinedDownInjection<E>,
    {
        Self {
            inspect_pre_batch,
            allows_batch_injection,
            inject_absolute_move_and_left_down,
        }
    }
}

#[cfg(any(windows, test))]
pub(crate) struct CombinedDownDragAfterDown<I, A> {
    inspect_after_down: I,
    allows_drag_path: A,
}

#[cfg(any(windows, test))]
impl<I, A> CombinedDownDragAfterDown<I, A> {
    pub(crate) fn new<E, T>(inspect_after_down: I, allows_drag_path: A) -> Self
    where
        I: FnOnce(u32) -> Result<T, E>,
        A: FnOnce(&T) -> bool,
    {
        Self {
            inspect_after_down,
            allows_drag_path,
        }
    }
}

#[cfg(any(windows, test))]
pub(crate) struct CombinedDownDragCleanup<U, I, A> {
    inject_best_effort_left_up: U,
    inspect_post_up: I,
    post_up_allows_release: A,
}

#[cfg(any(windows, test))]
impl<U, I, A> CombinedDownDragCleanup<U, I, A> {
    pub(crate) fn new<E, P>(
        inject_best_effort_left_up: U,
        inspect_post_up: I,
        post_up_allows_release: A,
    ) -> Self
    where
        U: FnOnce() -> SingleInputInjection<E>,
        I: FnOnce() -> Result<P, E>,
        A: FnOnce(&P) -> bool,
    {
        Self {
            inject_best_effort_left_up,
            inspect_post_up,
            post_up_allows_release,
        }
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CombinedDownDragSequenceOutcome<T, P, E> {
    pub batch_inserted: u32,
    pub pre_batch: Option<T>,
    pub after_down: Option<T>,
    pub post_up: Option<P>,
    pub path_sent: bool,
    pub path_started: bool,
    pub path_moves_inserted: usize,
    pub waypoints_completed: usize,
    pub total_waypoints: usize,
    pub failure_phase: Option<&'static str>,
    pub failure_error: Option<E>,
    pub release_attempted: bool,
    pub release_inserted: Option<u32>,
    pub release_error: Option<E>,
    pub post_up_error: Option<E>,
    pub post_up_released: Option<bool>,
    pub cleanup_failure_phase: Option<&'static str>,
}

#[cfg(any(windows, test))]
fn finish_combined_down_drag_with_release<T, P, E>(
    mut outcome: CombinedDownDragSequenceOutcome<T, P, E>,
    settle: &mut impl FnMut(Duration),
    inject_best_effort_left_up: impl FnOnce() -> SingleInputInjection<E>,
    inspect_post_up: impl FnOnce() -> Result<P, E>,
    post_up_allows_release: impl FnOnce(&P) -> bool,
) -> CombinedDownDragSequenceOutcome<T, P, E> {
    outcome.release_attempted = true;
    let release = inject_best_effort_left_up();
    outcome.release_inserted = Some(release.inserted);
    let release_accepted = release.was_accepted();
    outcome.release_error = release.error;
    if !release_accepted {
        outcome.cleanup_failure_phase = Some("button_up_injection");
    }
    if release_accepted {
        settle(RAW_DRAG_POST_UP_SETTLE);
    }
    match inspect_post_up() {
        Ok(post_up) => {
            let released = post_up_allows_release(&post_up);
            outcome.post_up = Some(post_up);
            outcome.post_up_released = Some(released);
            if !released && outcome.cleanup_failure_phase.is_none() {
                outcome.cleanup_failure_phase = Some("post_button_up_fence");
            }
        }
        Err(error) => {
            outcome.post_up_error = Some(error);
            if outcome.cleanup_failure_phase.is_none() {
                outcome.cleanup_failure_phase = Some("post_button_up_probe");
            }
        }
    }
    outcome
}

#[cfg(any(windows, test))]
pub(crate) fn run_windows_combined_down_drag_sequence<E, T, P>(
    prelude: CombinedDownDragPrelude<
        impl FnOnce() -> Result<T, E>,
        impl FnOnce(&T) -> bool,
        impl FnOnce() -> CombinedDownInjection<E>,
    >,
    after_down: CombinedDownDragAfterDown<
        impl FnOnce(u32) -> Result<T, E>,
        impl FnOnce(&T) -> bool,
    >,
    move_absolute_path: impl FnOnce() -> Result<(), E>,
    mut settle: impl FnMut(Duration),
    cleanup: CombinedDownDragCleanup<
        impl FnOnce() -> SingleInputInjection<E>,
        impl FnOnce() -> Result<P, E>,
        impl FnOnce(&P) -> bool,
    >,
) -> CombinedDownDragSequenceOutcome<T, P, E> {
    let CombinedDownDragPrelude {
        inspect_pre_batch,
        allows_batch_injection,
        inject_absolute_move_and_left_down,
    } = prelude;
    let CombinedDownDragAfterDown {
        inspect_after_down,
        allows_drag_path,
    } = after_down;
    let CombinedDownDragCleanup {
        inject_best_effort_left_up,
        inspect_post_up,
        post_up_allows_release,
    } = cleanup;
    let mut outcome = CombinedDownDragSequenceOutcome {
        batch_inserted: 0,
        pre_batch: None,
        after_down: None,
        post_up: None,
        path_sent: false,
        path_started: false,
        path_moves_inserted: 0,
        waypoints_completed: 0,
        total_waypoints: 0,
        failure_phase: None,
        failure_error: None,
        release_attempted: false,
        release_inserted: None,
        release_error: None,
        post_up_error: None,
        post_up_released: None,
        cleanup_failure_phase: None,
    };
    let pre_batch = match inspect_pre_batch() {
        Ok(pre_batch) => pre_batch,
        Err(error) => {
            outcome.failure_phase = Some("pre_batch_target_fence");
            outcome.failure_error = Some(error);
            return outcome;
        }
    };
    let can_inject = allows_batch_injection(&pre_batch);
    outcome.pre_batch = Some(pre_batch);
    if !can_inject {
        outcome.failure_phase = Some("pre_batch_fence");
        return outcome;
    }

    let injection = inject_absolute_move_and_left_down();
    outcome.batch_inserted = injection.inserted;
    if !injection.was_accepted() {
        let owns_button_down = injection.owns_button_down();
        outcome.failure_phase = Some("combined_move_down_batch");
        outcome.failure_error = injection.error;
        return if owns_button_down {
            finish_combined_down_drag_with_release(
                outcome,
                &mut settle,
                inject_best_effort_left_up,
                inspect_post_up,
                post_up_allows_release,
            )
        } else {
            outcome
        };
    }

    let after_down = match inspect_after_down(injection.inserted) {
        Ok(after_down) => after_down,
        Err(error) => {
            outcome.failure_phase = Some("after_button_down_probe");
            outcome.failure_error = Some(error);
            return finish_combined_down_drag_with_release(
                outcome,
                &mut settle,
                inject_best_effort_left_up,
                inspect_post_up,
                post_up_allows_release,
            );
        }
    };
    let can_move = allows_drag_path(&after_down);
    outcome.after_down = Some(after_down);
    if !can_move {
        outcome.failure_phase = Some("after_button_down_fence");
        return finish_combined_down_drag_with_release(
            outcome,
            &mut settle,
            inject_best_effort_left_up,
            inspect_post_up,
            post_up_allows_release,
        );
    }

    settle(RAW_DRAG_PRE_DOWN_SETTLE);

    match move_absolute_path() {
        Ok(()) => {
            settle(RAW_DRAG_DROP_SETTLE);
            outcome.path_sent = true;
            outcome.path_started = true;
            outcome.path_moves_inserted = 1;
            outcome.waypoints_completed = 1;
            outcome.total_waypoints = 1;
        }
        Err(error) => {
            outcome.failure_phase = Some("absolute_path");
            outcome.failure_error = Some(error);
        }
    }
    finish_combined_down_drag_with_release(
        outcome,
        &mut settle,
        inject_best_effort_left_up,
        inspect_post_up,
        post_up_allows_release,
    )
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FencedAbsolutePathOutcome<E> {
    total_waypoints: usize,
    waypoints_completed: usize,
    moves_inserted: usize,
    error: Option<E>,
}

#[cfg(any(windows, test))]
impl<E> FencedAbsolutePathOutcome<E> {
    pub(crate) fn path_started(&self) -> bool {
        self.moves_inserted > 0
    }

    pub(crate) fn moves_inserted(&self) -> usize {
        self.moves_inserted
    }

    pub(crate) fn waypoints_completed(&self) -> usize {
        self.waypoints_completed
    }

    pub(crate) fn total_waypoints(&self) -> usize {
        self.total_waypoints
    }

    pub(crate) fn into_result(self) -> Result<(), E> {
        self.error.map_or(Ok(()), Err)
    }
}

#[cfg(any(windows, test))]
pub(crate) fn run_windows_fenced_absolute_path_with_trace<E>(
    source: (i32, i32),
    destination: (i32, i32),
    steps: usize,
    duration_ms: u64,
    mut validate_exact_target: impl FnMut() -> Result<(), E>,
    mut inject_absolute_move: impl FnMut(i32, i32) -> Result<(), E>,
    mut settle: impl FnMut(Duration),
) -> FencedAbsolutePathOutcome<E> {
    let steps = steps.max(1);
    let step_delay = Duration::from_millis(duration_ms / steps as u64);
    let mut outcome = FencedAbsolutePathOutcome {
        total_waypoints: steps,
        waypoints_completed: 0,
        moves_inserted: 0,
        error: None,
    };
    for step in 1..=steps {
        if let Err(error) = validate_exact_target() {
            outcome.error = Some(error);
            return outcome;
        }
        let progress = step as f64 / steps as f64;
        let x = source.0 + ((destination.0 - source.0) as f64 * progress).round() as i32;
        let y = source.1 + ((destination.1 - source.1) as f64 * progress).round() as i32;
        if let Err(error) = inject_absolute_move(x, y) {
            outcome.error = Some(error);
            return outcome;
        }
        outcome.moves_inserted += 1;
        outcome.waypoints_completed += 1;
        if !step_delay.is_zero() {
            settle(step_delay);
        }
    }
    outcome
}

#[allow(dead_code)]
pub(crate) fn run_windows_fenced_absolute_path<E>(
    source: (i32, i32),
    destination: (i32, i32),
    steps: usize,
    duration_ms: u64,
    validate_exact_target: impl FnMut() -> Result<(), E>,
    inject_absolute_move: impl FnMut(i32, i32) -> Result<(), E>,
    settle: impl FnMut(Duration),
) -> Result<(), E> {
    run_windows_fenced_absolute_path_with_trace(
        source,
        destination,
        steps,
        duration_ms,
        validate_exact_target,
        inject_absolute_move,
        settle,
    )
    .into_result()
}
