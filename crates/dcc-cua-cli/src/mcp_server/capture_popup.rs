//! Closed owner ceiling and explicit secondary window selectors.
use super::*;

pub(super) fn owner_schema() -> Value {
    json!({"type":"boolean","default":false,
        "description":"Independent owner ceiling for explicit native capture popup observations supporting keyboard input to the unchanged exact foreground root. Requires pixels_only and a separately granted raw keyboard action. Snapshot selectors are at most eight unique exact HWNDs; every root redaction must be observed. Grants no pointer exception or automatic retargeting."})
}

pub(super) fn validate_start(input: &PrepareTaskInput) -> Result<(), String> {
    if input.allow_native_capture_popup_keyboard
        && (input.surface != TaskSurface::Window
            || input.observation_mode != TaskObservationMode::PixelsOnly
            || !["snapshot", "get_window_state", "execute_action"]
                .iter()
                .all(|required| {
                    input
                        .allowed_methods
                        .iter()
                        .any(|method| method == required)
                })
            || !input.allowed_actions.iter().any(|scope| {
                scope.is_pixels_input()
                    && matches!(
                        scope.action.as_str(),
                        "keypress" | "keyboard_shortcut" | "type" | "type_chars"
                    )
            }))
    {
        return Err("native capture popup keyboard requires an explicit pixels_only exact-window task, snapshot/state/action methods and an independently granted raw keyboard action".into());
    }
    Ok(())
}

pub(super) fn add_ceiling(allowed_actions: &mut Vec<TrustedTaskActionScope>, enabled: bool) {
    if enabled {
        allowed_actions.push(TrustedTaskActionScope {
            action: "native_capture_popup_keyboard".into(),
            input_kind: "window_state".into(),
            secret_input: false,
            authorization_category: "window_state".into(),
            browser_origin: None,
        });
    }
}

pub(super) fn validate_params(method: &str, params: &Value) -> Result<(), String> {
    if let Some(selected) = params.get("native_capture_popup_window_handles") {
        let items = selected
            .as_array()
            .ok_or_else(|| "native_capture_popup_window_handles must be an array".to_owned())?;
        let handles = items
            .iter()
            .map(Value::as_u64)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| "native capture popup HWNDs must be positive integers".to_owned())?;
        let unique = handles
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        if method != "snapshot"
            || handles.len() > 8
            || unique.len() != handles.len()
            || unique.contains(&0)
        {
            return Err(
                "snapshot accepts at most eight unique positive native capture popup HWNDs".into(),
            );
        }
    }
    Ok(())
}

pub(super) fn validate_call_ceiling(proposal: &TaskProposal, params: &Value) -> Result<(), String> {
    if params
        .get("native_capture_popup_window_handles")
        .is_some_and(|value| value.as_array().is_some_and(|items| !items.is_empty()))
        && !proposal
            .registration
            .allowed_actions
            .iter()
            .any(TrustedTaskActionScope::is_native_capture_popup_keyboard)
    {
        return Err(
            "native capture popup observation is outside the independent owner ceiling".into(),
        );
    }
    Ok(())
}
