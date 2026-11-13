use super::*;
use crate::interactive_desktop::windows_execution_context;
use rstest::rstest;

#[rstest]
#[case("IsolatedDesktop", Ok(false))]
#[case("IsolatedDesktop", Ok(true))]
#[case("Default", Ok(false))]
#[case("Default", Err("UOI_IO: os error 5"))]
fn discovery_context_refuses_unverified_or_mismatched_desktops(
    #[case] thread: &str,
    #[case] receives: Result<bool, &str>,
) {
    for surface in [Ok(()), Err("cursor: os error 5")] {
        let mut report = windows_diagnostic_base(Ok(0), Ok(Some("Default")), surface, false);
        crate::interactive_desktop::apply_execution_context(
            &mut report,
            windows_execution_context(
                Ok(Some("WinSta0")),
                Ok(Some(thread)),
                receives,
                Ok(Some("Default")),
            ),
        );
        assert_eq!(report["success"], false);
        assert_eq!(report["observation_ready"], false);
        assert_eq!(report["input_ready"], false);
        assert_eq!(report["retry_safe"], false);
        assert_eq!(report["input_surface_error"], json!(surface.err()));
        assert_eq!(report["input_message"], report["message"]);
        assert!(require_desktop_observation_from(&report).is_err());
        assert!(require_exact_window_observation_from(&report).is_err());
        assert!(require_window_activation_from(&report).is_err());
        assert!(crate::interactive_desktop::require_input_available_from(&report).is_err());
    }
}

#[rstest]
fn verified_context_keeps_cursor_failure_separate_from_observation() {
    let mut report =
        windows_diagnostic_base(Ok(0), Ok(Some("Default")), Err("cursor: os error 5"), true);
    crate::interactive_desktop::apply_execution_context(
        &mut report,
        windows_execution_context(
            Ok(Some("WinSta0")),
            Ok(Some("default")),
            Ok(true),
            Ok(Some("Default")),
        ),
    );
    assert_eq!(report["observation_ready"], true);
    assert_eq!(report["input_ready"], false);
    assert!(require_exact_window_observation_from(&report).is_ok());
}

#[rstest]
fn discovery_context_preserves_unknown_ownership_and_does_not_mask_session_failure() {
    for (thread, station, receives) in [
        (Err("thread: os error 5"), Ok(Some("WinSta0")), Ok(true)),
        (Ok(None), Ok(Some("WinSta0")), Ok(true)),
        (Ok(Some("Default")), Err("station: os error 5"), Ok(true)),
    ] {
        let mut report = windows_diagnostic_base(Ok(0), Ok(Some("Default")), Ok(()), true);
        crate::interactive_desktop::apply_execution_context(
            &mut report,
            windows_execution_context(station, thread, receives, Ok(Some("Default"))),
        );
        assert_eq!(report["code"], "interactive_desktop_context_unverified");
        assert_eq!(report["local_inventory_ready"], false);
        assert_eq!(report["input_ready"], false);
    }
    let mut report = windows_diagnostic_base(Ok(4), Ok(Some("Default")), Ok(()), true);
    crate::interactive_desktop::apply_execution_context(
        &mut report,
        windows_execution_context(
            Ok(Some("WinSta0")),
            Ok(Some("Default")),
            Ok(true),
            Ok(Some("Default")),
        ),
    );
    assert_eq!(report["code"], "interactive_session_not_active");
    assert_eq!(report["local_inventory_ready"], false);
}

#[cfg(windows)]
#[rstest]
fn cursor_info_fallback_obeys_the_native_structure_size_contract() {
    use crate::interactive_desktop::windows_cursor_info_input_surface;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CURSORINFO, GetCursorInfo};

    let mut missing_size = CURSORINFO::default();
    // SAFETY: the writable buffer is a complete CURSORINFO. Intentionally
    // leaving cbSize at zero exercises the native API's failure contract;
    // neither this negative control nor the production probe injects input.
    let invalid_result = unsafe { GetCursorInfo(&raw mut missing_size) };
    let invalid_error = std::io::Error::last_os_error();
    assert_eq!(
        invalid_result, 0,
        "Win32 accepted CURSORINFO without cbSize: {invalid_error}"
    );

    windows_cursor_info_input_surface().unwrap_or_else(|error| {
        panic!(
            "the production cursor-info fallback failed with the ABI-required size: {error}; \
             this native metadata test requires an accessible Windows input surface"
        )
    });
}

#[rstest]
fn execution_context_distinguishes_the_current_thread_from_the_input_desktop() {
    let context = windows_execution_context(
        Ok(Some("WinSta0")),
        Ok(Some("IsolatedDesktop")),
        Ok(false),
        Ok(Some("Default")),
    );

    assert_eq!(context["window_station"]["name"], "WinSta0");
    assert_eq!(context["thread_desktop"]["name"], "IsolatedDesktop");
    assert_eq!(context["thread_desktop"]["receives_input"], false);
    assert_eq!(context["input_desktop"]["name"], "Default");
}

#[rstest]
fn execution_context_keeps_denied_probes_unknown_and_preserves_the_error() {
    let denied = "access denied (os error 5)";
    let context = windows_execution_context(Err(denied), Err(denied), Err(denied), Ok(None));

    assert!(context["window_station"]["name"].is_null());
    assert_eq!(context["window_station"]["error"], denied);
    assert!(context["thread_desktop"]["name"].is_null());
    assert_eq!(context["thread_desktop"]["error"], denied);
    assert!(context["thread_desktop"]["receives_input"].is_null());
    assert_eq!(context["thread_desktop"]["receives_input_error"], denied);
    assert!(context["input_desktop"]["name"].is_null());
    assert!(context["input_desktop"]["error"].is_null());
}

#[rstest]
fn execution_context_does_not_infer_input_ownership_from_equal_desktop_names() {
    for receives_input in [Ok(true), Ok(false), Err("UOI_IO unavailable")] {
        let context = windows_execution_context(
            Ok(Some("WinSta0")),
            Ok(Some("Default")),
            receives_input,
            Ok(Some("Default")),
        );
        assert_eq!(
            context["thread_desktop"]["receives_input"],
            json!(receives_input.ok())
        );
        assert_eq!(
            context["thread_desktop"]["receives_input_error"],
            json!(receives_input.err())
        );
    }
}

#[rstest]
fn denied_input_desktop_probe_accepts_only_a_verified_default_thread_desktop() {
    let ready = windows_diagnostic_with_thread_fallback(
        Ok(0),
        Err("OpenInputDesktop: access denied"),
        Ok(Some("Default")),
        Ok(()),
        true,
    );
    let no_foreground = windows_diagnostic_with_thread_fallback(
        Ok(0),
        Err("OpenInputDesktop: access denied"),
        Ok(Some("Default")),
        Ok(()),
        false,
    );
    let secure = windows_diagnostic_with_thread_fallback(
        Ok(0),
        Err("OpenInputDesktop: access denied"),
        Ok(Some("Winlogon")),
        Ok(()),
        true,
    );

    assert_eq!(ready["success"], true);
    assert_eq!(ready["input_ready"], true);
    assert_eq!(ready["input_desktop_source"], "current_thread_fallback");
    assert_eq!(no_foreground["success"], false);
    assert_eq!(secure["success"], false);
}

#[rstest]
fn exact_window_activation_uses_the_observation_gate_not_the_raw_input_gate() {
    let unreadable_default_desktop =
        windows_diagnostic_base(Ok(0), Err("OpenInputDesktop: access denied"), Ok(()), true);
    let secure_desktop = windows_diagnostic_base(Ok(0), Ok(Some("Winlogon")), Ok(()), false);

    assert!(require_window_activation_from(&unreadable_default_desktop).is_ok());
    assert!(require_window_activation_from(&secure_desktop).is_err());
}
