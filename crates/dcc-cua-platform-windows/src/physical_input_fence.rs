//! Final physical mutation boundaries for an immutable, actually captured HWND.
//! No activation, upstream input delegation, semantic provider or fallback.
use serde::Serialize;
use std::time::{Duration, Instant};
use thiserror::Error;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput,
    VK_RETURN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SetCursorPos,
};

use crate::{
    ExactWindowNativeState, ExactWindowPixelEvidence, ExactWindowPixelInstanceEvidence, UiaError,
    UiaTarget, WindowsPointerButton, exact_window_native_state, exact_window_pixel_evidence,
    input::{click_input_batch, keyboard_input, modifier_virtual_key, virtual_key},
    visible_capture::{ThreadDpiAwarenessGuard, physical_rectangle_within_desktop},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowsPhysicalInputFence {
    pub target: UiaTarget,
    pub native_instance: ExactWindowPixelInstanceEvidence,
    pub native_window_bounds: [i32; 4],
    pub native_visible_bounds: [i32; 4],
    pub window_dpi: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsPhysicalInputFailureReason {
    InvalidRequest,
    TargetUnavailable,
    InstanceChanged,
    BoundsChanged,
    DpiChanged,
    TargetHidden,
    TargetMinimized,
    TargetNotForeground,
    TargetOccluded,
    PointOutsideTarget,
    TargetOutsideDesktop,
    DesktopUnavailable,
    InjectionIncomplete,
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsGuardedInputOperation {
    Click,
    Keypress,
    Text,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WindowsGuardedInputOutcome {
    pub operation: WindowsGuardedInputOperation,
    pub mutation_attempted: bool,
    pub cursor_move_attempted: bool,
    pub cursor_move_succeeded: bool,
    pub requested_events: u32,
    pub inserted_events: u32,
    pub cleanup_requested_events: u32,
    pub cleanup_inserted_events: u32,
    pub delivery_completed: bool,
    pub post_dispatch_validated: bool,
    pub fresh_observation_required: bool,
}

impl WindowsGuardedInputOutcome {
    fn new(operation: WindowsGuardedInputOperation) -> Self {
        Self {
            operation,
            mutation_attempted: false,
            cursor_move_attempted: false,
            cursor_move_succeeded: false,
            requested_events: 0,
            inserted_events: 0,
            cleanup_requested_events: 0,
            cleanup_inserted_events: 0,
            delivery_completed: false,
            post_dispatch_validated: false,
            fresh_observation_required: true,
        }
    }
}

#[derive(Clone, Debug, Error, Serialize)]
#[error("guarded physical input failed: {reason:?}")]
pub struct WindowsGuardedInputError {
    pub reason: WindowsPhysicalInputFailureReason,
    pub outcome: WindowsGuardedInputOutcome,
}

impl WindowsGuardedInputError {
    #[must_use]
    pub const fn is_pre_dispatch(&self) -> bool {
        !self.outcome.mutation_attempted
    }
}

type Reason = WindowsPhysicalInputFailureReason;

fn point_inside(point: (i32, i32), bounds: [i32; 4]) -> bool {
    bounds[2] > 0
        && bounds[3] > 0
        && point.0 >= bounds[0]
        && point.1 >= bounds[1]
        && i64::from(point.0) < i64::from(bounds[0]) + i64::from(bounds[2])
        && i64::from(point.1) < i64::from(bounds[1]) + i64::from(bounds[3])
}

// The same validator is used by the production final boundary and injected tests.
fn validate_physical_metadata(
    fence: WindowsPhysicalInputFence,
    evidence: ExactWindowPixelEvidence,
    state: ExactWindowNativeState,
    desktop: [i32; 4],
    point: Option<(i32, i32)>,
) -> Result<(), Reason> {
    if fence.target.process_id == 0
        || fence.target.window_handle == 0
        || fence.native_instance.process_creation_time_100ns == 0
        || fence.native_instance.window_thread_id == 0
        || fence.window_dpi == 0
        || fence.native_window_bounds[2] <= 4
        || fence.native_window_bounds[3] <= 4
        || fence.native_visible_bounds[2] <= 4
        || fence.native_visible_bounds[3] <= 4
    {
        return Err(Reason::InvalidRequest);
    }
    if evidence.process_id != fence.target.process_id
        || evidence.window_handle != fence.target.window_handle
        || state.process_id != fence.target.process_id
        || state.window_handle != fence.target.window_handle
        || evidence.instance != fence.native_instance
        || state.instance != fence.native_instance
    {
        return Err(Reason::InstanceChanged);
    }
    if evidence.bounds != fence.native_window_bounds
        || state.bounds != Some(fence.native_window_bounds)
        || evidence.visible_bounds != fence.native_visible_bounds
        || state.visible_bounds != Some(fence.native_visible_bounds)
    {
        return Err(Reason::BoundsChanged);
    }
    if evidence.dpi != fence.window_dpi || state.dpi != fence.window_dpi {
        return Err(Reason::DpiChanged);
    }
    if evidence.minimized || state.minimized {
        return Err(Reason::TargetMinimized);
    }
    if !evidence.visible || !state.visible {
        return Err(Reason::TargetHidden);
    }
    if !state.foreground {
        return Err(Reason::TargetNotForeground);
    }
    if !evidence.unobscured {
        return Err(Reason::TargetOccluded);
    }
    if !physical_rectangle_within_desktop(evidence.visible_bounds, desktop) {
        return Err(Reason::TargetOutsideDesktop);
    }
    if point.is_some_and(|point| {
        !point_inside(point, fence.native_window_bounds)
            || !point_inside(point, evidence.visible_bounds)
            || !point_inside(point, desktop)
    }) {
        return Err(Reason::PointOutsideTarget);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Key(u16, bool),
    Unicode(u16, bool),
    Mouse(WindowsPointerButton, bool),
    Move(i32, i32),
}

impl Event {
    fn release(self) -> Option<Self> {
        match self {
            Self::Key(key, false) => Some(Self::Key(key, true)),
            Self::Unicode(unit, false) => Some(Self::Unicode(unit, true)),
            Self::Mouse(button, false) => Some(Self::Mouse(button, true)),
            _ => None,
        }
    }
    fn is_release(self) -> bool {
        matches!(
            self,
            Self::Key(_, true) | Self::Unicode(_, true) | Self::Mouse(_, true)
        )
    }
}

#[derive(Clone, Copy)]
struct MutationReceipt {
    requested: u32,
    inserted: u32,
    cursor_attempted: bool,
    cursor_succeeded: bool,
}

// No wait, conversion or other callback may be placed between check and dispatch.
fn final_physical_boundary<T>(
    cleanup_only: bool,
    releases_only: bool,
    check: impl FnOnce() -> Result<(), Reason>,
    dispatch: impl FnOnce() -> T,
) -> Result<T, Reason> {
    if cleanup_only {
        if !releases_only {
            return Err(Reason::InvalidRequest);
        }
    } else {
        check()?;
    }
    Ok(dispatch())
}

trait InputBackend {
    fn check(
        &mut self,
        fence: WindowsPhysicalInputFence,
        point: Option<(i32, i32)>,
    ) -> Result<(), Reason>;
    fn mutate(
        &mut self,
        fence: WindowsPhysicalInputFence,
        point: Option<(i32, i32)>,
        events: &[Event],
        cursor: Option<(i32, i32)>,
        cleanup_only: bool,
    ) -> Result<MutationReceipt, Reason>;
    fn interrupted(&mut self) -> bool;
    fn elapsed_ms(&self) -> u64;
    fn pause(&mut self, milliseconds: u64);
}

struct NativeBackend<G, I> {
    gate: G,
    interrupted: I,
    started: Instant,
}

fn check_physical_fence(
    fence: WindowsPhysicalInputFence,
    point: Option<(i32, i32)>,
    mut gate: impl FnMut() -> Result<(), UiaError>,
    mut interrupted: impl FnMut() -> bool,
    read_proof: impl FnOnce() -> Result<ExactWindowPixelEvidence, Reason>,
    read_desktop: impl FnOnce() -> [i32; 4],
    read_final_state: impl FnOnce() -> Result<ExactWindowNativeState, Reason>,
) -> Result<(), Reason> {
    if interrupted() {
        return Err(Reason::Interrupted);
    }
    gate().map_err(|_| Reason::DesktopUnavailable)?;
    let evidence = read_proof()?;
    gate().map_err(|_| Reason::DesktopUnavailable)?;
    let desktop = read_desktop();
    // This full native identity/physical state read follows proof enumeration.
    let state = read_final_state()?;
    if interrupted() {
        return Err(Reason::Interrupted);
    }
    validate_physical_metadata(fence, evidence, state, desktop, point)
}

impl<G: FnMut() -> Result<(), UiaError>, I: FnMut() -> bool> InputBackend for NativeBackend<G, I> {
    fn check(
        &mut self,
        fence: WindowsPhysicalInputFence,
        point: Option<(i32, i32)>,
    ) -> Result<(), Reason> {
        // Shared strict production occlusion collector; never exclude a sibling,
        // allow a failed DWM query, follow a popup or capture pixels here.
        check_physical_fence(
            fence,
            point,
            &mut self.gate,
            &mut self.interrupted,
            || {
                exact_window_pixel_evidence(fence.target.process_id, fence.target.window_handle)
                    .map_err(|_| Reason::TargetUnavailable)
            },
            || unsafe {
                [
                    GetSystemMetrics(SM_XVIRTUALSCREEN),
                    GetSystemMetrics(SM_YVIRTUALSCREEN),
                    GetSystemMetrics(SM_CXVIRTUALSCREEN),
                    GetSystemMetrics(SM_CYVIRTUALSCREEN),
                ]
            },
            || {
                exact_window_native_state(fence.target.process_id, fence.target.window_handle)
                    .map_err(|_| Reason::TargetUnavailable)
            },
        )
    }

    fn mutate(
        &mut self,
        fence: WindowsPhysicalInputFence,
        point: Option<(i32, i32)>,
        events: &[Event],
        cursor: Option<(i32, i32)>,
        cleanup_only: bool,
    ) -> Result<MutationReceipt, Reason> {
        let inputs = events.iter().copied().map(native_event).collect::<Vec<_>>();
        let releases_only =
            cursor.is_none() && !events.is_empty() && events.iter().all(|event| event.is_release());
        // Prepare INPUT packets before the last fence. Dispatch contains only
        // the one OS mutation, so the checker cannot precede conversion/waits.
        final_physical_boundary(
            cleanup_only,
            releases_only,
            || self.check(fence, point),
            || match cursor {
                Some((x, y)) => MutationReceipt {
                    requested: 0,
                    inserted: 0,
                    cursor_attempted: true,
                    cursor_succeeded: unsafe { SetCursorPos(x, y) } != 0,
                },
                None => {
                    let requested = inputs.len() as u32;
                    let inserted = unsafe {
                        SendInput(
                            requested,
                            inputs.as_ptr(),
                            std::mem::size_of::<INPUT>() as i32,
                        )
                    };
                    MutationReceipt {
                        requested,
                        inserted,
                        cursor_attempted: false,
                        cursor_succeeded: false,
                    }
                }
            },
        )
    }

    fn interrupted(&mut self) -> bool {
        (self.interrupted)()
    }
    fn elapsed_ms(&self) -> u64 {
        self.started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
    fn pause(&mut self, milliseconds: u64) {
        std::thread::sleep(Duration::from_millis(milliseconds));
    }
}

fn native_event(event: Event) -> INPUT {
    match event {
        Event::Key(key, up) => keyboard_input(key, up),
        Event::Unicode(unit, up) => INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: unit,
                    dwFlags: KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { 0 },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        Event::Move(x, y) => click_input_batch((x, y), "left")[0],
        Event::Mouse(button, up) => click_input_batch(
            (0, 0),
            match button {
                WindowsPointerButton::Left => "left",
                WindowsPointerButton::Right => "right",
                WindowsPointerButton::Middle => "middle",
            },
        )[if up { 2 } else { 1 }],
    }
}

struct Transaction<'a, B> {
    backend: &'a mut B,
    fence: WindowsPhysicalInputFence,
    point: Option<(i32, i32)>,
    outcome: WindowsGuardedInputOutcome,
    pending_releases: Vec<Event>,
}

impl<B: InputBackend> Transaction<'_, B> {
    fn mutate(&mut self, events: &[Event], cursor: Option<(i32, i32)>) -> Result<(), Reason> {
        let receipt = self
            .backend
            .mutate(self.fence, self.point, events, cursor, false)?;
        self.outcome.mutation_attempted = true;
        self.outcome.cursor_move_attempted |= receipt.cursor_attempted;
        self.outcome.cursor_move_succeeded |= receipt.cursor_succeeded;
        self.outcome.requested_events += receipt.requested;
        self.outcome.inserted_events += receipt.inserted;
        for event in events.iter().copied().take(receipt.inserted as usize) {
            if let Some(release) = event.release() {
                self.pending_releases.push(release);
            } else if event.is_release()
                && let Some(index) = self
                    .pending_releases
                    .iter()
                    .rposition(|pending| *pending == event)
            {
                self.pending_releases.remove(index);
            }
        }
        if receipt.inserted != receipt.requested
            || (receipt.cursor_attempted && !receipt.cursor_succeeded)
        {
            return Err(Reason::InjectionIncomplete);
        }
        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), Reason> {
        if self.pending_releases.is_empty() {
            return Ok(());
        }
        let events = self
            .pending_releases
            .iter()
            .rev()
            .copied()
            .collect::<Vec<_>>();
        let receipt = self
            .backend
            .mutate(self.fence, self.point, &events, None, true)?;
        self.outcome.cleanup_requested_events += receipt.requested;
        self.outcome.cleanup_inserted_events += receipt.inserted;
        // One bounded release-only batch, no retry and no new down/move/text.
        self.pending_releases.clear();
        if receipt.requested != receipt.inserted {
            return Err(Reason::InjectionIncomplete);
        }
        Ok(())
    }

    fn finish(
        mut self,
        result: Result<(), Reason>,
    ) -> Result<WindowsGuardedInputOutcome, WindowsGuardedInputError> {
        let cleanup = self.cleanup();
        let result = result
            .and(cleanup)
            .and_then(|()| self.backend.check(self.fence, self.point));
        match result {
            Ok(()) => {
                self.outcome.delivery_completed = true;
                self.outcome.post_dispatch_validated = true;
                Ok(self.outcome)
            }
            Err(reason) => Err(WindowsGuardedInputError {
                reason,
                outcome: self.outcome,
            }),
        }
    }
}

fn checked_keys(keys: &[String], modifiers_only: bool) -> Result<Vec<u16>, Reason> {
    if keys.len() > 16 || (!modifiers_only && keys.is_empty()) {
        return Err(Reason::InvalidRequest);
    }
    let mut result = Vec::new();
    for key in keys {
        let value = modifier_virtual_key(key)
            .or_else(|| (!modifiers_only).then(|| virtual_key(key).ok()).flatten())
            .ok_or(Reason::InvalidRequest)?;
        if result.contains(&value) {
            return Err(Reason::InvalidRequest);
        }
        result.push(value);
    }
    Ok(result)
}

fn run_click<B: InputBackend>(
    backend: &mut B,
    fence: WindowsPhysicalInputFence,
    point: (i32, i32),
    count: usize,
    button: WindowsPointerButton,
    modifiers: &[String],
) -> Result<WindowsGuardedInputOutcome, WindowsGuardedInputError> {
    let mut transaction = Transaction {
        backend,
        fence,
        point: Some(point),
        outcome: WindowsGuardedInputOutcome::new(WindowsGuardedInputOperation::Click),
        pending_releases: Vec::new(),
    };
    let result = (|| {
        if !(1..=2).contains(&count) {
            return Err(Reason::InvalidRequest);
        }
        let modifiers = checked_keys(modifiers, true)?;
        transaction.mutate(&[], Some(point))?;
        if !modifiers.is_empty() {
            transaction.mutate(
                &modifiers
                    .iter()
                    .map(|key| Event::Key(*key, false))
                    .collect::<Vec<_>>(),
                None,
            )?;
        }
        let batch = [
            Event::Move(point.0, point.1),
            Event::Mouse(button, false),
            Event::Mouse(button, true),
        ];
        for index in 0..count {
            transaction.mutate(&batch, None)?;
            if index + 1 < count {
                transaction.backend.pause(80);
            }
        }
        Ok(())
    })();
    transaction.finish(result)
}

fn run_keypress<B: InputBackend>(
    backend: &mut B,
    fence: WindowsPhysicalInputFence,
    keys: &[String],
    duration: Option<u64>,
) -> Result<WindowsGuardedInputOutcome, WindowsGuardedInputError> {
    let mut transaction = Transaction {
        backend,
        fence,
        point: None,
        outcome: WindowsGuardedInputOutcome::new(WindowsGuardedInputOperation::Keypress),
        pending_releases: Vec::new(),
    };
    let result = (|| {
        let keys = checked_keys(keys, false)?;
        if duration.is_some_and(|duration| duration == 0 || duration > 60_000) {
            return Err(Reason::InvalidRequest);
        }
        let mut events = keys
            .iter()
            .map(|key| Event::Key(*key, false))
            .collect::<Vec<_>>();
        if let Some(duration) = duration {
            transaction.mutate(&events, None)?;
            let deadline = transaction.backend.elapsed_ms().saturating_add(duration);
            while transaction.backend.elapsed_ms() < deadline {
                if transaction.backend.interrupted() {
                    return Err(Reason::Interrupted);
                }
                transaction.backend.check(fence, None)?;
                transaction.backend.pause(
                    deadline
                        .saturating_sub(transaction.backend.elapsed_ms())
                        .min(20),
                );
            }
        } else {
            events.extend(keys.iter().rev().map(|key| Event::Key(*key, true)));
            transaction.mutate(&events, None)?;
        }
        Ok(())
    })();
    transaction.finish(result)
}

fn text_events(text: &str) -> Result<Vec<Event>, Reason> {
    if text.is_empty() || text.encode_utf16().count() > 4096 {
        return Err(Reason::InvalidRequest);
    }
    let mut result = Vec::new();
    let mut previous_cr = false;
    for character in text.chars() {
        if character == '\n' && previous_cr {
            previous_cr = false;
            continue;
        }
        previous_cr = character == '\r';
        if matches!(character, '\r' | '\n') {
            result.extend([Event::Key(VK_RETURN, false), Event::Key(VK_RETURN, true)]);
        } else {
            for unit in character.encode_utf16(&mut [0; 2]).iter().copied() {
                result.extend([Event::Unicode(unit, false), Event::Unicode(unit, true)]);
            }
        }
    }
    Ok(result)
}

fn run_text<B: InputBackend>(
    backend: &mut B,
    fence: WindowsPhysicalInputFence,
    text: &str,
) -> Result<WindowsGuardedInputOutcome, WindowsGuardedInputError> {
    let mut transaction = Transaction {
        backend,
        fence,
        point: None,
        outcome: WindowsGuardedInputOutcome::new(WindowsGuardedInputOperation::Text),
        pending_releases: Vec::new(),
    };
    let result = text_events(text).and_then(|events| transaction.mutate(&events, None));
    transaction.finish(result)
}

fn with_native_backend<G: FnMut() -> Result<(), UiaError>, I: FnMut() -> bool>(
    operation: WindowsGuardedInputOperation,
    gate: G,
    interrupted: I,
    run: impl FnOnce(
        &mut NativeBackend<G, I>,
    ) -> Result<WindowsGuardedInputOutcome, WindowsGuardedInputError>,
) -> Result<WindowsGuardedInputOutcome, WindowsGuardedInputError> {
    let _dpi = ThreadDpiAwarenessGuard::per_monitor_v2().map_err(|_| WindowsGuardedInputError {
        reason: Reason::TargetUnavailable,
        outcome: WindowsGuardedInputOutcome::new(operation),
    })?;
    let mut backend = NativeBackend {
        gate,
        interrupted,
        started: Instant::now(),
    };
    run(&mut backend)
}

/// No activation or retry. Caller must already have exact foreground. An
/// accepted delivery still requires fresh application-state verification.
pub fn send_guarded_click(
    fence: WindowsPhysicalInputFence,
    point: (i32, i32),
    count: usize,
    button: WindowsPointerButton,
    modifiers: &[String],
    input_available: impl FnMut() -> Result<(), UiaError>,
    interrupted: impl FnMut() -> bool,
) -> Result<WindowsGuardedInputOutcome, WindowsGuardedInputError> {
    with_native_backend(
        WindowsGuardedInputOperation::Click,
        input_available,
        interrupted,
        |backend| run_click(backend, fence, point, count, button, modifiers),
    )
}

pub fn send_guarded_keypress(
    fence: WindowsPhysicalInputFence,
    keys: &[String],
    hold_duration_ms: Option<u64>,
    input_available: impl FnMut() -> Result<(), UiaError>,
    interrupted: impl FnMut() -> bool,
) -> Result<WindowsGuardedInputOutcome, WindowsGuardedInputError> {
    with_native_backend(
        WindowsGuardedInputOperation::Keypress,
        input_available,
        interrupted,
        |backend| run_keypress(backend, fence, keys, hold_duration_ms),
    )
}

pub fn send_guarded_text(
    fence: WindowsPhysicalInputFence,
    text: &str,
    input_available: impl FnMut() -> Result<(), UiaError>,
    interrupted: impl FnMut() -> bool,
) -> Result<WindowsGuardedInputOutcome, WindowsGuardedInputError> {
    with_native_backend(
        WindowsGuardedInputOperation::Text,
        input_available,
        interrupted,
        |backend| run_text(backend, fence, text),
    )
}

#[cfg(test)]
mod tests;
