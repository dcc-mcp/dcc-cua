//! Explicit restore followed by activation, bound to one native window instance.
use crate::{ExactWindowNativeState, UiaError, UiaTarget, exact_window_state::validate_instance};
use thiserror::Error;

const READBACK_ATTEMPTS: usize = 40;

#[derive(Debug, Error)]
pub enum ExactWindowRestoreError {
    #[error("exact restore was refused before dispatch: {0}")]
    BeforeDispatch(UiaError),
    #[error("exact restore requires fresh evidence after dispatch: {0}")]
    AfterDispatch(UiaError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExactWindowRestoreOutcome {
    pub state: ExactWindowNativeState,
    /// Set at the native call boundary, even when Windows refuses that request.
    pub window_mutation_attempted: bool,
}

pub(crate) fn classify_restore_result<T>(
    result: Result<T, UiaError>,
    attempted: bool,
) -> Result<T, ExactWindowRestoreError> {
    result.map_err(|error| {
        if attempted {
            ExactWindowRestoreError::AfterDispatch(error)
        } else {
            ExactWindowRestoreError::BeforeDispatch(error)
        }
    })
}

/// Restore uses one gated foreground request rather than the general activation
/// fallback chain. Every subsequent poll can stop without another mutation.
pub(crate) fn run_restored_foreground_request(
    target: UiaTarget,
    expected: crate::ExactWindowPixelInstanceEvidence,
    mut available: impl FnMut() -> Result<(), UiaError>,
    mut read: impl FnMut() -> Result<ExactWindowNativeState, UiaError>,
    request: impl FnOnce() -> Result<(), UiaError>,
    mut wait: impl FnMut(),
) -> Result<bool, UiaError> {
    let validate = |state: &ExactWindowNativeState| {
        validate_instance(target, expected, state)?;
        if state.minimized || state.maximized || !state.visible {
            return Err(UiaError::InvalidTarget(
                "explicit activation requires the same restored, visible native instance".into(),
            ));
        }
        Ok(())
    };
    available()?;
    let before = read()?;
    validate(&before)?;
    if before.foreground {
        return Ok(true);
    }
    available()?;
    request()?;
    for attempt in 0..20 {
        available()?;
        let state = read()?;
        validate(&state)?;
        if state.foreground {
            return Ok(true);
        }
        if attempt < 19 {
            wait();
        }
    }
    Ok(false)
}

#[derive(Clone, Copy)]
enum RestoreStage {
    Minimized,
    Maximized,
}

impl RestoreStage {
    fn required(self, state: &ExactWindowNativeState) -> bool {
        match self {
            Self::Minimized => state.minimized,
            Self::Maximized => state.maximized,
        }
    }

    fn completed(self, state: &ExactWindowNativeState) -> bool {
        !state.minimized && (matches!(self, Self::Minimized) || !state.maximized)
    }
}

pub(crate) fn run_exact_restore_activate_sequence(
    target: UiaTarget,
    mut restore_available: impl FnMut() -> Result<(), UiaError>,
    mut activate_available: impl FnMut() -> Result<(), UiaError>,
    mut read: impl FnMut() -> Result<ExactWindowNativeState, UiaError>,
    mut restore: impl FnMut() -> Result<(), UiaError>,
    activate: impl FnOnce() -> Result<(), UiaError>,
    mut wait: impl FnMut(),
) -> Result<ExactWindowNativeState, UiaError> {
    restore_available()?;
    let before = read()?;
    validate_instance(target, before.instance, &before)?;
    let expected = before.instance;
    let mut current = before;
    // A minimized window can restore to its previous maximized state. Normalize
    // each observed state in its own bounded stage, with at most one request each.
    for stage in [RestoreStage::Minimized, RestoreStage::Maximized] {
        if !stage.required(&current) {
            continue;
        }
        restore_available()?;
        current = read()?;
        validate_instance(target, expected, &current)?;
        if !stage.required(&current) {
            continue;
        }
        restore_available()?;
        restore()?;
        let mut restored = false;
        for attempt in 0..READBACK_ATTEMPTS {
            restore_available()?;
            current = read()?;
            validate_instance(target, expected, &current)?;
            if stage.completed(&current) {
                restored = true;
                break;
            }
            if attempt + 1 < READBACK_ATTEMPTS {
                wait();
            }
        }
        if !restored {
            return Err(UiaError::OperationFailed(
                "the exact target remained minimized or maximized after the explicit restore request".into(),
            ));
        }
    }
    activate_available()?;
    let restored = read()?;
    validate_instance(target, expected, &restored)?;
    if restored.minimized || restored.maximized || !restored.visible {
        return Err(UiaError::InvalidTarget(
            "the exact target must be restored and visible before activation".into(),
        ));
    }
    activate_available()?;
    activate()?;
    let final_state = read()?;
    validate_instance(target, expected, &final_state)?;
    activate_available()?;
    if final_state.minimized
        || final_state.maximized
        || !final_state.visible
        || !final_state.foreground
    {
        return Err(UiaError::OperationFailed(
            "the exact target did not finish restored, visible, and foreground".into(),
        ));
    }
    Ok(final_state)
}

#[cfg(test)]
mod tests;
