#[cfg(windows)]
use super::*;

#[cfg(windows)]
pub(super) fn send_windows_key_holds(
    window_id: u64,
    keys: &[String],
    duration_ms: u64,
) -> ComputerUseResult<()> {
    let started_interrupt_generation = dcc_cua_interrupt::interrupt_generation();
    dcc_cua_platform_windows::send_held_keys_exact_foreground(window_id, keys, duration_ms, || {
        dcc_cua_interrupt::interrupt_generation_changed(
            started_interrupt_generation,
            dcc_cua_interrupt::interrupt_generation(),
        )
    })
    .map_err(|error| {
        let code = match error {
            dcc_cua_platform_windows::WindowsHeldKeyError::InvalidKey(_) => {
                ComputerUseErrorCode::InvalidAction
            }
            dcc_cua_platform_windows::WindowsHeldKeyError::Interrupted => {
                ComputerUseErrorCode::UserInterrupted
            }
            dcc_cua_platform_windows::WindowsHeldKeyError::TargetNotForeground { .. }
            | dcc_cua_platform_windows::WindowsHeldKeyError::Injection(_) => {
                ComputerUseErrorCode::InputFailed
            }
        };
        ComputerUseError::new(code, format!("send Windows held key: {error}"))
    })
}
