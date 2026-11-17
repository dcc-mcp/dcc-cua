use rstest::rstest;

#[cfg(windows)]
#[rstest]
#[case(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_UNAWARE)]
#[case(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_SYSTEM_AWARE)]
#[case(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)]
#[ignore = "reads native thread DPI context; never activates a window or sends input"]
fn foreground_click_uses_physical_coordinates_before_activation(
    #[case] caller: windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT,
) {
    use windows_sys::Win32::UI::HiDpi::{
        AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        GetThreadDpiAwarenessContext, SetThreadDpiAwarenessContext,
    };

    struct Restore(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
    let previous = unsafe { SetThreadDpiAwarenessContext(caller) };
    assert!(!previous.is_null());
    let _restore = Restore(previous);
    let mut physical_scope = false;
    let result = crate::input::send_click_exact_foreground_mods(
        crate::UiaTarget {
            process_id: 0,
            window_handle: 0,
        },
        (1180, 742),
        1,
        "left",
        &[],
        || {
            physical_scope = unsafe {
                AreDpiAwarenessContextsEqual(
                    GetThreadDpiAwarenessContext(),
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                ) != 0
            };
            Err(crate::UiaError::InvalidTarget(
                "stop before any native input".into(),
            ))
        },
    );
    assert!(
        matches!(result, Err(crate::input::WindowsForegroundClickError::Activation { attempts: 1, source: crate::UiaError::InvalidTarget(ref message), .. }) if message == "stop before any native input")
    );
    assert!(
        physical_scope,
        "physical screenshot coordinates must not use the caller's DPI virtualization"
    );
    assert!(unsafe { AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), caller) } != 0);
}
