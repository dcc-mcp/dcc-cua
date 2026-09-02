#[cfg(windows)]
use super::*;

#[cfg(windows)]
pub(crate) fn supported(action: &ComputerUseAction) -> bool {
    action.delivery_mode.as_deref() == Some("foreground")
        && matches!(action.action.as_str(), "keypress" | "keyboard_shortcut")
        && action.element_index.is_none()
        && action.element_token.is_none()
        && action.x.is_none()
        && action.y.is_none()
}

#[cfg(windows)]
pub(crate) async fn dispatch(
    action: &ComputerUseAction,
    window_id: u64,
) -> ComputerUseResult<Option<ComputerUseToolResult>> {
    if action.input_backend_id.as_deref() != Some(WINDOWS_POST_MESSAGE_KEY_BACKEND_ID)
        || !supported(action)
    {
        return Ok(None);
    }
    let keys = if action.action == "keyboard_shortcut" {
        keyboard_shortcut_keys(action)
    } else {
        action.keys.clone()
    };
    let key = keys.last().cloned().unwrap_or_default();
    let modifiers = if action.action == "keyboard_shortcut" {
        keys[..keys.len().saturating_sub(1)].to_vec()
    } else {
        action.modifiers.clone()
    };
    let key_for_send = key.clone();
    let result = tokio::task::spawn_blocking(move || {
        let modifier_refs: Vec<&str> = modifiers.iter().map(String::as_str).collect();
        dcc_cua_platform_windows::post_key(window_id, &key_for_send, &modifier_refs)
    })
    .await
    .map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::InputFailed,
            format!("join Windows PostMessage key: {error}"),
        )
    })?;
    result.map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::InputFailed,
            format!("send Windows PostMessage key: {error}"),
        )
    })?;
    Ok(Some(super::windows_input::windows_post_message_result(
        "windows_scoped_post_message_key",
        WINDOWS_POST_MESSAGE_KEY_BACKEND_ID,
        format!(
            "Posted scoped Windows key {key:?} via {WINDOWS_POST_MESSAGE_KEY_BACKEND_ID}; verify the target effect before continuing."
        ),
    )))
}
