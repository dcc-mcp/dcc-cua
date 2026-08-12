use super::*;
use rstest::rstest;

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
