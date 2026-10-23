//! Dedicated main-root keyboard boundary for explicitly observed capture popups.
//! The ordinary exact-root and all pointer boundaries remain unchanged.
use super::*;
use crate::native_capture_popup::{
    NativeCapturePopupProof, native_capture_ended_with_root_focus,
    read_native_capture_popup_proof_diagnostic,
};

fn validate_root_fence(
    fence: WindowsPhysicalInputFence,
    state: ExactWindowNativeState,
) -> Result<(), Reason> {
    if state.process_id != fence.target.process_id
        || state.window_handle != fence.target.window_handle
        || state.instance != fence.native_instance
    {
        return Err(Reason::InstanceChanged);
    }
    if state.bounds != Some(fence.native_window_bounds)
        || state.visible_bounds != Some(fence.native_visible_bounds)
    {
        return Err(Reason::BoundsChanged);
    }
    if state.dpi != fence.window_dpi {
        return Err(Reason::DpiChanged);
    }
    if !state.visible {
        return Err(Reason::TargetHidden);
    }
    if state.minimized {
        return Err(Reason::TargetMinimized);
    }
    if !state.foreground {
        return Err(Reason::TargetNotForeground);
    }
    Ok(())
}

// Same ordering as the ordinary final fence: slow proof, desktop recheck, then
// a last full native root read. No wait or conversion follows it before SendInput.
fn check_capture_popup_fence(
    fence: WindowsPhysicalInputFence,
    mut gate: impl FnMut() -> Result<(), UiaError>,
    mut interrupted: impl FnMut() -> bool,
    read_complete_proof: impl FnOnce() -> Result<(), Reason>,
    read_final_state: impl FnOnce() -> Result<ExactWindowNativeState, Reason>,
) -> Result<(), Reason> {
    if interrupted() {
        return Err(Reason::Interrupted);
    }
    gate().map_err(|_| Reason::DesktopUnavailable)?;
    read_complete_proof()?;
    gate().map_err(|_| Reason::DesktopUnavailable)?;
    let state = read_final_state()?;
    if interrupted() {
        return Err(Reason::Interrupted);
    }
    validate_root_fence(fence, state)
}

struct CapturePopupBackend<'a, G, I> {
    proof: &'a NativeCapturePopupProof,
    gate: G,
    interrupted: I,
    started: Instant,
    closed_postcheck_allowed: bool,
    rejection: Option<crate::NativeCapturePopupInputDiagnostic>,
}

fn keyboard_release_completed(
    events: &[Event],
    receipt: MutationReceipt,
    cleanup_only: bool,
) -> bool {
    if events.is_empty() || receipt.requested != receipt.inserted {
        return false;
    }
    if cleanup_only {
        return events.iter().all(|event| event.is_release());
    }
    let mut pending = Vec::new();
    for event in events {
        if let Some(release) = event.release() {
            pending.push(release);
        } else if let Some(index) = pending.iter().rposition(|release| release == event) {
            pending.remove(index);
        } else {
            return false;
        }
    }
    pending.is_empty()
}

fn validate_current_proof(
    expected: &NativeCapturePopupProof,
    current: &NativeCapturePopupProof,
    completed_keyboard: bool,
) -> Result<(), Reason> {
    if current == expected
        || (completed_keyboard && expected.matches_completed_keyboard_postcheck(current))
    {
        Ok(())
    } else {
        Err(Reason::InstanceChanged)
    }
}

impl<G: FnMut() -> Result<(), UiaError>, I: FnMut() -> bool> CapturePopupBackend<'_, G, I> {
    fn check_mode(
        &mut self,
        fence: WindowsPhysicalInputFence,
        allow_closed: bool,
    ) -> Result<(), Reason> {
        let expected = self.proof;
        validate_root_fence(fence, expected.root)?;
        let mut rejection = None;
        let result = check_capture_popup_fence(
            fence,
            &mut self.gate,
            &mut self.interrupted,
            || {
                let selected = expected
                    .overlays
                    .iter()
                    .map(|state| state.window_handle)
                    .collect::<Vec<_>>();
                match read_native_capture_popup_proof_diagnostic(
                    fence.target.process_id,
                    fence.target.window_handle,
                    &selected,
                ) {
                    // Only the final read-only check after balanced release can
                    // accept resized geometry from the same fully proven episode.
                    Ok(proof) => validate_current_proof(expected, &proof, allow_closed),
                    Err(error) if allow_closed => {
                        rejection = Some(error.diagnostic(expected.root));
                        // Enter may normally close search. Only the fully released
                        // final postcheck may accept that transition; no next input
                        // can use it. The strict original root proof must pass.
                        if !native_capture_ended_with_root_focus(
                            fence.target.process_id,
                            fence.target.window_handle,
                        )
                        .map_err(|_| Reason::TargetUnavailable)?
                        {
                            return Err(Reason::TargetOccluded);
                        }
                        let evidence = exact_window_pixel_evidence(
                            fence.target.process_id,
                            fence.target.window_handle,
                        )
                        .map_err(|_| Reason::TargetUnavailable)?;
                        if !evidence.unobscured {
                            return Err(Reason::TargetOccluded);
                        }
                        let state = exact_window_native_state(
                            fence.target.process_id,
                            fence.target.window_handle,
                        )
                        .map_err(|_| Reason::TargetUnavailable)?;
                        let desktop = unsafe {
                            [
                                GetSystemMetrics(SM_XVIRTUALSCREEN),
                                GetSystemMetrics(SM_YVIRTUALSCREEN),
                                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                                GetSystemMetrics(SM_CYVIRTUALSCREEN),
                            ]
                        };
                        validate_physical_metadata(fence, evidence, state, desktop, None, false)
                    }
                    Err(error) => {
                        rejection = Some(error.diagnostic(expected.root));
                        Err(Reason::TargetOccluded)
                    }
                }
            },
            || {
                exact_window_native_state(fence.target.process_id, fence.target.window_handle)
                    .map_err(|_| Reason::TargetUnavailable)
            },
        );
        self.rejection = if result == Err(Reason::TargetOccluded) {
            rejection
        } else {
            None
        };
        result
    }
}

impl<G: FnMut() -> Result<(), UiaError>, I: FnMut() -> bool> InputBackend
    for CapturePopupBackend<'_, G, I>
{
    fn capture_popup_rejection(&self) -> Option<crate::NativeCapturePopupInputDiagnostic> {
        self.rejection.clone()
    }

    fn check(
        &mut self,
        fence: WindowsPhysicalInputFence,
        point: Option<(i32, i32)>,
    ) -> Result<(), Reason> {
        if point.is_some() {
            return Err(Reason::InvalidRequest);
        }
        self.check_mode(fence, self.closed_postcheck_allowed)
    }

    fn mutate(
        &mut self,
        fence: WindowsPhysicalInputFence,
        point: Option<(i32, i32)>,
        events: &[Event],
        cursor: Option<(i32, i32)>,
        cleanup_only: bool,
    ) -> Result<MutationReceipt, Reason> {
        if point.is_some()
            || cursor.is_some()
            || events
                .iter()
                .any(|event| matches!(event, Event::Mouse(..) | Event::Move(..)))
        {
            return Err(Reason::InvalidRequest);
        }
        let inputs = events.iter().copied().map(native_event).collect::<Vec<_>>();
        let releases_only = !events.is_empty() && events.iter().all(|event| event.is_release());
        let receipt = final_physical_boundary(
            cleanup_only,
            releases_only,
            || self.check_mode(fence, false),
            || {
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
            },
        )?;
        self.closed_postcheck_allowed = keyboard_release_completed(events, receipt, cleanup_only);
        Ok(receipt)
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

fn with_capture_popup_backend<G: FnMut() -> Result<(), UiaError>, I: FnMut() -> bool>(
    operation: WindowsGuardedInputOperation,
    proof: &NativeCapturePopupProof,
    gate: G,
    interrupted: I,
    run: impl FnOnce(
        &mut CapturePopupBackend<'_, G, I>,
    ) -> Result<WindowsGuardedInputOutcome, WindowsGuardedInputError>,
) -> Result<WindowsGuardedInputOutcome, WindowsGuardedInputError> {
    let _dpi = ThreadDpiAwarenessGuard::per_monitor_v2().map_err(|_| WindowsGuardedInputError {
        reason: Reason::TargetUnavailable,
        outcome: WindowsGuardedInputOutcome::new(operation),
        native_input_fence: None,
        visibility_failure: None,
        native_capture_popup: None,
    })?;
    run(&mut CapturePopupBackend {
        proof,
        gate,
        interrupted,
        started: Instant::now(),
        closed_postcheck_allowed: false,
        rejection: None,
    })
}

/// Caller must hold the one-use, fully captured composite observation. This
/// backend never activates/follows a popup or grants pointer input.
pub fn send_guarded_capture_popup_keypress(
    fence: WindowsPhysicalInputFence,
    proof: &NativeCapturePopupProof,
    keys: &[String],
    hold_duration_ms: Option<u64>,
    input_available: impl FnMut() -> Result<(), UiaError>,
    interrupted: impl FnMut() -> bool,
) -> Result<WindowsGuardedInputOutcome, WindowsGuardedInputError> {
    with_capture_popup_backend(
        WindowsGuardedInputOperation::Keypress,
        proof,
        input_available,
        interrupted,
        |backend| run_keypress(backend, fence, keys, hold_duration_ms),
    )
}

pub fn send_guarded_capture_popup_text(
    fence: WindowsPhysicalInputFence,
    proof: &NativeCapturePopupProof,
    text: &str,
    input_available: impl FnMut() -> Result<(), UiaError>,
    interrupted: impl FnMut() -> bool,
) -> Result<WindowsGuardedInputOutcome, WindowsGuardedInputError> {
    with_capture_popup_backend(
        WindowsGuardedInputOperation::Text,
        proof,
        input_available,
        interrupted,
        |backend| run_text(backend, fence, text),
    )
}

#[cfg(test)]
mod tests;
