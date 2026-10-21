//! One queued show request, followed by exact-instance foreground readback.
use crate::{
    ExactWindowNativeState, ExactWindowPixelInstanceEvidence, UiaError, UiaTarget,
    exact_window_state::validate_instance,
};

pub(crate) fn run_queued_activation(
    target: UiaTarget,
    expected: ExactWindowPixelInstanceEvidence,
    mut read: impl FnMut() -> Result<ExactWindowNativeState, UiaError>,
    request: impl FnOnce() -> i32,
    mut wait: impl FnMut(),
) -> Result<bool, UiaError> {
    let validate = |state: &ExactWindowNativeState| {
        validate_instance(target, expected, state)?;
        if state.minimized {
            return Err(UiaError::TargetMinimized(
                "queued activation requires the same visible, non-minimized native instance".into(),
            ));
        }
        if !state.visible {
            return Err(UiaError::InvalidTarget(
                "queued activation requires the same visible native instance".into(),
            ));
        }
        Ok(())
    };
    let before = read()?;
    validate(&before)?;
    if before.foreground {
        return Ok(true);
    }
    // A nonzero return confirms only that Windows queued the request.
    let _ = request();
    for attempt in 0..20 {
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

#[cfg(test)]
mod tests;
