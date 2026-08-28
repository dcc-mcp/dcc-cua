use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use dcc_cua_interrupt::{interrupt_generation, interrupt_generation_changed};
use windows::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, TranslateMessage, WM_QUIT,
};

use super::{BannerRuntime, FRAME_INTERVAL, IndicatorError};

/// Keep the render interval without suspending the window-owning thread's
/// message service. Queue traffic never resets this deadline. Native message
/// handlers must themselves return; this is not a hard real-time guarantee.
pub(super) fn wait_for_frame(
    stop: &AtomicBool,
    interrupted: &AtomicBool,
    runtime: &BannerRuntime,
) -> Result<(), IndicatorError> {
    let deadline = Instant::now() + FRAME_INTERVAL;
    loop {
        if stop_requested(stop, interrupted, runtime)? || Instant::now() >= deadline {
            return Ok(());
        }
        let mut message = MSG::default();
        // PeekMessage also dispatches synchronous sent messages when its return
        // value is false. Only one queued message is dispatched per deadline /
        // cancellation check, so a posted-message flood cannot starve either.
        if unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            if message.message == WM_QUIT {
                return Err(IndicatorError::Backend(
                    "indicator message queue quit".into(),
                ));
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            continue;
        }
        if stop_requested(stop, interrupted, runtime)? {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        // Round up only to the Win32 millisecond resolution. Never poll with a
        // zero timeout, and never restart the full interval after a message.
        let milliseconds = remaining.as_nanos().div_ceil(1_000_000) as u32;
        match unsafe {
            MsgWaitForMultipleObjectsEx(None, milliseconds, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        } {
            WAIT_OBJECT_0 | WAIT_TIMEOUT => {}
            WAIT_FAILED => {
                return Err(IndicatorError::Backend(format!(
                    "wait for indicator messages: {}",
                    windows::core::Error::from_win32()
                )));
            }
            result => {
                return Err(IndicatorError::Backend(format!(
                    "unexpected indicator message wait result: {}",
                    result.0
                )));
            }
        }
    }
}

fn stop_requested(
    stop: &AtomicBool,
    interrupted: &AtomicBool,
    runtime: &BannerRuntime,
) -> Result<bool, IndicatorError> {
    if stop.load(Ordering::Acquire) {
        return Ok(true);
    }
    if !runtime.hub_active.load(Ordering::Acquire) {
        return Err(IndicatorError::Backend("Escape hub stopped".into()));
    }
    if interrupt_generation_changed(runtime.generation, interrupt_generation()) {
        interrupted.store(true, Ordering::Release);
        stop.store(true, Ordering::Release);
        return Ok(true);
    }
    Ok(false)
}
