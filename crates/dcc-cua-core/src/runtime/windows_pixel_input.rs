//! Explicit pixel input uses only local, final-boundary native dispatchers.
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

fn validate_pixel_action(action: &ComputerUseAction) -> ComputerUseResult<()> {
    if !matches!(
        action.action.as_str(),
        "click"
            | "double_click"
            | "right_click"
            | "toggle"
            | "keypress"
            | "keyboard_shortcut"
            | "type"
            | "type_chars"
    ) || action.element_index.is_some()
        || action.element_token.is_some()
        || action.input_backend_id.is_some()
        || action
            .delivery_mode
            .as_deref()
            .is_some_and(|mode| mode != "foreground")
        || !action.path.is_empty()
        || action.scroll_x.is_some()
        || action.scroll_y.is_some()
        || action.scroll_by.is_some()
        || action.steps.is_some()
        || action.delay_ms.is_some_and(|delay| delay != 0)
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "pixels_only requires a covered foreground click, keypress or text action without semantic selectors or alternate input backends",
        ));
    }
    let click = matches!(
        action.action.as_str(),
        "click" | "double_click" | "right_click" | "toggle"
    );
    if click && (action.duration_ms.is_some() || action.x.is_none() || action.y.is_none()) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "pixels_only clicks require screenshot coordinates and do not support held buttons",
        ));
    }
    if !click && (action.x.is_some() || action.y.is_some() || action.button.is_some()) {
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

#[cfg(windows)]
fn input_fence(
    action: &ComputerUseAction,
    observation: &ComputerUseObservation,
    scope: &ComputerUseTargetScope,
    session_id: &str,
    target: &WindowTarget,
) -> ComputerUseResult<dcc_cua_platform_windows::WindowsPhysicalInputFence> {
    let stale = || {
        ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "take a fresh pixel screenshot with complete native physical geometry before input",
        )
    };
    let instance = super::window_commands::exact_native_observation_instance(
        observation,
        action.observation_id.as_deref().unwrap_or_default(),
        scope,
        session_id,
    )?;
    let provenance = &observation.capture_provenance;
    if provenance["observation_mode"] != "pixels_only"
        || provenance["accessibility_available"] != false
        || target.pid != observation.process_id
        || target.window_id != observation.window_handle
    {
        return Err(stale());
    }
    let geometry = super::window_commands::exact_native_observation_geometry(observation)?;
    if geometry.win32_bounds != target.bounds {
        return Err(stale());
    }
    Ok(dcc_cua_platform_windows::WindowsPhysicalInputFence {
        target: dcc_cua_platform_windows::UiaTarget {
            process_id: target.pid,
            window_handle: target.window_id,
        },
        native_instance: instance,
        native_window_bounds: geometry.win32_bounds,
        native_visible_bounds: geometry.dwm_bounds.ok_or_else(stale)?,
        window_dpi: geometry.dpi,
    })
}

#[cfg(windows)]
fn physical_point(
    action: &ComputerUseAction,
    observation: &ComputerUseObservation,
) -> ComputerUseResult<(i32, i32)> {
    if [observation.source_rect[2], observation.source_rect[3]]
        != [
            i32::try_from(observation.width).unwrap_or(-1),
            i32::try_from(observation.height).unwrap_or(-1),
        ]
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "native pixel input requires one physical pixel per screenshot pixel",
        ));
    }
    let point = |coordinate: Option<f64>, size: u32, origin: i32| {
        let coordinate = coordinate
            .filter(|number| number.is_finite() && *number >= 0.0 && *number < f64::from(size))
            .ok_or_else(|| {
                ComputerUseError::new(
                    ComputerUseErrorCode::InvalidAction,
                    "pixel coordinates must remain inside the latest screenshot",
                )
            })?;
        let physical = f64::from(origin) + coordinate;
        if physical < f64::from(i32::MIN) || physical > f64::from(i32::MAX) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "physical screenshot coordinates overflow",
            ));
        }
        Ok(physical.floor() as i32)
    };
    Ok((
        point(action.x, observation.width, observation.source_rect[0])?,
        point(action.y, observation.height, observation.source_rect[1])?,
    ))
}

#[cfg(windows)]
fn input_error(failure: dcc_cua_platform_windows::WindowsGuardedInputError) -> ComputerUseError {
    use dcc_cua_platform_windows::WindowsPhysicalInputFailureReason as Reason;
    let code = match failure.reason {
        Reason::InvalidRequest | Reason::PointOutsideTarget => ComputerUseErrorCode::InvalidAction,
        Reason::InstanceChanged | Reason::BoundsChanged | Reason::DpiChanged => {
            ComputerUseErrorCode::StaleObservation
        }
        Reason::TargetMinimized => ComputerUseErrorCode::TargetMinimized,
        Reason::TargetUnavailable
        | Reason::TargetHidden
        | Reason::TargetNotForeground
        | Reason::TargetOccluded
        | Reason::TargetOutsideDesktop => ComputerUseErrorCode::TargetUnavailable,
        Reason::DesktopUnavailable => ComputerUseErrorCode::InteractiveDesktopUnavailable,
        Reason::Interrupted => ComputerUseErrorCode::UserInterrupted,
        Reason::InjectionIncomplete => ComputerUseErrorCode::InputFailed,
    };
    let attempted = failure.outcome.mutation_attempted;
    ComputerUseError::new(code, failure.to_string()).with_details(ComputerUseErrorDetails {
        phase: Some(if attempted {
            ComputerUseErrorPhase::LocalMutationDispatch
        } else {
            ComputerUseErrorPhase::PreDispatch
        }),
        action_attempted: Some(attempted),
        input_sent: Some(
            if failure.outcome.inserted_events > 0 || failure.outcome.cursor_move_succeeded {
                ComputerUseInputState::Sent
            } else if failure.outcome.cursor_move_attempted {
                ComputerUseInputState::Unknown
            } else {
                ComputerUseInputState::NotSent
            },
        ),
        completion: Some(if attempted {
            ComputerUseCompletionState::Unknown
        } else {
            ComputerUseCompletionState::Known
        }),
        effect_unknown: Some(attempted),
        automatic_input: Some(false),
        blind_retry: Some(false),
        fresh_observation_required: Some(true),
        ..Default::default()
    })
}

#[cfg(windows)]
pub(super) async fn perform_exact_pixel_action(
    action: &ComputerUseAction,
    observation: &ComputerUseObservation,
    scope: &ComputerUseTargetScope,
    session_id: &str,
    target: &WindowTarget,
    started_generation: u64,
    session_interrupted: bool,
) -> ComputerUseResult<ComputerUseToolResult> {
    validate_pixel_interrupt(
        started_generation,
        dcc_cua_interrupt::interrupt_generation(),
        session_interrupted,
    )?;
    validate_pixel_action(action)?;
    let fence = input_fence(action, observation, scope, session_id, target)?;
    let native_instance = fence.native_instance;
    let point = if matches!(
        action.action.as_str(),
        "click" | "double_click" | "right_click" | "toggle"
    ) {
        Some(physical_point(action, observation)?)
    } else {
        None
    };
    let action = action.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let gate = || windows_platform_input_gate("explicit_pixel_final_input");
        let interrupted = || dcc_cua_interrupt::interrupt_generation_changed(
            started_generation, dcc_cua_interrupt::interrupt_generation());
        match action.action.as_str() {
            "click" | "double_click" | "right_click" | "toggle" => {
                let button = match action.button.as_deref().unwrap_or(if action.action == "right_click" { "right" } else { "left" }) {
                    "left" => dcc_cua_platform_windows::WindowsPointerButton::Left,
                    "right" => dcc_cua_platform_windows::WindowsPointerButton::Right,
                    "middle" => dcc_cua_platform_windows::WindowsPointerButton::Middle,
                    _ => unreachable!("validate_action checks button names"),
                };
                dcc_cua_platform_windows::send_guarded_click(fence, point.expect("click point was validated"),
                    if action.action == "double_click" { 2 } else { 1 }, button, &action.modifiers, gate, interrupted)
            }
            "keypress" | "keyboard_shortcut" => {
                let keys = keyboard_shortcut_keys(&action);
                dcc_cua_platform_windows::send_guarded_keypress(fence, &keys, action.duration_ms, gate, interrupted)
            }
            "type" | "type_chars" => dcc_cua_platform_windows::send_guarded_text(
                fence, action.text.as_deref().unwrap_or_default(), gate, interrupted),
            _ => unreachable!("explicit pixel action names were validated"),
        }
    }).await.map_err(|_| ComputerUseError::new(ComputerUseErrorCode::CompletionUnknown,
        "the guarded native input worker did not return; take a fresh observation before any further input")
        .with_details(ComputerUseErrorDetails { phase: Some(ComputerUseErrorPhase::LocalMutationDispatch),
            action_attempted: Some(true), input_sent: Some(ComputerUseInputState::Unknown),
            completion: Some(ComputerUseCompletionState::Unknown), effect_unknown: Some(true),
            blind_retry: Some(false), fresh_observation_required: Some(true), ..Default::default() }))?
        .map_err(input_error)?;
    let delivered = outcome.delivery_completed && outcome.post_dispatch_validated;
    Ok(ComputerUseToolResult {
        status: if delivered { ComputerUseToolStatus::Succeeded } else { ComputerUseToolStatus::Rejected },
        value: json!({"success":delivered,"route":"windows_exact_pixel_final_input",
            "target":{"process_id":target.pid,"window_handle":target.window_id},
            "native_instance":native_instance,"delivery":outcome,
            "effect":"unverifiable","verification_required":true,"fresh_observation_required":true}),
        text: "Submitted guarded exact-window input; verify the application's actual state separately.".into(),
        images: Vec::new(), degraded: false,
    })
}

#[cfg(test)]
mod tests;
