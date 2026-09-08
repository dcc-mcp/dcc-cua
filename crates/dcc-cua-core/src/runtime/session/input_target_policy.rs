use crate::{
    ComputerUseAction, ComputerUseError, ComputerUseErrorCode, ComputerUseResult,
    window_target::WindowTarget,
};

pub(super) fn reject_ambiguous_embedded_browser_navigation(
    action: &ComputerUseAction,
    target: &WindowTarget,
) -> ComputerUseResult<()> {
    let is_unscoped_text_input = matches!(action.action.as_str(), "type" | "type_chars")
        && action.element_index.is_none()
        && action.element_token.is_none()
        && action.x.is_none()
        && action.y.is_none();
    let is_web_url = action.text.as_deref().is_some_and(|text| {
        let text = text.trim();
        text.starts_with("https://") || text.starts_with("http://")
    });
    let app_name = target.app_name.trim();
    let is_codex_app_shell = app_name.eq_ignore_ascii_case("ChatGPT.exe")
        || app_name.eq_ignore_ascii_case("ChatGPT")
        || app_name.eq_ignore_ascii_case("Codex.exe")
        || app_name.eq_ignore_ascii_case("Codex");

    if is_unscoped_text_input && is_web_url && is_codex_app_shell {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "URL input into the Codex app shell requires an explicit semantic browser address element or a browser session",
        ));
    }
    Ok(())
}
