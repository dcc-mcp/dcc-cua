//! One focus request inside an existing native input-queue attachment.
pub(crate) use crate::WindowsAttachedFocusDiagnostic as AttachedFocusCall;
use crate::{ExactWindowNativeState, ExactWindowPixelInstanceEvidence, UiaError, UiaTarget};

pub(crate) fn run_responsive_attachment<T>(
    mut gate: impl FnMut() -> Result<(), UiaError>,
    probe: impl FnOnce() -> Result<(), UiaError>,
    attach: impl FnOnce() -> Result<T, UiaError>,
) -> Result<T, UiaError> {
    gate()?;
    probe()?;
    crate::windows::input_gated_window_mutation(gate, attach)
}

#[derive(Clone, Copy)]
pub(crate) struct AttachedInputQueues {
    pub caller_thread_id: u32,
    pub target_thread_id: u32,
    pub foreground_thread_id: u32,
    pub attached_target: bool,
    pub attached_foreground: bool,
}

impl AttachedInputQueues {
    fn target_available(self) -> bool {
        self.caller_thread_id != 0
            && self.target_thread_id != 0
            && (self.caller_thread_id == self.target_thread_id
                || self.attached_target
                || (self.target_thread_id == self.foreground_thread_id && self.attached_foreground))
    }
}

/// Always finish the existing attachments before propagating the mutation result.
/// A returned focus HWND is evidence only; activation remains a separate native readback.
pub(crate) fn run_attached_focus(
    target: UiaTarget,
    expected: ExactWindowPixelInstanceEvidence,
    attachment: AttachedInputQueues,
    preflight: impl FnOnce() -> Result<ExactWindowNativeState, UiaError>,
    focus: impl FnOnce() -> Result<AttachedFocusCall, UiaError>,
    detach: impl FnOnce(),
) -> (Option<AttachedFocusCall>, Result<(), UiaError>) {
    let mut diagnostic = None;
    let result = (|| {
        if !attachment.target_available() {
            return Ok(());
        }
        let before = preflight()?;
        crate::exact_window_state::validate_instance(target, expected, &before)?;
        if before.minimized || !before.visible {
            return Err(UiaError::InvalidTarget(
                "attached focus requires the same visible, non-minimized window".into(),
            ));
        }
        if before.foreground {
            return Ok(());
        }
        diagnostic = Some(focus()?);
        Ok(())
    })();
    detach();
    (diagnostic, result)
}

#[cfg(test)]
mod tests;
