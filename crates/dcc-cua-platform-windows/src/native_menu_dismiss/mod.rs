//! Cancellation authority for one observed, active standard popup menu.
//!
//! This is a window-state operation, not background keyboard delivery. The
//! owner comes exclusively from the target GUI thread's hwndMenuOwner; neither
//! GW_OWNER nor a guessed main window can substitute for that evidence.
use serde::Serialize;
use thiserror::Error;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GUI_INMENUMODE, GUI_INMOVESIZE, GUI_POPUPMENUMODE, GUI_SYSTEMMENUMODE,
};

use crate::{ExactWindowNativeState, UiaError, UiaTarget};

mod native;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativeMenuProof {
    pub popup: ExactWindowNativeState,
    pub owner: ExactWindowNativeState,
    pub flags: u32,
    pub active_window_handle: u64,
    pub capture_window_handle: u64,
}

#[derive(Debug, Serialize)]
pub struct NativeMenuDismissOutcome {
    pub proof: NativeMenuProof,
    pub message_attempted: bool,
    pub popup_hidden_or_gone: bool,
    pub menu_inactive: bool,
    pub owner_alive: bool,
    pub input_sent: bool,
}

#[derive(Debug, Error)]
pub enum NativeMenuDismissError {
    #[error("native menu dismissal refused before dispatch: {0}")]
    BeforeDispatch(UiaError),
    #[error("native menu dismissal is unconfirmed after one message attempt: {0}")]
    AfterDispatch(UiaError),
}

/// Admission authority expires before sending. Read-only confirmation retains
/// revocation and desktop fences without renewing or reusing that authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeMenuGatePhase {
    PreDispatch,
    PostDispatch,
}

pub(super) fn refused(reason: &str) -> UiaError {
    UiaError::InvalidTarget(format!("native_menu_inactive_or_unproven: {reason}"))
}

pub(super) fn validate_proof(
    target: UiaTarget,
    proof: &NativeMenuProof,
    standard_popup: bool,
    unique_popups: &[u64],
    move_size_window: u64,
) -> Result<(), UiaError> {
    let popup = proof.popup;
    let owner = proof.owner;
    if target.process_id == 0
        || target.window_handle == 0
        || popup.process_id != target.process_id
        || popup.window_handle != target.window_handle
        || !standard_popup
        || !popup.visible
        || popup.minimized
        || popup.bounds.is_none_or(|b| b[2] <= 0 || b[3] <= 0)
        || popup.dpi == 0
        || popup.instance.process_creation_time_100ns == 0
        || popup.instance.window_thread_id == 0
        || popup.instance.window_class_hash == 0
        || owner.process_id != popup.process_id
        || owner.window_handle == 0
        || owner.window_handle == popup.window_handle
        || owner.instance.process_creation_time_100ns != popup.instance.process_creation_time_100ns
        || owner.instance.window_thread_id != popup.instance.window_thread_id
        || owner.instance.window_class_hash == 0
        || ![0, owner.window_handle].contains(&popup.instance.owner_window_handle)
        || proof.flags & (GUI_INMENUMODE | GUI_POPUPMENUMODE) != GUI_INMENUMODE | GUI_POPUPMENUMODE
        || proof.flags & (GUI_SYSTEMMENUMODE | GUI_INMOVESIZE) != 0
        || move_size_window != 0
        || (proof.active_window_handle != 0 && proof.active_window_handle != owner.window_handle)
        || ![0, popup.window_handle, owner.window_handle].contains(&proof.capture_window_handle)
        || unique_popups != [popup.window_handle]
    {
        return Err(refused(
            "active popup, exact owner or uniqueness proof failed",
        ));
    }
    Ok(())
}

/// Read-only authority evidence. A visible but inactive popup is an error, not
/// an already-dismissed menu. The caller must bind this proof to a short-lived,
/// purpose-specific, one-use session token before accepting a mutation.
pub fn read_exact_native_menu(target: UiaTarget) -> Result<NativeMenuProof, UiaError> {
    native::read(target)
}

pub(super) fn popup_hidden_or_gone(
    expected: &ExactWindowNativeState,
    mut exists: impl FnMut() -> bool,
    read: impl FnOnce() -> Result<ExactWindowNativeState, UiaError>,
) -> Result<bool, UiaError> {
    if !exists() {
        return Ok(true);
    }
    let actual = match read() {
        Ok(actual) => actual,
        // Successful cancellation may destroy the popup during the exact
        // state read. A surviving HWND must retain the original read error.
        Err(error) => return if exists() { Err(error) } else { Ok(true) },
    };
    if actual.instance != expected.instance {
        return Err(refused("target popup HWND was reused after the message"));
    }
    Ok(!actual.visible)
}

pub(super) fn run_sequence(
    expected: &NativeMenuProof,
    mut gate: impl FnMut(NativeMenuGatePhase) -> Result<(), UiaError>,
    read: impl FnOnce() -> Result<NativeMenuProof, UiaError>,
    dispatch: impl FnOnce(
        &mut dyn FnMut() -> Result<(), UiaError>,
    ) -> Result<(), NativeMenuDismissError>,
    after: impl FnOnce(&mut dyn FnMut() -> Result<(), UiaError>) -> Result<(), UiaError>,
) -> Result<NativeMenuDismissOutcome, NativeMenuDismissError> {
    use NativeMenuDismissError::{AfterDispatch, BeforeDispatch};
    gate(NativeMenuGatePhase::PreDispatch).map_err(BeforeDispatch)?;
    let current = read().map_err(BeforeDispatch)?;
    if current != *expected {
        return Err(BeforeDispatch(refused("native menu authority changed")));
    }
    gate(NativeMenuGatePhase::PreDispatch).map_err(BeforeDispatch)?;
    // dispatch is FnOnce: failures and timeouts cannot cause a resend.
    dispatch(&mut || gate(NativeMenuGatePhase::PreDispatch))?;
    after(&mut || gate(NativeMenuGatePhase::PostDispatch)).map_err(AfterDispatch)?;
    Ok(NativeMenuDismissOutcome {
        proof: current,
        message_attempted: true,
        popup_hidden_or_gone: true,
        menu_inactive: true,
        owner_alive: true,
        input_sent: false,
    })
}

fn run_on_sender(
    job: impl FnOnce() -> Result<NativeMenuDismissOutcome, NativeMenuDismissError> + Send,
) -> Result<NativeMenuDismissOutcome, NativeMenuDismissError> {
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .name("dcc-cua-menu-cancel".into())
            .spawn_scoped(scope, job)
            .map_err(|_| {
                NativeMenuDismissError::BeforeDispatch(refused(
                    "isolated menu sender could not start",
                ))
            })?;
        worker.join().unwrap_or_else(|_| {
            Err(NativeMenuDismissError::AfterDispatch(
                UiaError::OperationFailed(
                    "native menu sender failed; possible single message attempt is unconfirmed"
                        .into(),
                ),
            ))
        })
    })
}

/// One bounded, standard WM_CANCELMODE to the independently proven menu owner.
/// Never activates, attaches input queues, injects input, invokes a menu item,
/// closes an application, follows another popup, or retries the message.
pub fn dismiss_exact_native_menu(
    expected: &NativeMenuProof,
    gate: impl FnMut(NativeMenuGatePhase) -> Result<(), UiaError> + Send,
) -> Result<NativeMenuDismissOutcome, NativeMenuDismissError> {
    if native::is_owner_thread(expected) {
        return Err(NativeMenuDismissError::BeforeDispatch(refused(
            "cancellation cannot send from the owner's GUI thread",
        )));
    }
    let target = UiaTarget {
        process_id: expected.popup.process_id,
        window_handle: expected.popup.window_handle,
    };
    // Proof, revocation/desktop checks and the single send all run on this
    // fresh thread. There is no caller-permit channel whose queue could outlive
    // the identity or stop check, and no inherited input-queue attachment.
    run_on_sender(|| {
        run_sequence(
            expected,
            gate,
            || native::read(target),
            |_| native::send_once(expected).map_err(NativeMenuDismissError::AfterDispatch),
            |gate| native::confirm(expected, gate),
        )
    })
}
