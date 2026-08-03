#[cfg(windows)]
pub(crate) fn prepare_platform_process() {
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };

    // Windows rejects this when an embedding DCC already selected a DPI mode.
    // That is safe: this supplies the missing default for the standalone Host.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

#[cfg(not(windows))]
pub(crate) fn prepare_platform_process() {}
