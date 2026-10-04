use super::*;
use crate::interactive_desktop::windows_execution_context;
use rstest::rstest;

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
