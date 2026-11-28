use super::*;

pub(super) fn validate_pixel_interrupt(
    started: u64,
    current: u64,
    latched: bool,
) -> ComputerUseResult<()> {
    if latched || dcc_cua_interrupt::interrupt_generation_changed(started, current) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::UserInterrupted,
            "pixel input was interrupted before native dispatch",
        )
        .with_details(ComputerUseErrorDetails {
            phase: Some(ComputerUseErrorPhase::PreDispatch),
            action_attempted: Some(false),
            input_sent: Some(ComputerUseInputState::NotSent),
            completion: Some(ComputerUseCompletionState::Known),
            effect_unknown: Some(false),
            blind_retry: Some(false),
            fresh_observation_required: Some(true),
            ..Default::default()
        }));
    }
    Ok(())
}

pub(super) fn validate_pixel_action(action: &ComputerUseAction) -> ComputerUseResult<()> {
    let supported = matches!(
        action.action.as_str(),
        "click"
            | "double_click"
            | "right_click"
            | "toggle"
            | "drag"
            | "key_sequence"
            | "keypress"
            | "keyboard_shortcut"
            | "type"
            | "type_chars"
    ) || cfg!(target_os = "linux") && action.action == "move";
    if !supported
        || action.element_index.is_some()
        || action.element_token.is_some()
        || action.input_backend_id.is_some()
        || action
            .delivery_mode
            .as_deref()
            .is_some_and(|mode| mode != "foreground")
        || (action.action != "drag" && !action.path.is_empty())
        || action.scroll_x.is_some()
        || action.scroll_y.is_some()
        || action.scroll_by.is_some()
        || (action.action != "drag" && action.steps.is_some())
        || (action.action != "key_sequence" && action.delay_ms.is_some_and(|delay| delay != 0))
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "pixels_only requires a covered foreground pointer, keypress or text action without semantic selectors or alternate input backends",
        ));
    }
    let click = matches!(
        action.action.as_str(),
        "click" | "double_click" | "right_click" | "toggle"
    );
    let drag = action.action == "drag";
    if drag
        && (action.path.len() < 2
            || action.path.len() > 201
            || action.x.is_some()
            || action.y.is_some()
            || !action.keys.is_empty()
            || !action.modifiers.is_empty()
            || action.text.is_some()
            || action.type_chars_only
            || action
                .duration_ms
                .is_some_and(|duration| duration == 0 || duration > MAX_DRAG_DURATION_MS)
            || action
                .steps
                .is_some_and(|steps| !(1..=MAX_DRAG_STEPS).contains(&steps))
            || (action.path.len() > 2 && action.steps.is_some())
            || action
                .path
                .iter()
                .any(|point| !point.x.is_finite() || !point.y.is_finite()))
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "pixels_only drag requires a bounded trajectory, a bounded duration, and no keyboard or semantic fields",
        ));
    }
    if click && (action.duration_ms.is_some() || action.x.is_none() || action.y.is_none()) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "pixels_only clicks require screenshot coordinates and do not support held buttons",
        ));
    }
    let movement = action.action == "move";
    if movement
        && (action.x.is_none()
            || action.y.is_none()
            || action.button.is_some()
            || !action.modifiers.is_empty()
            || !action.keys.is_empty()
            || action.duration_ms.is_some())
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "pixel move requires an unmodified window-local point",
        ));
    }
    if !click
        && !drag
        && !movement
        && (action.x.is_some() || action.y.is_some() || action.button.is_some())
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "pixels_only keypress and text actions use the existing exact foreground focus",
        ));
    }
    let text = matches!(action.action.as_str(), "type" | "type_chars");
    if (text
        && (!action.keys.is_empty()
            || !action.modifiers.is_empty()
            || action.duration_ms.is_some()))
        || (!text && (action.text.is_some() || action.type_chars_only))
        || (click && !action.keys.is_empty())
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "pixel input fields must belong to the requested physical operation",
        ));
    }
    Ok(())
}
