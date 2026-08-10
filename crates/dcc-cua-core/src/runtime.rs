use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use base64::Engine;
use cua_driver_sdk::CuaDriver;
use dcc_cua_indicator::{
    BannerActivity, BannerActivityGuard, BannerFailureKind, BannerTarget, ControlBanner,
    IndicatorError, localized_control_label,
};
use serde_json::{Value, json};

use crate::contracts::*;
use crate::interactive_desktop;
use crate::live_observation::{LiveObservation, LiveObservationFence, observation_sequence_fence};
use crate::observation::semantic_observation;
use crate::policy::*;
use crate::showcase::{ShowcaseRecorder, fit_dimensions_with_bounds, resize_bgra};
use crate::window_target::{WindowTarget, validate_target_policy};
#[cfg(windows)]
use crate::windows_uia_fallback::WindowsUiaFallback;

mod action_result;
pub(crate) mod application;
mod menu_commands;
mod recording;
pub(crate) use recording::{RecordingHealth, RecordingKeepalive, aggregate_recording_state};
mod session;
#[cfg(test)]
pub(crate) use session::{
    ensure_target_available_for_action, gated_cursor_operation, gated_desktop_observation,
    gated_exact_window_observation, gated_exact_window_publication,
    preflight_live_observation_start, resolved_application_name,
    run_gated_preinvalidated_window_mutation, run_preinvalidated_window_mutation,
};
mod window_commands;

#[cfg(any(not(windows), test))]
pub(crate) fn activation_completion_unknown(error: ComputerUseError) -> ComputerUseError {
    let detail = error
        .message
        .replace("; the window session was invalidated", "");
    ComputerUseError::new(
        ComputerUseErrorCode::CompletionUnknown,
        format!(
            "{detail}; completion_unknown=true; automatic_input=false; blind_retry=false; fresh_observation_required=true"
        ),
    )
}

#[cfg(windows)]
fn windows_platform_input_gate(
    stage: &'static str,
) -> Result<(), dcc_cua_platform_windows::UiaError> {
    interactive_desktop::require_input_available().map_err(|error| {
        dcc_cua_platform_windows::UiaError::PermissionDenied(format!(
            "input_gate_stage={stage}: {error}"
        ))
    })
}

#[cfg(windows)]
pub(crate) fn map_windows_window_mutation_error(
    context: &str,
    error: dcc_cua_platform_windows::UiaError,
) -> ComputerUseError {
    let code = match &error {
        dcc_cua_platform_windows::UiaError::PermissionDenied(message)
            if message.contains("input_gate_stage=") =>
        {
            ComputerUseErrorCode::InteractiveDesktopUnavailable
        }
        dcc_cua_platform_windows::UiaError::InvalidTarget(message)
            if message.contains("target_minimized:") =>
        {
            ComputerUseErrorCode::TargetMinimized
        }
        dcc_cua_platform_windows::UiaError::InvalidTarget(_) => {
            ComputerUseErrorCode::TargetUnavailable
        }
        _ => ComputerUseErrorCode::InputFailed,
    };
    ComputerUseError::new(code, format!("{context}: {error}"))
}

const INPUT_CALL_TIMEOUT: Duration = Duration::from_secs(15);
const CURSOR_GLIDE_MS: u64 = 180;
const SESSION_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const RECORDING_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(any(windows, test))]
const RAW_DRAG_PRE_DOWN_SETTLE: Duration = Duration::from_millis(50);
#[cfg(any(windows, test))]
const RAW_DRAG_DROP_SETTLE: Duration = Duration::from_millis(75);
#[cfg(any(windows, test))]
const RAW_DRAG_POST_UP_SETTLE: Duration = Duration::from_millis(50);
#[cfg(windows)]
pub(crate) const RELATIVE_DRAG_MAX_ATTEMPTS_PER_WAYPOINT: usize = 6;
#[cfg(windows)]
const RELATIVE_DRAG_ENDPOINT_TOLERANCE_PX: i32 = 0;
#[cfg(windows)]
const RELATIVE_DRAG_QUANTIZED_STALL_TOLERANCE_PX: i32 = 1;
#[cfg(windows)]
const RELATIVE_DRAG_INTERMEDIATE_TOLERANCE_PX: i32 = 1;
#[cfg(windows)]
const RELATIVE_DRAG_DAMPING_MIN_EFFECTIVE_COMMAND_PX: i32 = 2;
#[cfg(windows)]
const RELATIVE_DRAG_STAGNATION_ESCAPE_MAX_RESIDUAL_PX: i32 = 3;
#[cfg(windows)]
const RELATIVE_DRAG_STAGNATION_ESCAPE_MAX_COMMAND_PX: i32 = 4;

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
    inserted: u32,
    error: Option<E>,
}

#[cfg(any(windows, test))]
impl<E> CombinedDownInjection<E> {
    #[cfg(test)]
    pub(crate) fn accepted() -> Self {
        Self {
            inserted: 2,
            error: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn incomplete(inserted: u32, error: E) -> Self {
        Self {
            inserted,
            error: Some(error),
        }
    }

    pub(crate) fn was_accepted(&self) -> bool {
        self.inserted == 2 && self.error.is_none()
    }

    #[cfg(test)]
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
    inserted: u32,
    error: Option<E>,
}

#[cfg(any(windows, test))]
impl<E> SingleInputInjection<E> {
    #[cfg(test)]
    pub(crate) fn accepted() -> Self {
        Self {
            inserted: 1,
            error: None,
        }
    }

    #[cfg(test)]
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

#[cfg(test)]
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

#[cfg(windows)]
#[derive(Debug, Clone, serde::Serialize)]
struct WindowsSendInputCount {
    requested: u32,
    inserted: u32,
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct WindowsPostButtonUpSnapshot {
    pub(crate) async_button_down: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_fence: Option<dcc_cua_platform_windows::WindowsRawInputSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_fence_error: Option<String>,
}

#[cfg(windows)]
impl WindowsPostButtonUpSnapshot {
    pub(crate) fn new(
        async_button_down: bool,
        target_fence: Option<dcc_cua_platform_windows::WindowsRawInputSnapshot>,
        target_fence_error: Option<String>,
    ) -> Self {
        Self {
            async_button_down,
            target_fence,
            target_fence_error,
        }
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, serde::Serialize)]
struct WindowsRawDragCleanupTrace {
    attempted: bool,
    send_input: WindowsSendInputCount,
    #[serde(skip_serializing_if = "Option::is_none")]
    post_up: Option<WindowsPostButtonUpSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    injection_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    post_up_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_phase: Option<&'static str>,
    released: bool,
}

#[cfg(windows)]
#[derive(Debug, Clone, serde::Serialize)]
struct WindowsCombinedPathTrace {
    path_started: bool,
    path_moves_inserted: usize,
    waypoints_completed: usize,
    total_waypoints: usize,
    complete: bool,
}

#[cfg(windows)]
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct RelativeMoveInjection {
    requested: u32,
    inserted: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[cfg(windows)]
impl RelativeMoveInjection {
    #[cfg(test)]
    pub(crate) fn accepted() -> Self {
        Self {
            requested: 1,
            inserted: 1,
            error: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn incomplete(inserted: u32, error: impl Into<String>) -> Self {
        Self {
            requested: 1,
            inserted,
            error: Some(error.into()),
        }
    }

    fn was_accepted(&self) -> bool {
        self.requested == 1 && self.inserted == self.requested && self.error.is_none()
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, serde::Serialize)]
struct WindowsRelativeMoveTrace {
    waypoint_index: usize,
    attempt: usize,
    before: [i32; 2],
    target: [i32; 2],
    residual: [i32; 2],
    damping_applied: [bool; 2],
    stagnation_escape_applied: [bool; 2],
    delta: [i32; 2],
    requested: u32,
    inserted: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<[i32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor_moved: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    injection_error: Option<String>,
}

#[cfg(windows)]
#[derive(Debug, Clone, serde::Serialize)]
struct WindowsRelativeWaypointCompletion {
    waypoint_index: usize,
    target: [i32; 2],
    actual: [i32; 2],
    rule: &'static str,
    max_error_px: i64,
    attempt_budget: usize,
    attempts_used: usize,
    remaining_attempts: usize,
}

#[cfg(windows)]
#[derive(Debug, Clone, serde::Serialize)]
struct WindowsRelativeDragPathTrace {
    schema_version: u32,
    backend: &'static str,
    completion_policy: &'static str,
    tolerance_px: i32,
    quantized_stall_tolerance_px: i32,
    intermediate_tolerance_px: i32,
    damping_min_effective_command_px: i32,
    stagnation_escape_max_residual_px: i32,
    stagnation_escape_max_command_px: i32,
    max_attempts_per_waypoint: usize,
    move_attempt_budget: usize,
    move_attempts_used: usize,
    waypoint_count: usize,
    waypoints_reached: usize,
    fence_checks: usize,
    endpoint_reached: bool,
    endpoint_exact: bool,
    all_waypoints_exact: bool,
    moves: Vec<WindowsRelativeMoveTrace>,
    waypoint_completions: Vec<WindowsRelativeWaypointCompletion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_detail: Option<String>,
}

#[cfg(windows)]
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct WindowsRawDragInputTrace {
    schema_version: u32,
    backend: &'static str,
    backend_id: &'static str,
    button: String,
    send_input: WindowsSendInputCount,
    #[serde(skip_serializing_if = "Option::is_none")]
    batch_events: Option<[&'static str; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pre_batch: Option<dcc_cua_platform_windows::WindowsRawInputSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after_down: Option<dcc_cua_platform_windows::WindowsRawInputSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probe_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    relative_path: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_phase: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleanup: Option<WindowsRawDragCleanupTrace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    combined_path: Option<WindowsCombinedPathTrace>,
}

#[cfg(windows)]
impl WindowsRawDragInputTrace {
    pub(crate) fn new(
        button: &str,
        inserted: u32,
        after_down: dcc_cua_platform_windows::WindowsRawInputSnapshot,
    ) -> Self {
        Self {
            schema_version: 1,
            backend: "windows_send_input",
            backend_id: WINDOWS_SEND_INPUT_BACKEND_ID,
            button: button.to_owned(),
            send_input: WindowsSendInputCount {
                requested: 1,
                inserted,
            },
            batch_events: None,
            pre_batch: None,
            after_down: Some(after_down),
            probe_error: None,
            relative_path: None,
            failure_phase: None,
            failure_detail: None,
            cleanup: None,
            combined_path: None,
        }
    }

    fn probe_failed(button: &str, inserted: u32, error: impl Into<String>) -> Self {
        Self {
            schema_version: 1,
            backend: "windows_send_input",
            backend_id: WINDOWS_SEND_INPUT_BACKEND_ID,
            button: button.to_owned(),
            send_input: WindowsSendInputCount {
                requested: 1,
                inserted,
            },
            batch_events: None,
            pre_batch: None,
            after_down: None,
            probe_error: Some(error.into()),
            relative_path: None,
            failure_phase: None,
            failure_detail: None,
            cleanup: None,
            combined_path: None,
        }
    }

    fn combined(
        outcome: &CombinedDownDragSequenceOutcome<
            dcc_cua_platform_windows::WindowsRawInputSnapshot,
            WindowsPostButtonUpSnapshot,
            String,
        >,
    ) -> Self {
        let cleanup = outcome.release_attempted.then(|| {
            let released = outcome.release_inserted == Some(1)
                && outcome.release_error.is_none()
                && outcome.post_up_error.is_none()
                && outcome.post_up_released == Some(true);
            WindowsRawDragCleanupTrace {
                attempted: true,
                send_input: WindowsSendInputCount {
                    requested: 1,
                    inserted: outcome.release_inserted.unwrap_or(0),
                },
                post_up: outcome.post_up.clone(),
                injection_error: outcome.release_error.clone(),
                post_up_error: outcome.post_up_error.clone(),
                failure_phase: outcome.cleanup_failure_phase,
                released,
            }
        });
        Self {
            schema_version: 1,
            backend: "windows_send_input_combined_down_drag",
            backend_id: WINDOWS_COMBINED_DOWN_DRAG_BACKEND_ID,
            button: "left".to_owned(),
            send_input: WindowsSendInputCount {
                requested: 2,
                inserted: outcome.batch_inserted,
            },
            batch_events: Some(["absolute_virtual_desktop_move", "button_only_left_down"]),
            pre_batch: outcome.pre_batch.clone(),
            after_down: outcome.after_down.clone(),
            probe_error: None,
            relative_path: None,
            failure_phase: outcome.failure_phase,
            failure_detail: outcome.failure_error.clone(),
            cleanup,
            combined_path: Some(WindowsCombinedPathTrace {
                path_started: outcome.path_started,
                path_moves_inserted: outcome.path_moves_inserted,
                waypoints_completed: outcome.waypoints_completed,
                total_waypoints: outcome.total_waypoints,
                complete: outcome.path_sent,
            }),
        }
    }

    pub(crate) fn with_relative_path(mut self, relative_path: Value) -> Self {
        self.backend = "windows_send_input_relative";
        self.backend_id = WINDOWS_RELATIVE_SEND_INPUT_BACKEND_ID;
        self.relative_path = Some(relative_path);
        self
    }

    fn backend_id(&self) -> &'static str {
        self.backend_id
    }

    fn relative_path_attempted(&self) -> bool {
        self.relative_path.is_some()
    }

    fn input_sent(&self) -> bool {
        self.send_input.inserted > 0
    }

    fn path_started(&self) -> bool {
        self.combined_path
            .as_ref()
            .is_some_and(|path| path.path_started)
    }

    fn live_pre_batch_foreground_verified(&self, fallback: bool) -> bool {
        if self.backend_id == WINDOWS_COMBINED_DOWN_DRAG_BACKEND_ID {
            self.pre_batch.as_ref().is_some_and(|snapshot| {
                snapshot.foreground_relation
                    == dcc_cua_platform_windows::WindowsForegroundRelation::ExactTarget
            })
        } else {
            fallback
        }
    }

    fn release_succeeded(&self, release_error: Option<&String>) -> Option<bool> {
        if self.backend_id == WINDOWS_COMBINED_DOWN_DRAG_BACKEND_ID {
            self.cleanup.as_ref().map(|cleanup| cleanup.released)
        } else {
            Some(release_error.is_none())
        }
    }

    fn delivery_failure_phase(
        &self,
        path_sent: bool,
        release_succeeded: Option<bool>,
    ) -> Option<&'static str> {
        if !path_sent {
            self.failure_phase.or_else(|| {
                if self.relative_path_attempted() {
                    Some("relative_path_calibration")
                } else {
                    Some("after_button_down_probe")
                }
            })
        } else if release_succeeded != Some(true) {
            self.cleanup
                .as_ref()
                .and_then(|cleanup| cleanup.failure_phase)
                .or(Some("button_up_cleanup"))
        } else {
            None
        }
    }

    fn allows_drag_path(&self) -> bool {
        self.send_input.inserted == self.send_input.requested
            && self
                .after_down
                .as_ref()
                .is_some_and(dcc_cua_platform_windows::WindowsRawInputSnapshot::allows_drag_path)
    }
}

#[cfg(windows)]
type WindowsRawDragOutcome = RawDragSequenceOutcome<WindowsRawDragInputTrace, String>;

#[cfg(windows)]
pub(crate) fn windows_combined_raw_drag_outcome(
    sequence: CombinedDownDragSequenceOutcome<
        dcc_cua_platform_windows::WindowsRawInputSnapshot,
        WindowsPostButtonUpSnapshot,
        String,
    >,
) -> WindowsRawDragOutcome {
    let release_error = if !sequence.release_attempted {
        None
    } else {
        sequence
            .release_error
            .clone()
            .or_else(|| sequence.post_up_error.clone())
            .or_else(|| {
                (sequence.release_inserted != Some(1)).then(|| {
                    format!(
                        "combined drag LEFTUP inserted {}/1 events",
                        sequence.release_inserted.unwrap_or(0)
                    )
                })
            })
            .or_else(|| {
                (sequence.post_up_released != Some(true))
                    .then(|| "left button remained down after combined drag cleanup".to_owned())
            })
    };
    let trace = WindowsRawDragInputTrace::combined(&sequence);
    RawDragSequenceOutcome {
        trace,
        path_sent: sequence.path_sent,
        release_error,
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
struct WindowsRawDragPath {
    source: (i32, i32),
    destination: (i32, i32),
    duration_ms: u64,
    steps: usize,
}

#[cfg(windows)]
fn cursor_axis_distance(actual: i32, expected: i32) -> i64 {
    (i64::from(actual) - i64::from(expected)).abs()
}

#[cfg(windows)]
fn cursor_reached(actual: (i32, i32), expected: (i32, i32), tolerance_px: i32) -> bool {
    let tolerance = i64::from(tolerance_px.max(0));
    cursor_axis_distance(actual.0, expected.0) <= tolerance
        && cursor_axis_distance(actual.1, expected.1) <= tolerance
}

#[cfg(windows)]
fn cursor_max_error_px(actual: (i32, i32), expected: (i32, i32)) -> i64 {
    cursor_axis_distance(actual.0, expected.0).max(cursor_axis_distance(actual.1, expected.1))
}

#[cfg(windows)]
fn configured_completion_rule(
    actual: (i32, i32),
    expected: (i32, i32),
    tolerance_px: i32,
) -> Option<&'static str> {
    if actual == expected {
        Some("exact")
    } else if cursor_reached(actual, expected, tolerance_px) {
        Some("configured_tolerance")
    } else {
        None
    }
}

#[cfg(windows)]
fn is_unit_relative_command(delta: (i32, i32)) -> bool {
    delta != (0, 0) && delta.0.unsigned_abs() <= 1 && delta.1.unsigned_abs() <= 1
}

#[cfg(windows)]
fn relative_cursor_delta(actual: i32, expected: i32) -> i32 {
    (i64::from(expected) - i64::from(actual)).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(windows)]
fn residual_crossed_target(previous: Option<i32>, current: i32) -> bool {
    previous.is_some_and(|previous| {
        previous != 0 && current != 0 && previous.signum() != current.signum()
    })
}

#[cfg(windows)]
fn relative_command_for_residual(residual: i32, damped: bool) -> i32 {
    if !damped {
        return residual;
    }
    let magnitude = i64::from(residual).abs();
    let minimum_effective_magnitude = if magnitude > 1 {
        i64::from(RELATIVE_DRAG_DAMPING_MIN_EFFECTIVE_COMMAND_PX)
    } else {
        1
    };
    let damped_magnitude = (magnitude / 2).max(minimum_effective_magnitude);
    (i64::from(residual.signum()) * damped_magnitude)
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(windows)]
fn bounded_relative_command_for_residual(
    residual: i32,
    crossed_target: bool,
    previous_command: Option<i32>,
    previous_motion: Option<i64>,
    trust_region_magnitude: i64,
) -> i32 {
    let legacy_command = relative_command_for_residual(residual, crossed_target);
    let residual_magnitude = i64::from(residual).abs();
    let legacy_magnitude = i64::from(legacy_command).abs();
    let adaptive_magnitude = previous_command
        .zip(previous_motion)
        .filter(|(command, motion)| {
            crossed_target
                && *command != 0
                && *motion != 0
                && i64::from(command.signum()) == motion.signum()
                && legacy_magnitude > i64::from(*command).abs()
        })
        .map(|(command, motion)| {
            let command_magnitude = i64::from(command).abs();
            let observed_motion_magnitude = motion.abs();
            let minimum_effective_magnitude = if residual_magnitude > 1 {
                i64::from(RELATIVE_DRAG_DAMPING_MIN_EFFECTIVE_COMMAND_PX)
            } else {
                1
            };
            residual_magnitude
                .saturating_mul(command_magnitude)
                .checked_div(observed_motion_magnitude)
                .unwrap_or(0)
                .max(minimum_effective_magnitude.min(command_magnitude))
                .min(command_magnitude)
        });
    let command_magnitude = adaptive_magnitude
        .unwrap_or(legacy_magnitude)
        .min(trust_region_magnitude.max(0));
    i64::from(residual.signum())
        .saturating_mul(command_magnitude)
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(windows)]
fn stagnation_escape_command(
    residual: i32,
    command: i32,
    previous_command: Option<i32>,
    previous_axis_moved: Option<bool>,
) -> (i32, bool) {
    let can_escape = residual != 0
        && residual.unsigned_abs() <= RELATIVE_DRAG_STAGNATION_ESCAPE_MAX_RESIDUAL_PX as u32
        && previous_axis_moved == Some(false)
        && previous_command
            .is_some_and(|previous| previous != 0 && previous.signum() == residual.signum());
    if !can_escape {
        return (command, false);
    }
    let previous_magnitude = previous_command.map_or(0, i32::unsigned_abs);
    let escaped_magnitude = command
        .unsigned_abs()
        .max(previous_magnitude.saturating_add(1))
        .min(RELATIVE_DRAG_STAGNATION_ESCAPE_MAX_COMMAND_PX as u32);
    let escaped = residual.signum() * escaped_magnitude as i32;
    (escaped, escaped != command)
}

#[cfg(windows)]
pub(crate) fn run_windows_calibrated_relative_path(
    waypoints: &[(i32, i32)],
    max_attempts_per_waypoint: usize,
    tolerance_px: i32,
    mut validate_target_fence: impl FnMut() -> Result<(), String>,
    mut cursor_position: impl FnMut() -> Result<(i32, i32), String>,
    mut send_relative: impl FnMut(i32, i32) -> RelativeMoveInjection,
    mut settle_waypoint: impl FnMut(),
) -> Value {
    let tolerance_px = tolerance_px.max(0);
    let mut trace = WindowsRelativeDragPathTrace {
        schema_version: 4,
        backend: "windows_send_input_relative",
        completion_policy: "configured_tolerance_or_intermediate_physical_pixel_or_observed_unit_stall",
        tolerance_px,
        quantized_stall_tolerance_px: RELATIVE_DRAG_QUANTIZED_STALL_TOLERANCE_PX,
        intermediate_tolerance_px: RELATIVE_DRAG_INTERMEDIATE_TOLERANCE_PX,
        damping_min_effective_command_px: RELATIVE_DRAG_DAMPING_MIN_EFFECTIVE_COMMAND_PX,
        stagnation_escape_max_residual_px: RELATIVE_DRAG_STAGNATION_ESCAPE_MAX_RESIDUAL_PX,
        stagnation_escape_max_command_px: RELATIVE_DRAG_STAGNATION_ESCAPE_MAX_COMMAND_PX,
        max_attempts_per_waypoint,
        move_attempt_budget: waypoints.len().saturating_mul(max_attempts_per_waypoint),
        move_attempts_used: 0,
        waypoint_count: waypoints.len(),
        waypoints_reached: 0,
        fence_checks: 0,
        endpoint_reached: waypoints.is_empty(),
        endpoint_exact: waypoints.is_empty(),
        all_waypoints_exact: waypoints.is_empty(),
        moves: Vec::new(),
        waypoint_completions: Vec::new(),
        failure: None,
        failure_detail: None,
    };
    let mut current = match cursor_position() {
        Ok(position) => position,
        Err(error) => {
            trace.failure = Some("cursor_position_unavailable");
            trace.failure_detail = Some(error);
            return json!(trace);
        }
    };

    let mut all_waypoints_exact = true;
    let mut final_waypoint_exact = waypoints.is_empty();
    for (waypoint_index, &target) in waypoints.iter().enumerate() {
        let is_final_waypoint = waypoint_index + 1 == waypoints.len();
        let move_attempt_start = trace.move_attempts_used;
        let mut completion_rule = configured_completion_rule(current, target, tolerance_px);
        let initial_residual = (
            relative_cursor_delta(current.0, target.0),
            relative_cursor_delta(current.1, target.1),
        );
        let trust_region_magnitude = (
            i64::from(initial_residual.0).abs(),
            i64::from(initial_residual.1).abs(),
        );
        let mut previous_residual: Option<(i32, i32)> = None;
        let mut previous_command: Option<(i32, i32)> = None;
        let mut previous_motion: Option<(i64, i64)> = None;
        let mut previous_axis_moved: Option<(bool, bool)> = None;
        for attempt in 1..=max_attempts_per_waypoint {
            if completion_rule.is_some() {
                break;
            }
            let residual = (
                relative_cursor_delta(current.0, target.0),
                relative_cursor_delta(current.1, target.1),
            );
            let previous_x = previous_residual.map(|value| value.0);
            let previous_y = previous_residual.map(|value| value.1);
            let damping_applied = [
                residual_crossed_target(previous_x, residual.0),
                residual_crossed_target(previous_y, residual.1),
            ];
            let bounded_delta = (
                bounded_relative_command_for_residual(
                    residual.0,
                    damping_applied[0],
                    previous_command.map(|value| value.0),
                    previous_motion.map(|value| value.0),
                    trust_region_magnitude.0,
                ),
                bounded_relative_command_for_residual(
                    residual.1,
                    damping_applied[1],
                    previous_command.map(|value| value.1),
                    previous_motion.map(|value| value.1),
                    trust_region_magnitude.1,
                ),
            );
            let (delta_x, escape_x) = stagnation_escape_command(
                residual.0,
                bounded_delta.0,
                previous_command.map(|value| value.0),
                previous_axis_moved.map(|value| value.0),
            );
            let (delta_y, escape_y) = stagnation_escape_command(
                residual.1,
                bounded_delta.1,
                previous_command.map(|value| value.1),
                previous_axis_moved.map(|value| value.1),
            );
            let delta = (delta_x, delta_y);
            let stagnation_escape_applied = [escape_x, escape_y];
            trace.fence_checks += 1;
            if let Err(error) = validate_target_fence() {
                trace.failure = Some("target_fence_lost");
                trace.failure_detail = Some(error);
                return json!(trace);
            }
            let injection = send_relative(delta.0, delta.1);
            trace.move_attempts_used += 1;
            let mut move_trace = WindowsRelativeMoveTrace {
                waypoint_index,
                attempt,
                before: [current.0, current.1],
                target: [target.0, target.1],
                residual: [residual.0, residual.1],
                damping_applied,
                stagnation_escape_applied,
                delta: [delta.0, delta.1],
                requested: injection.requested,
                inserted: injection.inserted,
                after: None,
                cursor_moved: None,
                injection_error: injection.error.clone(),
            };
            if !injection.was_accepted() {
                trace.failure = Some("send_input_incomplete");
                trace.failure_detail = injection.error;
                trace.moves.push(move_trace);
                return json!(trace);
            }
            let before = current;
            current = match cursor_position() {
                Ok(position) => position,
                Err(error) => {
                    trace.failure = Some("cursor_position_unavailable");
                    trace.failure_detail = Some(error);
                    trace.moves.push(move_trace);
                    return json!(trace);
                }
            };
            move_trace.after = Some([current.0, current.1]);
            let cursor_moved = current != before;
            let axis_moved = (current.0 != before.0, current.1 != before.1);
            let observed_motion = (
                i64::from(current.0) - i64::from(before.0),
                i64::from(current.1) - i64::from(before.1),
            );
            move_trace.cursor_moved = Some(cursor_moved);
            trace.moves.push(move_trace);
            previous_residual = Some(residual);
            previous_command = Some(delta);
            previous_motion = Some(observed_motion);
            previous_axis_moved = Some(axis_moved);
            completion_rule = configured_completion_rule(current, target, tolerance_px)
                .or_else(|| {
                    // Intermediate waypoints approximate a continuous path on the integer cursor
                    // lattice. One measured physical pixel is bounded path error, but the final
                    // release waypoint intentionally cannot use this completion rule.
                    (!is_final_waypoint
                        && cursor_moved
                        && cursor_reached(current, target, RELATIVE_DRAG_INTERMEDIATE_TOLERANCE_PX))
                    .then_some("intermediate_physical_pixel")
                })
                .or_else(|| {
                    (!cursor_moved
                        && is_unit_relative_command(delta)
                        && cursor_reached(
                            current,
                            target,
                            RELATIVE_DRAG_QUANTIZED_STALL_TOLERANCE_PX,
                        ))
                    .then_some("quantized_unit_stall")
                });
        }
        let Some(completion_rule) = completion_rule else {
            trace.failure = Some("endpoint_not_reached");
            return json!(trace);
        };
        all_waypoints_exact &= completion_rule == "exact";
        if is_final_waypoint {
            final_waypoint_exact = completion_rule == "exact";
        }
        let attempts_used = trace.move_attempts_used - move_attempt_start;
        trace.waypoints_reached += 1;
        trace
            .waypoint_completions
            .push(WindowsRelativeWaypointCompletion {
                waypoint_index,
                target: [target.0, target.1],
                actual: [current.0, current.1],
                rule: completion_rule,
                max_error_px: cursor_max_error_px(current, target),
                attempt_budget: max_attempts_per_waypoint,
                attempts_used,
                remaining_attempts: max_attempts_per_waypoint.saturating_sub(attempts_used),
            });
        settle_waypoint();
    }

    trace.endpoint_reached = true;
    trace.endpoint_exact = final_waypoint_exact;
    trace.all_waypoints_exact = all_waypoints_exact;
    json!(trace)
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowsForegroundDragBackend {
    SendInput,
    RelativeSendInput,
    CombinedDownDrag,
    SyntheticTouch,
}

#[cfg(windows)]
pub(crate) fn select_windows_foreground_drag_backend(
    action: &ComputerUseAction,
) -> Result<WindowsForegroundDragBackend, String> {
    if action.action != "drag" || action.delivery_mode.as_deref() != Some("foreground") {
        return Err("explicit Windows input backends support foreground drag only".into());
    }
    if action.element_index.is_some() || action.element_token.is_some() {
        return Err(
            "explicit Windows foreground drag backends require screenshot coordinates".into(),
        );
    }
    match action.input_backend_id.as_deref() {
        Some(WINDOWS_SEND_INPUT_BACKEND_ID) => Ok(WindowsForegroundDragBackend::SendInput),
        Some(WINDOWS_RELATIVE_SEND_INPUT_BACKEND_ID) => {
            Ok(WindowsForegroundDragBackend::RelativeSendInput)
        }
        None | Some(WINDOWS_COMBINED_DOWN_DRAG_BACKEND_ID) => {
            if action.button.as_deref().unwrap_or("left") != "left" {
                return Err(format!(
                    "{WINDOWS_COMBINED_DOWN_DRAG_BACKEND_ID} supports left-button foreground drag only"
                ));
            }
            if !action.modifiers.is_empty() {
                return Err(format!(
                    "{WINDOWS_COMBINED_DOWN_DRAG_BACKEND_ID} does not support drag modifiers"
                ));
            }
            if action.path.len() != 2 {
                return Err(format!(
                    "{WINDOWS_COMBINED_DOWN_DRAG_BACKEND_ID} requires exactly two path points"
                ));
            }
            Ok(WindowsForegroundDragBackend::CombinedDownDrag)
        }
        Some(WINDOWS_SYNTHETIC_TOUCH_BACKEND_ID) => {
            if action.button.as_deref().unwrap_or("left") != "left" {
                return Err(
                    "windows.synthetic_touch.v1 supports left-button foreground drag only".into(),
                );
            }
            Ok(WindowsForegroundDragBackend::SyntheticTouch)
        }
        Some(backend_id) => Err(format!(
            "unsupported Windows foreground drag input backend {backend_id:?}"
        )),
    }
}

pub(crate) fn explicit_input_backend_rejection(action: &ComputerUseAction) -> Option<String> {
    action.input_backend_id.as_ref()?;
    #[cfg(windows)]
    {
        select_windows_foreground_drag_backend(action).err()
    }
    #[cfg(not(windows))]
    {
        Some(format!(
            "input backend {:?} is not supported on this platform",
            action.input_backend_id.as_deref().unwrap_or_default()
        ))
    }
}

fn input_target_fence(target: &WindowTarget, foreground_verified: bool) -> Value {
    json!({
        "process_id": target.pid,
        "window_handle": target.window_id,
        "exact_window": true,
        "foreground_required": true,
        "foreground_verified": foreground_verified,
    })
}

pub(crate) fn input_backend_rejection_result(
    backend_id: &str,
    reason: &str,
    target: &WindowTarget,
) -> ComputerUseToolResult {
    ComputerUseToolResult {
        value: json!({
            "success": false,
            "route": "input_backend_selection",
            "delivery": {
                "mode": "foreground",
                "backend_id": backend_id,
                "api_accepted": false,
                "consumer_effect_confirmed": false,
                "completion_known": false,
                "verification_required": true,
                "retry_safe": false,
                "fallback_attempted": false,
                "rejection_reason": reason,
                "target_fence": input_target_fence(target, target.is_foreground),
            },
            "effect": "not_attempted",
        }),
        text: format!("Rejected input backend {backend_id:?} without attempting input: {reason}"),
        images: Vec::new(),
        degraded: true,
    }
}

#[cfg(windows)]
pub(crate) fn windows_raw_drag_delivery(
    outcome: &WindowsRawDragOutcome,
    target: &WindowTarget,
) -> Value {
    let release_succeeded = outcome
        .trace
        .release_succeeded(outcome.release_error.as_ref());
    let api_accepted = outcome.path_sent && release_succeeded == Some(true);
    let failure_phase = outcome
        .trace
        .delivery_failure_phase(outcome.path_sent, release_succeeded);
    let foreground_verified = outcome
        .trace
        .live_pre_batch_foreground_verified(target.is_foreground);
    json!({
        "mode": "foreground",
        "backend_id": outcome.trace.backend_id(),
        "api_accepted": api_accepted,
        "consumer_effect_confirmed": false,
        "completion_known": false,
        "confirmed": false,
        "input_sent": outcome.trace.input_sent(),
        "delivered": outcome.path_sent,
        "path_sent": outcome.path_sent,
        "retry_safe": false,
        "verification_required": true,
        "fallback_attempted": false,
        "release_succeeded": release_succeeded,
        "cleanup_error": outcome.release_error,
        "failure_phase": failure_phase,
        "target_fence": input_target_fence(target, foreground_verified),
        "input_trace": outcome.trace,
    })
}

#[cfg(windows)]
pub(crate) fn windows_synthetic_touch_result(
    api_result: Result<(), String>,
    target: &WindowTarget,
    foreground_verified: bool,
) -> ComputerUseToolResult {
    let api_accepted = api_result.is_ok();
    let backend_error = api_result.err();
    let mut delivery = json!({
        "mode": "foreground",
        "backend_id": WINDOWS_SYNTHETIC_TOUCH_BACKEND_ID,
        "api_accepted": api_accepted,
        "consumer_effect_confirmed": false,
        "completion_known": false,
        "verification_required": true,
        "retry_safe": false,
        "target_fence": input_target_fence(target, foreground_verified),
    });
    if let Some(error) = backend_error.as_deref() {
        delivery["backend_error"] = json!(error);
    }
    ComputerUseToolResult {
        value: json!({
            "success": api_accepted,
            "route": "windows_scoped_fast_input",
            "delivery": delivery,
            "effect": "unverifiable",
        }),
        text: if api_accepted {
            "The Windows synthetic-touch drag API accepted the scoped input path; verify the target effect before continuing.".into()
        } else {
            format!(
                "The Windows synthetic-touch drag API rejected the scoped input path: {}",
                backend_error.as_deref().unwrap_or("unknown backend error")
            )
        },
        images: Vec::new(),
        degraded: !api_accepted,
    }
}

#[cfg(windows)]
pub(crate) fn windows_synthetic_touch_attempt<Inject>(
    input_gate: ComputerUseResult<()>,
    inject: Inject,
    target: &WindowTarget,
) -> ComputerUseResult<ComputerUseToolResult>
where
    Inject: FnOnce() -> Result<(), String>,
{
    input_gate?;
    Ok(windows_synthetic_touch_result(inject(), target, true))
}

#[cfg(windows)]
pub(crate) fn windows_combined_source_move_and_left_down_inputs(
    source: (i32, i32),
    virtual_desktop: (i32, i32, i32, i32),
) -> [windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT; 2] {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_MOVE,
        MOUSEEVENTF_MOVE_NOCOALESCE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT,
    };

    let (virtual_x, virtual_y, virtual_width, virtual_height) = virtual_desktop;
    let (normalized_x, normalized_y) = platform_windows::virtualdesk::to_virtualdesk_absolute(
        source.0,
        source.1,
        virtual_x,
        virtual_y,
        virtual_width.max(1),
        virtual_height.max(1),
    );
    [
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: normalized_x,
                    dy: normalized_y,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE
                        | MOUSEEVENTF_MOVE_NOCOALESCE
                        | MOUSEEVENTF_ABSOLUTE
                        | MOUSEEVENTF_VIRTUALDESK,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_LEFTDOWN,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
    ]
}

#[cfg(windows)]
pub(crate) fn inject_windows_combined_input_batch_with(
    source: (i32, i32),
    virtual_desktop: (i32, i32, i32, i32),
    send_input: impl FnOnce(&[windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT]) -> u32,
    last_error: impl FnOnce() -> String,
) -> CombinedDownInjection<String> {
    let inputs = windows_combined_source_move_and_left_down_inputs(source, virtual_desktop);
    let inserted = send_input(&inputs);
    CombinedDownInjection {
        inserted,
        error: (inserted != inputs.len() as u32).then(|| {
            format!(
                "SendInput combined source MOVE + LEFTDOWN inserted {inserted}/{} events: {}",
                inputs.len(),
                last_error()
            )
        }),
    }
}

#[cfg(windows)]
fn windows_virtual_desktop() -> (i32, i32, i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
        )
    }
}

#[cfg(windows)]
fn inject_windows_combined_source_move_and_left_down(
    source: (i32, i32),
) -> CombinedDownInjection<String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{INPUT, SendInput};

    inject_windows_combined_input_batch_with(
        source,
        windows_virtual_desktop(),
        |inputs| unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        },
        || std::io::Error::last_os_error().to_string(),
    )
}

#[cfg(windows)]
fn inject_windows_direct_absolute_mouse_move(x: i32, y: i32) -> Result<(), String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{INPUT, SendInput};

    let inputs =
        windows_combined_source_move_and_left_down_inputs((x, y), windows_virtual_desktop());
    let inserted = unsafe { SendInput(1, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    if inserted == 1 {
        Ok(())
    } else {
        Err(format!(
            "SendInput direct absolute drag waypoint ({x}, {y}) inserted {inserted}/1 events: {}",
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(windows)]
fn inject_windows_best_effort_left_up() -> SingleInputInjection<String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTUP, MOUSEINPUT, SendInput,
    };

    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_LEFTUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inserted = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
    SingleInputInjection {
        inserted,
        error: (inserted != 1).then(|| {
            format!(
                "SendInput combined drag LEFTUP inserted {inserted}/1 events: {}",
                std::io::Error::last_os_error()
            )
        }),
    }
}

#[cfg(windows)]
fn snapshot_windows_left_button_after_up(
    target: dcc_cua_platform_windows::UiaTarget,
) -> WindowsPostButtonUpSnapshot {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};

    // Sample the button independently first. Even if the target HWND disappears after the path,
    // cleanup evidence must still say whether our owned LEFTDOWN was released.
    let async_button_down = unsafe { GetAsyncKeyState(i32::from(VK_LBUTTON)) } as u16 & 0x8000 != 0;
    match dcc_cua_platform_windows::snapshot_raw_pointer_input_after_down(
        target,
        dcc_cua_platform_windows::WindowsPointerButton::Left,
    ) {
        Ok(target_fence) => {
            WindowsPostButtonUpSnapshot::new(async_button_down, Some(target_fence), None)
        }
        Err(error) => WindowsPostButtonUpSnapshot::new(
            async_button_down,
            None,
            Some(format!("sample exact target fence after LEFTUP: {error}")),
        ),
    }
}

#[cfg(windows)]
fn inject_consumable_windows_mouse_move(x: i32, y: i32) -> Result<(), String> {
    use windows_sys::Win32::UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE,
            MOUSEEVENTF_MOVE_NOCOALESCE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
        },
        WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN, SetCursorPos,
        },
    };

    let (virtual_x, virtual_y, virtual_width, virtual_height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
        )
    };
    let (normalized_x, normalized_y) = platform_windows::virtualdesk::to_virtualdesk_absolute(
        x,
        y,
        virtual_x,
        virtual_y,
        virtual_width,
        virtual_height,
    );
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: normalized_x,
                dy: normalized_y,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE
                    | MOUSEEVENTF_MOVE_NOCOALESCE
                    | MOUSEEVENTF_ABSOLUTE
                    | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    if unsafe { SetCursorPos(x, y) } == 0 {
        return Err(format!(
            "SetCursorPos({x}, {y}) before raw drag failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let sent = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
    if sent != 1 {
        return Err(format!(
            "SendInput source move inserted {sent}/1 events before raw drag: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_cursor_position() -> Result<(i32, i32), String> {
    use windows_sys::Win32::{Foundation::POINT, UI::WindowsAndMessaging::GetCursorPos};

    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&raw mut point) } == 0 {
        return Err(format!(
            "GetCursorPos while calibrating relative drag failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok((point.x, point.y))
}

#[cfg(windows)]
fn inject_windows_relative_mouse_move(dx: i32, dy: i32) -> RelativeMoveInjection {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEEVENTF_MOVE_NOCOALESCE, MOUSEINPUT,
        SendInput,
    };

    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                // Deliberately omit ABSOLUTE and VIRTUALDESK. This backend exists to test the
                // relative packet shape some game consumers distinguish from absolute motion.
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_MOVE_NOCOALESCE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inserted = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
    RelativeMoveInjection {
        requested: 1,
        inserted,
        error: (inserted != 1).then(|| {
            format!(
                "SendInput relative drag move ({dx}, {dy}) inserted {inserted}/1 events: {}",
                std::io::Error::last_os_error()
            )
        }),
    }
}

#[cfg(windows)]
fn validate_windows_relative_drag_fence(
    target: dcc_cua_platform_windows::UiaTarget,
    button: dcc_cua_platform_windows::WindowsPointerButton,
) -> Result<(), String> {
    let snapshot = dcc_cua_platform_windows::snapshot_raw_pointer_input_after_down(target, button)
        .map_err(|error| format!("revalidate exact Windows drag target: {error}"))?;
    if snapshot.allows_drag_path() {
        Ok(())
    } else {
        Err(format!(
            "exact target drag fence rejected movement (async_button_down={}, \
             foreground_relation={:?})",
            snapshot.async_button_down, snapshot.foreground_relation
        ))
    }
}

#[cfg(windows)]
fn inject_single_windows_mouse_button(button: &str, pressed: bool) -> Result<u32, String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
        MOUSEINPUT, SendInput,
    };

    let flags = match (button, pressed) {
        ("right", true) => MOUSEEVENTF_RIGHTDOWN,
        ("right", false) => MOUSEEVENTF_RIGHTUP,
        ("middle", true) => MOUSEEVENTF_MIDDLEDOWN,
        ("middle", false) => MOUSEEVENTF_MIDDLEUP,
        (_, true) => MOUSEEVENTF_LEFTDOWN,
        (_, false) => MOUSEEVENTF_LEFTUP,
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
    if sent != 1 {
        let phase = if pressed { "button-down" } else { "button-up" };
        return Err(format!(
            "SendInput raw drag {phase} inserted {sent}/1 events: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(sent)
}

#[cfg(windows)]
fn send_windows_separated_raw_drag(
    target: dcc_cua_platform_windows::UiaTarget,
    path: WindowsRawDragPath,
    button: &str,
) -> Result<WindowsRawDragOutcome, String> {
    let steps = path.steps.max(1);
    let step_delay = Duration::from_millis(path.duration_ms / steps as u64);
    let (from_x, from_y) = path.source;
    let (to_x, to_y) = path.destination;
    let pointer_button = match button {
        "right" => dcc_cua_platform_windows::WindowsPointerButton::Right,
        "middle" => dcc_cua_platform_windows::WindowsPointerButton::Middle,
        _ => dcc_cua_platform_windows::WindowsPointerButton::Left,
    };
    run_windows_separated_raw_drag_sequence(
        || inject_consumable_windows_mouse_move(from_x, from_y),
        std::thread::sleep,
        || inject_single_windows_mouse_button(button, true),
        |inserted| match dcc_cua_platform_windows::snapshot_raw_pointer_input_after_down(
            target,
            pointer_button,
        ) {
            Ok(after_down) => WindowsRawDragInputTrace::new(button, inserted, after_down),
            Err(error) => WindowsRawDragInputTrace::probe_failed(
                button,
                inserted,
                format!("sample Windows raw drag after button-down: {error}"),
            ),
        },
        WindowsRawDragInputTrace::allows_drag_path,
        || {
            for step in 1..=steps {
                let progress = step as f64 / steps as f64;
                let x = from_x + ((to_x - from_x) as f64 * progress).round() as i32;
                let y = from_y + ((to_y - from_y) as f64 * progress).round() as i32;
                inject_consumable_windows_mouse_move(x, y)?;
                if !step_delay.is_zero() {
                    std::thread::sleep(step_delay);
                }
            }
            Ok(())
        },
        || inject_single_windows_mouse_button(button, false).map(|_| ()),
    )
}

#[cfg(windows)]
fn send_windows_combined_down_drag(
    target: dcc_cua_platform_windows::UiaTarget,
    path: WindowsRawDragPath,
) -> WindowsRawDragOutcome {
    use dcc_cua_platform_windows::{WindowsForegroundRelation, WindowsPointerButton};

    let path_started = std::cell::Cell::new(false);
    let path_moves_inserted = std::cell::Cell::new(0_usize);
    let waypoints_completed = std::cell::Cell::new(0_usize);
    let total_waypoints = std::cell::Cell::new(path.steps.max(1));
    let mut sequence = run_windows_combined_down_drag_sequence(
        CombinedDownDragPrelude::new(
            || {
                dcc_cua_platform_windows::snapshot_raw_pointer_input_after_down(
                    target,
                    WindowsPointerButton::Left,
                )
                .map_err(|error| format!("validate exact pre-batch Windows drag target: {error}"))
            },
            |snapshot| {
                !snapshot.async_button_down
                    && snapshot.foreground_relation == WindowsForegroundRelation::ExactTarget
            },
            || inject_windows_combined_source_move_and_left_down(path.source),
        ),
        CombinedDownDragAfterDown::new(
            |_| {
                dcc_cua_platform_windows::snapshot_raw_pointer_input_after_down(
                    target,
                    WindowsPointerButton::Left,
                )
                .map_err(|error| format!("sample combined drag after LEFTDOWN: {error}"))
            },
            dcc_cua_platform_windows::WindowsRawInputSnapshot::allows_drag_path,
        ),
        || {
            let outcome = run_windows_fenced_absolute_path_with_trace(
                path.source,
                path.destination,
                path.steps,
                path.duration_ms,
                || validate_windows_relative_drag_fence(target, WindowsPointerButton::Left),
                inject_windows_direct_absolute_mouse_move,
                std::thread::sleep,
            );
            path_started.set(outcome.path_started());
            path_moves_inserted.set(outcome.moves_inserted());
            waypoints_completed.set(outcome.waypoints_completed());
            total_waypoints.set(outcome.total_waypoints());
            outcome.into_result()
        },
        std::thread::sleep,
        CombinedDownDragCleanup::new(
            inject_windows_best_effort_left_up,
            || Ok(snapshot_windows_left_button_after_up(target)),
            |snapshot| !snapshot.async_button_down,
        ),
    );
    sequence.path_started = path_started.get();
    sequence.path_moves_inserted = path_moves_inserted.get();
    sequence.waypoints_completed = waypoints_completed.get();
    sequence.total_waypoints = total_waypoints.get();
    windows_combined_raw_drag_outcome(sequence)
}

#[cfg(windows)]
fn send_windows_calibrated_relative_drag(
    target: dcc_cua_platform_windows::UiaTarget,
    path: WindowsRawDragPath,
    button: &str,
) -> Result<WindowsRawDragOutcome, String> {
    let steps = path.steps.max(1);
    let step_delay = Duration::from_millis(path.duration_ms / steps as u64);
    let (from_x, from_y) = path.source;
    let (to_x, to_y) = path.destination;
    let pointer_button = match button {
        "right" => dcc_cua_platform_windows::WindowsPointerButton::Right,
        "middle" => dcc_cua_platform_windows::WindowsPointerButton::Middle,
        _ => dcc_cua_platform_windows::WindowsPointerButton::Left,
    };

    inject_consumable_windows_mouse_move(from_x, from_y)?;
    std::thread::sleep(RAW_DRAG_PRE_DOWN_SETTLE);
    let inserted = inject_single_windows_mouse_button(button, true)?;
    let mut trace = match dcc_cua_platform_windows::snapshot_raw_pointer_input_after_down(
        target,
        pointer_button,
    ) {
        Ok(after_down) => WindowsRawDragInputTrace::new(button, inserted, after_down),
        Err(error) => WindowsRawDragInputTrace::probe_failed(
            button,
            inserted,
            format!("sample Windows relative drag after button-down: {error}"),
        ),
    };
    if !trace.allows_drag_path() {
        let release_result = inject_single_windows_mouse_button(button, false).map(|_| ());
        if release_result.is_ok() {
            std::thread::sleep(RAW_DRAG_POST_UP_SETTLE);
        }
        return Ok(RawDragSequenceOutcome {
            trace,
            path_sent: false,
            release_error: release_result.err(),
        });
    }
    std::thread::sleep(RAW_DRAG_PRE_DOWN_SETTLE);

    let waypoints: Vec<_> = (1..=steps)
        .map(|step| {
            let progress = step as f64 / steps as f64;
            (
                from_x + ((to_x - from_x) as f64 * progress).round() as i32,
                from_y + ((to_y - from_y) as f64 * progress).round() as i32,
            )
        })
        .collect();
    let relative_path = run_windows_calibrated_relative_path(
        &waypoints,
        RELATIVE_DRAG_MAX_ATTEMPTS_PER_WAYPOINT,
        RELATIVE_DRAG_ENDPOINT_TOLERANCE_PX,
        || validate_windows_relative_drag_fence(target, pointer_button),
        windows_cursor_position,
        inject_windows_relative_mouse_move,
        || {
            if !step_delay.is_zero() {
                std::thread::sleep(step_delay);
            }
        },
    );
    let path_sent = relative_path["endpoint_reached"].as_bool() == Some(true);
    trace = trace.with_relative_path(relative_path);
    if path_sent {
        std::thread::sleep(RAW_DRAG_DROP_SETTLE);
    }
    let release_result = inject_single_windows_mouse_button(button, false).map(|_| ());
    if release_result.is_ok() {
        std::thread::sleep(RAW_DRAG_POST_UP_SETTLE);
    }

    Ok(RawDragSequenceOutcome {
        trace,
        path_sent,
        release_error: release_result.err(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionBannerPhase {
    Preparing,
    Injecting,
}

pub(crate) fn banner_activity_for_action_phase(
    action: &ComputerUseAction,
    phase: ActionBannerPhase,
) -> BannerActivity {
    match phase {
        ActionBannerPhase::Preparing => BannerActivity::Operating,
        ActionBannerPhase::Injecting => match action.action.as_str() {
            "type" | "type_chars" | "set_text" | "set_value" | "keypress" | "keyboard_shortcut" => {
                BannerActivity::KeyboardInput
            }
            _ => BannerActivity::PointerInput,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveObservationStartDisposition {
    ReuseExisting,
    StartedNew,
}

pub(crate) fn live_observation_start_disposition(
    state: Option<&Value>,
) -> LiveObservationStartDisposition {
    if state.is_some_and(|state| state["active"] == true && state["terminal_reason"].is_null()) {
        LiveObservationStartDisposition::ReuseExisting
    } else {
        LiveObservationStartDisposition::StartedNew
    }
}

pub(crate) fn banner_activity_for_bound_tool(name: &str) -> BannerActivity {
    match name {
        "browser_navigate" | "browser_prepare" => BannerActivity::Navigating,
        "get_browser_state" => BannerActivity::Observing,
        _ => BannerActivity::Operating,
    }
}

pub(crate) fn attach_banner_status(mut value: Value, banner: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("banner".into(), banner);
        value
    } else {
        json!({"cua": value, "banner": banner})
    }
}

pub(crate) fn attach_indicator_motion_to_activation(
    mut activation: Value,
    banner: &Value,
) -> Value {
    let Some(motion) = banner.get("motion") else {
        return activation;
    };
    if let Some(object) = activation.as_object_mut() {
        object.insert("indicator_motion".into(), motion.clone());
        activation
    } else {
        json!({"cua": activation, "indicator_motion": motion})
    }
}

pub(crate) fn map_indicator_error(context: &str, error: IndicatorError) -> ComputerUseError {
    let code = match error {
        IndicatorError::InvalidTarget(_) => ComputerUseErrorCode::InvalidTarget,
        IndicatorError::Backend(_) => ComputerUseErrorCode::BackendUnavailable,
    };
    ComputerUseError::new(code, format!("{context}: {error}"))
}

pub(crate) fn held_coordinate_click_as_drag(
    action: &ComputerUseAction,
) -> Option<ComputerUseAction> {
    let (Some(duration_ms), Some(x), Some(y)) = (action.duration_ms, action.x, action.y) else {
        return None;
    };
    if action.action != "click" || duration_ms == 0 {
        return None;
    }

    let mut drag = action.clone();
    drag.action = "drag".into();
    drag.x = None;
    drag.y = None;
    drag.path = vec![ComputerUsePoint { x, y }; 2];
    drag.steps = Some(20);
    Some(drag)
}

async fn call_driver_tool(
    driver: &CuaDriver,
    name: impl Into<String>,
    arguments: String,
    operation: &str,
) -> ComputerUseResult<cua_driver_sdk::ToolResult> {
    call_driver_tool_with_timeout(driver, name, arguments, operation, INPUT_CALL_TIMEOUT).await
}

async fn call_driver_tool_with_timeout(
    driver: &CuaDriver,
    name: impl Into<String>,
    arguments: String,
    operation: &str,
    timeout: Duration,
) -> ComputerUseResult<cua_driver_sdk::ToolResult> {
    let name = name.into();
    let result = await_input_call(driver.call_tool(name, arguments), timeout, operation).await?;
    result.map_err(|error| map_driver_error(operation, error))
}

pub(crate) async fn await_input_call<T>(
    future: impl Future<Output = T>,
    timeout: Duration,
    operation: &str,
) -> ComputerUseResult<T> {
    tokio::time::timeout(timeout, future).await.map_err(|_| {
        ComputerUseError::new(
            ComputerUseErrorCode::InputFailed,
            format!(
                "CUA {operation} timed out after {} ms; the window session was invalidated",
                timeout.as_millis()
            ),
        )
    })
}

/// One shared CUA runtime. Create it once per host process.
#[derive(Clone)]
pub struct ComputerUseDriver {
    driver: Arc<CuaDriver>,
    upstream_cursor_renderer_enabled: bool,
    // ponytail: cache the static tool registry for this driver; recreate the
    // driver if a future SDK supports runtime tool registration.
    tool_inventory: Arc<tokio::sync::OnceCell<Value>>,
}

impl ComputerUseDriver {
    pub fn create() -> ComputerUseResult<Self> {
        crate::driver_factory::ensure_bundled_cursor_theme()?;
        crate::driver_factory::create_embedded()
            .map(Self::from_driver)
            .map_err(|error| map_driver_error("create CUA runtime", error))
    }

    /// Create a directly supervised official CUA worker.
    ///
    /// The packaged macOS Host uses this upstream-owned process boundary so
    /// CUA can keep AppKit on the worker's main thread and render its native
    /// per-session cursor without turning the Host itself into an app bundle.
    pub fn create_private_worker(options: PrivateWorkerOptions) -> ComputerUseResult<Self> {
        crate::driver_factory::ensure_bundled_cursor_theme()?;
        crate::driver_factory::create_private_worker(options)
            .map(Self::from_driver)
            .map_err(|error| map_driver_error("create CUA private worker", error))
    }

    /// Create a configured runtime with a trusted host authorization callback.
    ///
    /// The callback is constructor-only and must be owned by Core or another
    /// trusted embedding host. It is never exposed through CUA tools or Host
    /// IPC, and returning `Allow` must require an explicit user decision.
    pub fn create_with_authorization_host(
        options: ConfiguredDriverOptions,
        host: Arc<dyn DriverAuthorizationHost>,
    ) -> ComputerUseResult<Self> {
        crate::driver_factory::ensure_bundled_cursor_theme()?;
        crate::driver_factory::create_authorized(options, host)
            .map(Self::from_driver)
            .map_err(|error| map_driver_error("create authorized CUA runtime", error))
    }

    fn from_driver((driver, upstream_cursor_renderer_enabled): (Arc<CuaDriver>, bool)) -> Self {
        Self {
            driver,
            upstream_cursor_renderer_enabled,
            tool_inventory: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    #[must_use]
    pub const fn upstream_cursor_renderer_enabled(&self) -> bool {
        self.upstream_cursor_renderer_enabled
    }

    pub fn session(
        &self,
        scope: ComputerUseTargetScope,
        app_name: impl Into<String>,
        session_id: impl Into<String>,
    ) -> ComputerUseResult<ComputerUseSession> {
        self.session_with_agent(scope, app_name, "Agent", session_id)
    }

    pub fn session_with_agent(
        &self,
        scope: ComputerUseTargetScope,
        app_name: impl Into<String>,
        agent_name: impl Into<String>,
        session_id: impl Into<String>,
    ) -> ComputerUseResult<ComputerUseSession> {
        ComputerUseSession::new(
            self.clone(),
            scope,
            app_name.into(),
            agent_name.into(),
            session_id.into(),
        )
    }

    pub fn desktop_session(
        &self,
        session_id: impl Into<String>,
    ) -> ComputerUseResult<ComputerUseDesktopSession> {
        self.desktop_session_with_agent("Agent", session_id)
    }

    pub fn desktop_session_with_agent(
        &self,
        agent_name: impl Into<String>,
        session_id: impl Into<String>,
    ) -> ComputerUseResult<ComputerUseDesktopSession> {
        ComputerUseDesktopSession::new(self.clone(), agent_name.into(), session_id.into())
    }

    pub fn raw(&self) -> &Arc<CuaDriver> {
        &self.driver
    }

    /// List the currently visible native windows through the CUA runtime.
    pub async fn list_windows(&self) -> ComputerUseResult<Vec<Value>> {
        self.list_windows_filtered(None, false).await
    }

    /// Enumerate windows with filters applied by the native CUA backend.
    pub async fn list_windows_filtered(
        &self,
        pid: Option<u32>,
        on_screen_only: bool,
    ) -> ComputerUseResult<Vec<Value>> {
        list_windows_with_driver(&self.driver, pid, on_screen_only).await
    }

    #[cfg(not(windows))]
    pub(crate) async fn capture_exact_window_png(
        &self,
        process_id: u32,
        window_handle: u64,
        session_id: &str,
    ) -> ComputerUseResult<Vec<u8>> {
        let matches_target = |windows: &[Value]| {
            windows.iter().any(|window| {
                WindowTarget::from_value(window).is_some_and(|target| {
                    target.pid == process_id && target.window_id == window_handle
                })
            })
        };
        let before = self.list_windows_filtered(Some(process_id), false).await?;
        if !matches_target(&before) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                "live observation target identity changed",
            ));
        }
        let result = call_driver_tool(
            &self.driver,
            "get_window_state",
            json!({
                "window_id": window_handle,
                "pid": process_id,
                "include_screenshot": true,
                "max_elements": 1,
                "max_depth": 1,
                "session": session_id,
            })
            .to_string(),
            "capture portable live window frame",
        )
        .await?;
        ensure_tool_ok("capture portable live window frame", &result)?;
        let after = self.list_windows_filtered(Some(process_id), false).await?;
        if !matches_target(&after) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                "live observation target identity changed during capture",
            ));
        }
        let image = result.images.first().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "CUA live window state returned no screenshot",
            )
        })?;
        base64::engine::general_purpose::STANDARD
            .decode(&image.data_base64)
            .map_err(|error| {
                ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
            })
    }

    /// Wait until the bounded native window query returns at least one row.
    pub async fn wait_for_window(
        &self,
        request: &ComputerUseWindowWaitRequest,
    ) -> ComputerUseResult<Value> {
        let (timeout_ms, interval_ms) = request.limits()?;
        let started = Instant::now();
        loop {
            let mut windows = self
                .list_windows_filtered(request.query.process_id, request.query.on_screen_only)
                .await?;
            windows.retain(|window| request.query.matches_window(window));
            if !windows.is_empty() {
                return Ok(json!({
                    "windows": windows,
                    "count": windows.len(),
                    "waited_ms": started.elapsed().as_millis(),
                }));
            }
            if started.elapsed() >= Duration::from_millis(timeout_ms) {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::MissingWindow,
                    "window query timed out",
                ));
            }
            tokio::time::sleep(Duration::from_millis(interval_ms)).await;
        }
    }

    /// Enumerate installed and currently running applications through CUA.
    pub async fn list_apps(&self) -> ComputerUseResult<Value> {
        let value = self.call_tool_value("list_apps", json!({})).await?;
        value["structuredContent"]
            .as_object()
            .cloned()
            .map(Value::Object)
            .ok_or_else(|| {
                ComputerUseError::new(
                    ComputerUseErrorCode::BackendUnavailable,
                    "CUA list_apps omitted structuredContent",
                )
            })
    }

    /// Return the CUA tool inventory for adapter capability discovery.
    /// The registry is cached for the lifetime of this shared driver so Host
    /// calls do not re-fetch it just to validate one tool schema.
    pub async fn list_tools(&self) -> ComputerUseResult<Value> {
        Ok(self.cached_tool_inventory().await?.clone())
    }

    /// Probe the embedded CUA runtime without creating a second process.
    ///
    /// Diagnostics are always returned as data so supervisors can distinguish
    /// a live-but-unready Host from a transport failure.
    pub async fn diagnostics(&self) -> Value {
        let (metadata, windows, permissions, health) = tokio::join!(
            self.driver.metadata(),
            self.list_windows(),
            self.call_global_tool("check_permissions", json!({})),
            self.call_global_tool("health_report", json!({})),
        );
        let interactive_desktop = interactive_desktop::diagnostic();
        let driver = match metadata {
            Ok(result) => json!({"success": true, "result": result}),
            Err(error) => json!({"success": false, "message": error.to_string()}),
        };
        let window_inventory = match windows {
            Ok(result) => json!({"success": true, "count": result.len()}),
            Err(error) => json!({
                "success": false,
                "code": error.code,
                "message": error.message,
            }),
        };
        let permissions = diagnostic_tool_check(permissions);
        let health = diagnostic_tool_check(health);
        let ready = driver["success"] == true
            && window_inventory["success"] == true
            && permissions["success"] == true
            && health["success"] == true
            && health["result"]["overall"] == "ok"
            && interactive_desktop["success"] == true
            && interactive_desktop["input_ready"] == true;
        json!({
            "type": "diagnostics",
            "schema_version": 1,
            "backend": "cua-driver-sdk",
            "ready": ready,
            "checks": {
                "driver": driver,
                "window_inventory": window_inventory,
                "permissions": permissions,
                "health": health,
                "interactive_desktop": interactive_desktop,
            },
        })
    }

    /// Call a non-window-bound CUA tool from the local CLI surface.
    /// Window-bound tools must use an exact `ComputerUseSession` instead.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        validate_native_tool_request(name, &arguments)?;
        self.call_unscoped_tool(name, arguments).await
    }

    /// Call a CUA tool that is not bound to an exact native window.
    ///
    /// Host IPC callers must use the grant-gated global route. Dedicated
    /// application, input, browser, recording, and lifecycle routes stay out
    /// of this escape hatch.
    pub async fn call_global_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        validate_native_tool_request(name, &arguments)?;
        if !native_tool_allowed_globally(name) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!("CUA tool {name:?} must use its dedicated or window-bound route"),
            ));
        }
        self.call_unscoped_tool(name, arguments).await
    }

    async fn call_unscoped_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        let schema = self.tool_schema(name).await?;
        if schema["properties"].get("pid").is_some()
            || schema["properties"].get("window_id").is_some()
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                format!("CUA tool {name:?} requires an exact window session"),
            ));
        }
        let result = call_driver_tool(
            &self.driver,
            name,
            arguments.to_string(),
            &format!("call CUA {name}"),
        )
        .await?;
        ensure_tool_ok(&format!("call CUA {name}"), &result)?;
        native_tool_result(result)
    }

    /// Capture the full desktop without widening any window-scoped session.
    pub async fn desktop_snapshot(&self) -> ComputerUseResult<ComputerUseDesktopSnapshot> {
        self.desktop_snapshot_for(None).await
    }

    async fn desktop_snapshot_for(
        &self,
        session: Option<&str>,
    ) -> ComputerUseResult<ComputerUseDesktopSnapshot> {
        interactive_desktop::require_desktop_observation_available()?;
        let mut arguments = json!({});
        if let Some(session) = session {
            arguments["session"] = Value::String(session.to_owned());
        }
        let result = call_driver_tool(
            &self.driver,
            "get_desktop_state",
            arguments.to_string(),
            "capture CUA desktop state",
        )
        .await?;
        ensure_tool_ok("capture CUA desktop state", &result)?;
        let image = result.images.first().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "CUA desktop state returned no screenshot",
            )
        })?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(&image.data_base64)
            .map_err(|error| {
                ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
            })?;
        let state = result
            .structured_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_else(|| json!({}));
        Ok(ComputerUseDesktopSnapshot {
            data,
            state,
            observation_id: format!(
                "desktop-{}",
                OBSERVATION_COUNTER.fetch_add(1, Ordering::Relaxed)
            ),
        })
    }

    pub async fn screen_size(&self) -> ComputerUseResult<Value> {
        self.call_tool_value("get_screen_size", json!({})).await
    }

    pub async fn cursor_position(&self) -> ComputerUseResult<Value> {
        self.call_tool_value("get_cursor_position", json!({})).await
    }

    async fn call_tool_value(&self, name: &str, arguments: Value) -> ComputerUseResult<Value> {
        let result = call_driver_tool(
            &self.driver,
            name,
            arguments.to_string(),
            &format!("call CUA {name}"),
        )
        .await?;
        ensure_tool_ok(&format!("call CUA {name}"), &result)?;
        serde_json::from_str(&result.raw_json).map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA {name} returned invalid JSON: {error}"),
            )
        })
    }

    async fn tool_schema(&self, name: &str) -> ComputerUseResult<Value> {
        tool_schema_from_inventory(self.cached_tool_inventory().await?, name)
    }

    async fn cached_tool_inventory(&self) -> ComputerUseResult<&Value> {
        self.tool_inventory
            .get_or_try_init(|| async {
                let raw = self
                    .driver
                    .list_tools_json()
                    .await
                    .map_err(|error| map_driver_error("list CUA tools", error))?;
                serde_json::from_str(&raw).map_err(|error| {
                    ComputerUseError::new(
                        ComputerUseErrorCode::BackendUnavailable,
                        format!("CUA tool inventory returned invalid JSON: {error}"),
                    )
                })
            })
            .await
    }
}

pub(crate) fn diagnostic_tool_check(result: ComputerUseResult<ComputerUseToolResult>) -> Value {
    match result {
        Ok(result) => json!({
            "success": true,
            "degraded": result.degraded,
            "summary": result.text,
            "result": result.value.get("structuredContent").unwrap_or(&result.value),
        }),
        Err(error) => json!({
            "success": false,
            "code": error.code,
            "message": error.message,
        }),
    }
}

pub(crate) fn tool_schema_from_inventory(
    inventory: &Value,
    name: &str,
) -> ComputerUseResult<Value> {
    inventory["tools"]
        .as_array()
        .and_then(|tools| {
            tools
                .iter()
                .find(|tool| tool["name"].as_str() == Some(name))
        })
        .and_then(|tool| tool.get("inputSchema"))
        .cloned()
        .ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!("CUA tool {name:?} is not present in the live inventory"),
            )
        })
}

async fn enable_session_marker(
    driver: &ComputerUseDriver,
    session_id: &str,
    context: &str,
) -> ComputerUseResult<()> {
    if !driver.upstream_cursor_renderer_enabled() {
        return Ok(());
    }

    let result = call_driver_tool(
        &driver.driver,
        "set_agent_cursor_enabled",
        json!({"session": session_id, "enabled": true}).to_string(),
        context,
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            cleanup_started_session(driver, session_id).await;
            return Err(error);
        }
    };
    if let Err(error) = ensure_tool_ok(context, &result) {
        cleanup_started_session(driver, session_id).await;
        return Err(error);
    }
    let result = call_driver_tool(
        &driver.driver,
        "set_agent_cursor_motion",
        json!({"session": session_id, "glide_duration_ms": CURSOR_GLIDE_MS}).to_string(),
        context,
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            cleanup_started_session(driver, session_id).await;
            return Err(error);
        }
    };
    if let Err(error) = ensure_tool_ok(context, &result) {
        cleanup_started_session(driver, session_id).await;
        return Err(error);
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn uses_windows_foreground_fast_path(action: &ComputerUseAction) -> bool {
    action.delivery_mode.as_deref() == Some("foreground")
        && matches!(
            action.action.as_str(),
            "click"
                | "double_click"
                | "right_click"
                | "toggle"
                | "drag"
                | "keypress"
                | "keyboard_shortcut"
        )
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowsPostInputFocusLoss {
    pub actual_foreground_window: Option<String>,
    pub input_sent: bool,
    pub delivery_confirmed: bool,
    pub retry_safe: bool,
    pub verification_required: bool,
    pub effect: &'static str,
}

/// Classify the one Windows driver error that is raised only after every click
/// input was inserted. It must not be treated as a retryable delivery failure:
/// a competing window may have taken focus while the target was processing the
/// mouse-up, so the only safe next step is a fresh observation.
#[must_use]
#[cfg(any(windows, test))]
pub(crate) fn windows_post_input_focus_loss(message: &str) -> Option<WindowsPostInputFocusLoss> {
    const POST_CLICK_MARKER: &str = "was not foreground after the click";
    const ACTUAL_WINDOW_MARKER: &str = "(actual foreground HWND ";

    if !message.contains("foreground_unavailable:") || !message.contains(POST_CLICK_MARKER) {
        return None;
    }
    let actual_foreground_window = message
        .split_once(ACTUAL_WINDOW_MARKER)
        .and_then(|(_, tail)| tail.trim().strip_suffix(')'))
        .map(str::to_owned);
    Some(WindowsPostInputFocusLoss {
        actual_foreground_window,
        input_sent: true,
        delivery_confirmed: false,
        retry_safe: false,
        verification_required: true,
        effect: "unverifiable",
    })
}

#[cfg(windows)]
async fn perform_windows_foreground_fast_action(
    action: &ComputerUseAction,
    session_id: &str,
    target: &WindowTarget,
    control_banner: Option<&ControlBanner>,
) -> ComputerUseResult<Option<ComputerUseToolResult>> {
    if !uses_windows_foreground_fast_path(action) {
        return Ok(None);
    }

    dcc_cua_platform_windows::activate_window(
        dcc_cua_platform_windows::UiaTarget {
            process_id: target.pid,
            window_handle: target.window_id,
        },
        || windows_platform_input_gate("foreground_fast_action_activation"),
    )
    .map_err(|error| {
        map_windows_window_mutation_error(
            "activate the exact Windows target before pointer input",
            error,
        )
    })?;

    let window_id = target.window_id;
    let [left, top, _, _] = target.bounds;
    let screen_point =
        |point: ComputerUsePoint| (left + point.x.round() as i32, top + point.y.round() as i32);
    let animate_to = |x: i32, y: i32| {
        platform_windows::overlay::send_command(
            session_id.to_owned(),
            cursor_overlay::OverlayCommand::PinAbove(target.window_id),
        );
        tokio::time::timeout(
            Duration::from_millis(CURSOR_GLIDE_MS + 70),
            platform_windows::overlay::animate_cursor_to(
                session_id.to_owned(),
                f64::from(x),
                f64::from(y),
            ),
        )
    };
    let mut post_input_focus_loss = None;
    let mut raw_drag_outcome = None;

    let text = if matches!(action.action.as_str(), "keypress" | "keyboard_shortcut") {
        let key = action.keys.last().cloned().unwrap_or_default();
        let modifiers = if action.action == "keyboard_shortcut" {
            action.keys[..action.keys.len().saturating_sub(1)].to_vec()
        } else {
            action.modifiers.clone()
        };
        let key_target = dcc_cua_platform_windows::UiaTarget {
            process_id: target.pid,
            window_handle: target.window_id,
        };
        {
            let _input_activity = control_banner.map(|banner| {
                banner.begin_activity(banner_activity_for_action_phase(
                    action,
                    ActionBannerPhase::Injecting,
                ))
            });
            tokio::task::spawn_blocking(move || {
                dcc_cua_platform_windows::activate_window(key_target, || {
                    windows_platform_input_gate("foreground_keypress")
                })
                .map_err(|error| {
                    map_windows_window_mutation_error(
                        "validate the exact Windows target before keypress input",
                        error,
                    )
                })?;
                let modifiers: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                platform_windows::input::keyboard::send_key_synthesized(window_id, &key, &modifiers)
                    .map_err(|error| {
                        ComputerUseError::new(
                            ComputerUseErrorCode::InputFailed,
                            format!("send Windows foreground keypress: {error}"),
                        )
                    })
            })
            .await
            .map_err(|error| {
                ComputerUseError::new(
                    ComputerUseErrorCode::InputFailed,
                    format!("join Windows foreground keypress: {error}"),
                )
            })??;
        }
        "Sent scoped Windows keypress.".to_owned()
    } else if action.action == "drag" {
        let first = action.path.first().copied().unwrap_or(ComputerUsePoint {
            x: action.x.unwrap_or_default(),
            y: action.y.unwrap_or_default(),
        });
        let last = action.path.last().copied().unwrap_or(first);
        let (from_x, from_y) = screen_point(first);
        let (to_x, to_y) = screen_point(last);
        let _ = animate_to(from_x, from_y).await;
        platform_windows::overlay::send_command(
            session_id.to_owned(),
            cursor_overlay::OverlayCommand::ClickPulse {
                x: f64::from(from_x),
                y: f64::from(from_y),
            },
        );
        platform_windows::overlay::send_command(
            session_id.to_owned(),
            cursor_overlay::OverlayCommand::MoveTo {
                x: f64::from(to_x),
                y: f64::from(to_y),
                end_heading_radians: std::f64::consts::FRAC_PI_4,
            },
        );
        let duration_ms = action.duration_ms.unwrap_or(500);
        let steps = action.steps.unwrap_or(20);
        let button = action.button.as_deref().unwrap_or("left").to_owned();
        let drag_target = dcc_cua_platform_windows::UiaTarget {
            process_id: target.pid,
            window_handle: target.window_id,
        };
        dcc_cua_platform_windows::activate_window(drag_target, || {
            windows_platform_input_gate("foreground_drag_after_cursor_glide")
        })
        .map_err(|error| {
            map_windows_window_mutation_error(
                "validate the exact Windows target before drag input",
                error,
            )
        })?;
        let backend = match select_windows_foreground_drag_backend(action) {
            Ok(backend) => backend,
            Err(reason) => {
                let backend_id = action.input_backend_id.as_deref().unwrap_or_default();
                return Ok(Some(input_backend_rejection_result(
                    backend_id, &reason, target,
                )));
            }
        };
        if backend == WindowsForegroundDragBackend::SyntheticTouch {
            let result = {
                let _input_activity = control_banner.map(|banner| {
                    banner.begin_activity(banner_activity_for_action_phase(
                        action,
                        ActionBannerPhase::Injecting,
                    ))
                });
                let result_target = target.clone();
                tokio::task::spawn_blocking(move || {
                    let input_gate = dcc_cua_platform_windows::activate_window(drag_target, || {
                        windows_platform_input_gate("synthetic_touch_activation")
                    })
                    .map_err(|error| {
                        map_windows_window_mutation_error(
                            "validate the exact Windows target before synthetic-touch input",
                            error,
                        )
                    });
                    windows_synthetic_touch_attempt(
                        input_gate,
                        || {
                            platform_windows::input::inject::inject_drag_screen(
                                window_id,
                                from_x,
                                from_y,
                                to_x,
                                to_y,
                                steps as usize,
                                "left",
                            )
                            .map_err(|error| error.to_string())
                        },
                        &result_target,
                    )
                })
                .await
                .map_err(|error| {
                    ComputerUseError::new(
                        ComputerUseErrorCode::InputFailed,
                        format!("join Windows synthetic-touch foreground drag: {error}"),
                    )
                })??
            };
            return Ok(Some(result));
        }
        let outcome = {
            let _input_activity = control_banner.map(|banner| {
                banner.begin_activity(banner_activity_for_action_phase(
                    action,
                    ActionBannerPhase::Injecting,
                ))
            });
            tokio::task::spawn_blocking(move || {
                dcc_cua_platform_windows::activate_window(drag_target, || {
                    windows_platform_input_gate("foreground_raw_drag_dispatch")
                })
                .map_err(|error| {
                    map_windows_window_mutation_error(
                        "validate the exact Windows target before raw drag input",
                        error,
                    )
                })?;
                if let Some(message) =
                    platform_windows::input::post_message_blocked_by_uipi(window_id)
                {
                    Err(ComputerUseError::new(
                        ComputerUseErrorCode::InputFailed,
                        message,
                    ))
                } else {
                    let path = WindowsRawDragPath {
                        source: (from_x, from_y),
                        destination: (to_x, to_y),
                        duration_ms,
                        steps: steps as usize,
                    };
                    let outcome = match backend {
                        WindowsForegroundDragBackend::SendInput => {
                            send_windows_separated_raw_drag(drag_target, path, &button)
                        }
                        WindowsForegroundDragBackend::RelativeSendInput => {
                            send_windows_calibrated_relative_drag(drag_target, path, &button)
                        }
                        WindowsForegroundDragBackend::CombinedDownDrag => {
                            Ok(send_windows_combined_down_drag(drag_target, path))
                        }
                        WindowsForegroundDragBackend::SyntheticTouch => {
                            unreachable!("synthetic touch returns before raw drag dispatch")
                        }
                    };
                    outcome.map_err(|error| {
                        ComputerUseError::new(
                            ComputerUseErrorCode::InputFailed,
                            format!("send Windows foreground drag: {error}"),
                        )
                    })
                }
            })
            .await
            .map_err(|error| {
                ComputerUseError::new(
                    ComputerUseErrorCode::InputFailed,
                    format!("join Windows foreground drag: {error}"),
                )
            })??
        };
        let release_succeeded = outcome
            .trace
            .release_succeeded(outcome.release_error.as_ref());
        let text = if outcome.path_sent && release_succeeded == Some(true) {
            format!("Sent scoped Windows drag from ({from_x}, {from_y}) to ({to_x}, {to_y}).")
        } else if outcome.path_sent {
            format!(
                "Completed the scoped Windows drag path to ({to_x}, {to_y}), but LEFTUP cleanup \
                 was not verified; no fallback backend was used."
            )
        } else if outcome.trace.relative_path_attempted() {
            format!(
                "Stopped scoped Windows relative drag before ({to_x}, {to_y}) because the OS \
                 cursor did not satisfy the bounded path calibration; button-up cleanup was \
                 attempted and no fallback backend was used."
            )
        } else if outcome.trace.backend_id() == WINDOWS_COMBINED_DOWN_DRAG_BACKEND_ID {
            let movement = if outcome.trace.path_started() {
                "after partial waypoint movement"
            } else {
                "before a complete waypoint path"
            };
            let failure_phase = outcome
                .trace
                .delivery_failure_phase(outcome.path_sent, release_succeeded)
                .unwrap_or("combined_drag");
            format!(
                "Stopped scoped Windows combined drag {movement} during {failure_phase}; the \
                 typed input trace records inserted events and cleanup, and no fallback backend \
                 was used."
            )
        } else {
            format!(
                "Stopped scoped Windows drag at ({from_x}, {from_y}) after the button-down \
                 delivery probe; no path movement was sent."
            )
        };
        raw_drag_outcome = Some(outcome);
        text
    } else {
        let (x, y) = screen_point(ComputerUsePoint {
            x: action.x.unwrap_or_default(),
            y: action.y.unwrap_or_default(),
        });
        let _ = animate_to(x, y).await;
        platform_windows::overlay::send_command(
            session_id.to_owned(),
            cursor_overlay::OverlayCommand::ClickPulse {
                x: f64::from(x),
                y: f64::from(y),
            },
        );
        let count = usize::from(action.action == "double_click") + 1;
        let button = action
            .button
            .as_deref()
            .unwrap_or(if action.action == "right_click" {
                "right"
            } else {
                "left"
            })
            .to_owned();
        let modifiers = action.modifiers.clone();
        let click_target = dcc_cua_platform_windows::UiaTarget {
            process_id: target.pid,
            window_handle: target.window_id,
        };
        let sent = {
            let _input_activity = control_banner.map(|banner| {
                banner.begin_activity(banner_activity_for_action_phase(
                    action,
                    ActionBannerPhase::Injecting,
                ))
            });
            tokio::task::spawn_blocking(move || {
                dcc_cua_platform_windows::activate_window(click_target, || {
                    windows_platform_input_gate("foreground_click_after_cursor_glide")
                })
                .map_err(|error| {
                    map_windows_window_mutation_error(
                        "validate the exact Windows target before click input",
                        error,
                    )
                })?;
                let modifiers: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                platform_windows::input::mouse::send_click_synthesized_active_mods(
                    window_id, x, y, count, &button, &modifiers,
                )
                .map_err(|error| {
                    ComputerUseError::new(
                        ComputerUseErrorCode::InputFailed,
                        format!("send Windows foreground click: {error}"),
                    )
                })
            })
            .await
            .map_err(|error| {
                ComputerUseError::new(
                    ComputerUseErrorCode::InputFailed,
                    format!("join Windows foreground click: {error}"),
                )
            })?
        };
        match sent {
            Ok(()) => format!("Sent scoped Windows click at ({x}, {y})."),
            Err(error) => {
                if error.code != ComputerUseErrorCode::InputFailed {
                    return Err(error);
                }
                let message = error.message;
                let Some(outcome) = windows_post_input_focus_loss(&message) else {
                    return Err(ComputerUseError::new(error.code, message));
                };
                post_input_focus_loss = Some(outcome);
                format!(
                    "Sent scoped Windows click at ({x}, {y}), but another window took focus \
                     before post-input verification. Do not retry; inspect the fresh observation."
                )
            }
        }
    };

    let _finishing_activity =
        control_banner.map(|banner| banner.begin_activity(BannerActivity::Operating));

    let (delivery, effect, degraded) = if let Some(outcome) = post_input_focus_loss {
        (
            json!({
                "mode": "foreground",
                "confirmed": outcome.delivery_confirmed,
                "actual_foreground_window": outcome.actual_foreground_window,
                "input_sent": outcome.input_sent,
                "retry_safe": outcome.retry_safe,
                "verification_required": outcome.verification_required,
                "failure_phase": "post_input_focus_lost",
            }),
            outcome.effect,
            true,
        )
    } else if let Some(outcome) = raw_drag_outcome.as_ref() {
        (
            windows_raw_drag_delivery(outcome, target),
            "unverifiable",
            !outcome.path_sent || outcome.release_error.is_some(),
        )
    } else {
        (
            json!({
                "mode": "foreground",
                "confirmed": true,
                "input_sent": true,
                "retry_safe": false,
                "verification_required": false,
            }),
            "unverifiable",
            false,
        )
    };

    let success = raw_drag_outcome
        .as_ref()
        .is_none_or(|outcome| outcome.path_sent && outcome.release_error.is_none());
    Ok(Some(ComputerUseToolResult {
        value: json!({
            "success": success,
            "route": "windows_scoped_fast_input",
            "delivery": delivery,
            "effect": effect,
        }),
        text,
        images: Vec::new(),
        degraded,
    }))
}

async fn cleanup_started_session(driver: &ComputerUseDriver, session_id: &str) {
    let _ = call_driver_tool(
        &driver.driver,
        "end_session",
        json!({"session": session_id}).to_string(),
        "cleanup CUA session",
    )
    .await;
}

/// A long-lived, exact-window Computer Use session.
struct ActiveShowcase {
    recorder: ShowcaseRecorder,
    owns_live_observation: bool,
}

struct LiveObservationStartOutcome {
    state: Value,
    disposition: LiveObservationStartDisposition,
}

pub struct ComputerUseSession {
    driver: ComputerUseDriver,
    scope: ComputerUseTargetScope,
    app_name: String,
    agent_name: String,
    session_id: String,
    marker: ComputerUseMarker,
    control_banner: Option<ControlBanner>,
    target: Option<WindowTarget>,
    pub(crate) observation: Option<ComputerUseObservation>,
    live_observation: Option<LiveObservation>,
    post_action_live_sequence_fence: Option<LiveObservationFence>,
    observation_transition_live_sequence_fence: Option<LiveObservationFence>,
    showcase: Option<ActiveShowcase>,
    recording_active: bool,
    recording_expected_video: bool,
    recording_health: Option<RecordingHealth>,
    recording_keepalive: Option<RecordingKeepalive>,
    #[cfg(windows)]
    pub(crate) windows_uia: Option<WindowsUiaFallback>,
    last_upstream_session_refresh: Option<Instant>,
    active: bool,
    escalated: bool,
}

/// A bounded desktop-scope session for screen-absolute discovery and input.
/// Window-scoped sessions remain the preferred path for semantic controls.
pub struct ComputerUseDesktopSession {
    driver: ComputerUseDriver,
    session_id: String,
    marker: ComputerUseMarker,
    active: bool,
    latest_observation_id: Option<String>,
}

impl std::fmt::Debug for ComputerUseDesktopSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ComputerUseDesktopSession")
            .field("session_id", &self.session_id)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl ComputerUseDesktopSession {
    fn new(
        driver: ComputerUseDriver,
        agent_name: String,
        session_id: String,
    ) -> ComputerUseResult<Self> {
        if session_id.trim().is_empty() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "desktop session_id must not be empty",
            ));
        }
        Ok(Self {
            driver,
            session_id,
            marker: ComputerUseMarker {
                visible: false,
                label: localized_control_label(&agent_name, "Desktop"),
                backend: "cua-driver-sdk",
            },
            active: false,
            latest_observation_id: None,
        })
    }

    pub async fn start(&mut self) -> ComputerUseResult<Value> {
        if self.active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "desktop session is already active",
            ));
        }
        interactive_desktop::require_desktop_observation_available()?;
        let result = call_driver_tool(
            &self.driver.driver,
            "start_session",
            json!({
                "session": self.session_id,
                "capture_scope": "desktop",
                "cursor_theme": {"theme_id": MOUSE_CURSOR_THEME, "reduced_motion": "auto"},
                "_public_session_label": self.marker.label,
            })
            .to_string(),
            "start CUA desktop session",
        )
        .await?;
        ensure_tool_ok("start CUA desktop session", &result)?;
        enable_session_marker(&self.driver, &self.session_id, "show CUA desktop marker").await?;
        self.active = true;
        self.marker.visible = true;
        Ok(json!({"success": true, "active": true, "marker": self.marker}))
    }

    pub async fn screenshot(&mut self) -> ComputerUseResult<ComputerUseDesktopSnapshot> {
        if !self.active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "desktop session is not active",
            ));
        }
        let snapshot = tokio::time::timeout(
            INPUT_CALL_TIMEOUT,
            self.driver.desktop_snapshot_for(Some(&self.session_id)),
        )
        .await
        .map_err(|_| {
            ComputerUseError::new(
                ComputerUseErrorCode::InputFailed,
                "CUA desktop snapshot timed out; the desktop session was invalidated",
            )
        })??;
        self.latest_observation_id = Some(snapshot.observation_id.clone());
        Ok(snapshot)
    }

    pub async fn perform_action(
        &mut self,
        action: &ComputerUseAction,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        if !self.active {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "desktop session is not active",
            ));
        }
        validate_action(action)?;
        if action.element_index.is_some()
            || action.element_token.is_some()
            || matches!(action.action.as_str(), "set_text" | "set_value")
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "desktop actions support only screen-coordinate input",
            ));
        }
        if self.latest_observation_id.as_deref() != action.observation_id.as_deref() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "take a fresh desktop snapshot before acting",
            ));
        }
        interactive_desktop::require_input_available()?;
        let arguments = desktop_action_arguments(action, &self.session_id);
        let tool = arguments["_tool"].as_str().unwrap_or_default().to_owned();
        let mut arguments = arguments;
        arguments
            .as_object_mut()
            .expect("desktop action arguments are an object")
            .remove("_tool");
        let result = call_driver_tool(
            &self.driver.driver,
            tool.clone(),
            arguments.to_string(),
            &format!("execute desktop CUA {tool}"),
        )
        .await?;
        ensure_tool_ok(&format!("execute desktop CUA {tool}"), &result)?;
        let mut result = native_tool_result(result)?;
        self.latest_observation_id = None;
        result.value = json!({
            "success": true,
            "action": action,
            "marker": self.marker,
            "cua": result.value,
        });
        Ok(result)
    }

    pub async fn stop(&mut self) -> ComputerUseResult<Value> {
        if !self.active {
            return Ok(json!({"success": true, "active": false}));
        }
        let result = call_driver_tool(
            &self.driver.driver,
            "end_session",
            json!({"session": self.session_id}).to_string(),
            "stop CUA desktop session",
        )
        .await;
        if let Err(error) = &result {
            self.active = false;
            self.marker.visible = false;
            self.latest_observation_id = None;
            return Err(error.clone());
        }
        let result = result?;
        ensure_tool_ok("stop CUA desktop session", &result)?;
        self.active = false;
        self.marker.visible = false;
        self.latest_observation_id = None;
        Ok(json!({"success": true, "active": false, "marker": self.marker}))
    }
}

#[cfg(windows)]
fn desktop_crop_bounds(target: &WindowTarget) -> ComputerUseResult<([i32; 4], u32)> {
    let window_handle = usize::try_from(target.window_id).map_err(|_| {
        ComputerUseError::new(
            ComputerUseErrorCode::InvalidTarget,
            "window handle does not fit the current Windows target",
        )
    })?;
    let dpi = unsafe {
        windows_sys::Win32::UI::HiDpi::GetDpiForWindow(window_handle as *mut core::ffi::c_void)
    };
    Ok((scale_bounds_for_dpi(target.bounds, dpi)?, dpi))
}

#[cfg(not(windows))]
fn desktop_crop_bounds(target: &WindowTarget) -> ComputerUseResult<([i32; 4], u32)> {
    Ok((target.bounds, 96))
}

#[cfg(windows)]
async fn capture_exact_window(window_id: u64) -> ComputerUseResult<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        platform_windows::capture::screenshot_window_bytes(window_id).map_err(|error| {
            ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
        })
    })
    .await
    .map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            format!("exact window capture task failed: {error}"),
        )
    })?
}

#[cfg(not(windows))]
async fn capture_exact_window(_window_id: u64) -> ComputerUseResult<Vec<u8>> {
    Err(ComputerUseError::new(
        ComputerUseErrorCode::BackendUnavailable,
        "exact native window capture is unavailable on this platform",
    ))
}

#[cfg(windows)]
fn mark_foreground_window(windows: &mut [WindowTarget]) {
    let foreground = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }
        as usize as u64;
    for window in windows {
        window.is_foreground = window.window_id == foreground;
    }
}

#[cfg(not(windows))]
fn mark_foreground_window(windows: &mut [WindowTarget]) {
    if windows.iter().any(|window| window.is_foreground) {
        return;
    }
    mark_foreground_by_z_index(windows);
}

#[cfg(any(not(windows), test))]
pub(crate) fn mark_foreground_by_z_index(windows: &mut [WindowTarget]) {
    let Some(highest) = windows.iter().filter_map(|window| window.z_index).max() else {
        return;
    };
    for window in windows {
        window.is_foreground = window.z_index == Some(highest);
    }
}

async fn list_windows_with_driver(
    driver: &Arc<CuaDriver>,
    pid: Option<u32>,
    on_screen_only: bool,
) -> ComputerUseResult<Vec<Value>> {
    if let Some(windows) = crate::window_target::native_inventory(pid, on_screen_only) {
        return Ok(windows);
    }
    let mut arguments = json!({});
    if let Some(pid) = pid {
        arguments["pid"] = json!(pid);
    }
    if on_screen_only {
        arguments["on_screen_only"] = Value::Bool(true);
    }
    let result = call_driver_tool(
        driver,
        "list_windows",
        arguments.to_string(),
        "list CUA windows",
    )
    .await?;
    ensure_tool_ok("list CUA windows", &result)?;
    let value: Value = result.raw_json.parse::<Value>().map_err(|error| {
        ComputerUseError::new(ComputerUseErrorCode::BackendUnavailable, error.to_string())
    })?;
    value["structuredContent"]["windows"]
        .as_array()
        .cloned()
        .ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "CUA list_windows omitted structuredContent.windows",
            )
        })
}
