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
    let values = provenance["native_window_bounds"]
        .as_array()
        .ok_or_else(stale)?;
    let mut bounds = [0_i32; 4];
    if values.len() != bounds.len() {
        return Err(stale());
    }
    for (destination, value) in bounds.iter_mut().zip(values) {
        *destination = value
            .as_i64()
            .and_then(|number| i32::try_from(number).ok())
            .ok_or_else(stale)?;
    }
    let dpi = provenance["window_dpi"]
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .filter(|number| *number > 0)
        .ok_or_else(stale)?;
    if bounds != target.bounds
        || bounds[2] <= 0
        || bounds[3] <= 0
        || observation.width == 0
        || observation.height == 0
        || observation.source_rect[2] <= 0
        || observation.source_rect[3] <= 0
    {
        return Err(stale());
    }
    Ok(dcc_cua_platform_windows::WindowsPhysicalInputFence {
        target: dcc_cua_platform_windows::UiaTarget {
            process_id: target.pid,
            window_handle: target.window_id,
        },
        native_instance: instance,
        native_window_bounds: bounds,
        window_dpi: dpi,
    })
}

#[cfg(windows)]
fn physical_point(
    action: &ComputerUseAction,
    observation: &ComputerUseObservation,
) -> ComputerUseResult<(i32, i32)> {
    let point = |coordinate: Option<f64>, size: u32, origin: i32, extent: i32| {
        let coordinate = coordinate
            .filter(|number| number.is_finite() && *number >= 0.0 && *number < f64::from(size))
            .ok_or_else(|| {
                ComputerUseError::new(
                    ComputerUseErrorCode::InvalidAction,
                    "pixel coordinates must remain inside the latest screenshot",
                )
            })?;
        let physical = f64::from(origin) + coordinate * f64::from(extent) / f64::from(size);
        if physical < f64::from(i32::MIN) || physical > f64::from(i32::MAX) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "physical screenshot coordinates overflow",
            ));
        }
        Ok(physical.floor() as i32)
    };
    Ok((
        point(
            action.x,
            observation.width,
            observation.source_rect[0],
            observation.source_rect[2],
        )?,
        point(
            action.y,
            observation.height,
            observation.source_rect[1],
            observation.source_rect[3],
        )?,
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
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(7, 7, false, true)]
    #[case(7, 8, false, false)]
    #[case(8, 8, true, false)]
    fn interruption_before_or_during_preflight_cannot_become_a_new_baseline(
        #[case] started: u64,
        #[case] current: u64,
        #[case] latched: bool,
        #[case] accepted: bool,
    ) {
        let result = validate_pixel_interrupt(started, current, latched);
        assert_eq!(result.is_ok(), accepted);
        if let Err(error) = result {
            let details = error.details.unwrap();
            assert_eq!(error.code, ComputerUseErrorCode::UserInterrupted);
            assert_eq!(details.action_attempted, Some(false));
            assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
        }
    }

    #[cfg(windows)]
    #[rstest]
    #[case(false, false, 0, ComputerUseInputState::NotSent)]
    #[case(true, false, 0, ComputerUseInputState::Unknown)]
    #[case(true, true, 0, ComputerUseInputState::Sent)]
    #[case(false, false, 1, ComputerUseInputState::Sent)]
    fn failed_input_preserves_partial_cursor_and_event_delivery(
        #[case] cursor_move_attempted: bool,
        #[case] cursor_move_succeeded: bool,
        #[case] inserted_events: u32,
        #[case] expected: ComputerUseInputState,
    ) {
        use dcc_cua_platform_windows::{
            WindowsGuardedInputError, WindowsGuardedInputOperation, WindowsGuardedInputOutcome,
            WindowsPhysicalInputFailureReason,
        };
        let attempted = cursor_move_attempted || inserted_events > 0;
        let error = input_error(WindowsGuardedInputError {
            reason: WindowsPhysicalInputFailureReason::Interrupted,
            outcome: WindowsGuardedInputOutcome {
                operation: WindowsGuardedInputOperation::Click,
                mutation_attempted: attempted,
                cursor_move_attempted,
                cursor_move_succeeded,
                requested_events: 2,
                inserted_events,
                cleanup_requested_events: 0,
                cleanup_inserted_events: 0,
                delivery_completed: false,
                post_dispatch_validated: false,
                fresh_observation_required: true,
            },
        });
        let details = error.details.unwrap();
        assert_eq!(details.input_sent, Some(expected));
        assert_eq!(details.action_attempted, Some(attempted));
        assert_eq!(details.effect_unknown, Some(attempted));
        assert_eq!(details.fresh_observation_required, Some(true));
        assert_eq!(details.blind_retry, Some(false));
    }

    #[rstest]
    #[case("click", true)]
    #[case("double_click", true)]
    #[case("right_click", true)]
    #[case("toggle", true)]
    #[case("keypress", true)]
    #[case("keyboard_shortcut", true)]
    #[case("type", true)]
    #[case("type_chars", true)]
    #[case("drag", false)]
    #[case("move", false)]
    #[case("scroll", false)]
    #[case("set_text", false)]
    fn explicit_pixel_routes_are_closed(#[case] name: &str, #[case] accepted: bool) {
        let mut action = ComputerUseAction {
            action: name.into(),
            ..Default::default()
        };
        if matches!(name, "click" | "double_click" | "right_click" | "toggle") {
            action.x = Some(1.0);
            action.y = Some(2.0);
        }
        assert_eq!(validate_pixel_action(&action).is_ok(), accepted);
    }

    #[rstest]
    #[case("selector")]
    #[case("index")]
    #[case("backend")]
    #[case("background")]
    #[case("duration")]
    #[case("path")]
    #[case("delay")]
    fn pixel_input_never_falls_through_to_other_dispatchers(#[case] variant: &str) {
        let mut action = ComputerUseAction {
            action: "click".into(),
            x: Some(1.0),
            y: Some(2.0),
            ..Default::default()
        };
        match variant {
            "selector" => action.element_token = Some("other".into()),
            "index" => action.element_index = Some(1),
            "backend" => action.input_backend_id = Some("other".into()),
            "background" => action.delivery_mode = Some("background".into()),
            "duration" => action.duration_ms = Some(1),
            "path" => action.path.push(ComputerUsePoint { x: 1.0, y: 2.0 }),
            "delay" => action.delay_ms = Some(1),
            _ => unreachable!(),
        }
        assert_eq!(
            validate_pixel_action(&action).unwrap_err().code,
            ComputerUseErrorCode::InvalidAction
        );
    }

    #[cfg(windows)]
    fn observed() -> ComputerUseObservation {
        ComputerUseObservation {
            observation_id: "pixel-1".into(),
            window_handle: 99,
            process_id: 42,
            window_title: String::new(),
            width: 200,
            height: 100,
            source_rect: [-300, 80, 400, 200],
            capture_backend: "dcc-cua-visible-exact-window".into(),
            session_id: "owned".into(),
            capture_provenance: json!({
                "process_id":42,"window_handle":99,"pixels_captured":true,"whole_desktop_capture":false,
                "scope":"window","capture_generation":1,"window_dpi":240,
                "native_window_bounds":[-310,70,420,220],"observation_mode":"pixels_only","accessibility_available":false,
                "native_instance":{"process_creation_time_100ns":7,"window_thread_id":8,
                    "window_class_hash":9,"owner_window_handle":0}
            }),
        }
    }

    #[cfg(windows)]
    #[rstest]
    #[case("valid", true)]
    #[case("missing_bounds", false)]
    #[case("wrong_bounds", false)]
    #[case("overflow_bounds", false)]
    #[case("overflow_dpi", false)]
    #[case("semantic", false)]
    #[case("wrong_pid", false)]
    #[case("wrong_hwnd", false)]
    #[case("zero_source", false)]
    fn native_input_requires_the_actual_physical_capture_fence(
        #[case] variant: &str,
        #[case] accepted: bool,
    ) {
        let mut observation = observed();
        let mut target = WindowTarget {
            pid: 42,
            window_id: 99,
            title: String::new(),
            app_name: String::new(),
            bounds: [-310, 70, 420, 220],
            is_on_screen: true,
            is_minimized: false,
            z_index: None,
            is_foreground: true,
        };
        match variant {
            "missing_bounds" => {
                observation.capture_provenance["native_window_bounds"] = Value::Null
            }
            "wrong_bounds" => target.bounds[0] += 1,
            "overflow_bounds" => {
                observation.capture_provenance["native_window_bounds"][0] = json!(2147483648_i64)
            }
            "overflow_dpi" => observation.capture_provenance["window_dpi"] = json!(4294967296_u64),
            "semantic" => observation.capture_provenance["observation_mode"] = json!("semantic"),
            "wrong_pid" => target.pid += 1,
            "wrong_hwnd" => target.window_id += 1,
            "zero_source" => observation.source_rect[2] = 0,
            _ => {}
        }
        let scope = ComputerUseTargetScope {
            process_id: Some(42),
            window_handle: Some(99),
            ..Default::default()
        };
        let action = ComputerUseAction {
            action: "click".into(),
            observation_id: Some("pixel-1".into()),
            ..Default::default()
        };
        assert_eq!(
            input_fence(&action, &observation, &scope, "owned", &target).is_ok(),
            accepted
        );
    }

    #[cfg(windows)]
    #[test]
    fn cropped_scaled_pixels_use_the_saved_physical_origin_including_negative_monitors() {
        let observation = observed();
        let action = ComputerUseAction {
            action: "click".into(),
            x: Some(100.0),
            y: Some(50.0),
            ..Default::default()
        };
        assert_eq!(physical_point(&action, &observation).unwrap(), (-100, 180));
        let outside = ComputerUseAction {
            x: Some(200.0),
            ..action
        };
        assert_eq!(
            physical_point(&outside, &observation).unwrap_err().code,
            ComputerUseErrorCode::InvalidAction
        );
    }
}
