use super::super::input_target_policy::reject_ambiguous_embedded_browser_navigation;
use super::*;
use rstest::rstest;

fn target(app_name: &str) -> WindowTarget {
    WindowTarget {
        pid: 42,
        window_id: 84,
        title: "Test window".into(),
        app_name: app_name.into(),
        bounds: [0, 0, 800, 600],
        is_on_screen: true,
        is_minimized: false,
        z_index: Some(0),
        is_foreground: true,
    }
}

#[rstest]
fn codex_app_shell_rejects_unscoped_url_typing() {
    let action = ComputerUseAction {
        action: "type".into(),
        text: Some("https://example.test/document".into()),
        delivery_mode: Some("foreground".into()),
        ..ComputerUseAction::default()
    };
    let error = reject_ambiguous_embedded_browser_navigation(&action, &target("ChatGPT.exe"))
        .expect_err("unscoped URL input into the Codex shell must fail closed");
    assert_eq!(error.code, ComputerUseErrorCode::InvalidAction);
    assert!(error.message.contains("semantic browser address element"));
}

#[rstest]
fn explicit_or_non_codex_text_targets_remain_available() {
    let explicit = ComputerUseAction {
        action: "type".into(),
        element_index: Some(7),
        text: Some("https://example.test/document".into()),
        ..ComputerUseAction::default()
    };
    assert!(
        reject_ambiguous_embedded_browser_navigation(&explicit, &target("ChatGPT.exe")).is_ok()
    );

    let unscoped_browser = ComputerUseAction {
        action: "type".into(),
        text: Some("https://example.test/document".into()),
        ..ComputerUseAction::default()
    };
    assert!(
        reject_ambiguous_embedded_browser_navigation(&unscoped_browser, &target("chrome.exe"))
            .is_ok()
    );
}
