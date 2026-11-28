//! Explicit pixel input uses only local, final-boundary native dispatchers.
use super::*;

#[cfg(windows)]
const DEFAULT_PIXEL_DRAG_DURATION_MS: u64 = 160;

pub(super) use super::pixel_input_policy::{validate_pixel_action, validate_pixel_interrupt};

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
    let position = (
        point(action.x, observation.width, observation.source_rect[0])?,
        point(action.y, observation.height, observation.source_rect[1])?,
    );
    require_observed_pixel_point(&observation.capture_provenance, position)?;
    Ok(position)
}

#[cfg(windows)]
fn physical_drag_points(
    action: &ComputerUseAction,
    observation: &ComputerUseObservation,
) -> ComputerUseResult<Vec<(i32, i32)>> {
    validate_pixel_action(action)?;
    if action.path.len() > 2 && action.steps.is_some() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "steps is only supported for a two-point drag path",
        ));
    }
    let mut points = if action.path.len() == 2 {
        let [start, end] = action.path.as_slice() else {
            unreachable!("path length was checked");
        };
        let steps = action.steps.unwrap_or(20);
        let mut sample = action.clone();
        let mut points = Vec::with_capacity(steps as usize + 1);
        for index in 0..=steps {
            let fraction = f64::from(index) / f64::from(steps);
            sample.x = Some(start.x + (end.x - start.x) * fraction);
            sample.y = Some(start.y + (end.y - start.y) * fraction);
            points.push(physical_point(&sample, observation)?);
        }
        points
    } else {
        action
            .path
            .iter()
            .map(|point| {
                let mut sample = action.clone();
                sample.x = Some(point.x);
                sample.y = Some(point.y);
                physical_point(&sample, observation)
            })
            .collect::<ComputerUseResult<Vec<_>>>()?
    };
    points.dedup();
    if points.len() < 2 || points.first() == points.last() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "drag trajectory must contain two distinct physical pixels",
        ));
    }
    // Also refuse a redacted rectangle between samples; a sparse caller path
    // cannot jump across pixels that were excluded from its observation.
    if let Some(visibility) = observation.capture_provenance.get("visibility") {
        let regions: Vec<[i32; 4]> =
            serde_json::from_value(visibility["redacted_rectangles"].clone()).map_err(|_| {
                ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "invalid pixel redaction evidence",
                )
            })?;
        for pair in points.windows(2) {
            for &[x, y, width, height] in &regions {
                let mut lower = 0.0_f64;
                let mut upper = 1.0_f64;
                for (start, end, minimum, extent) in [
                    (pair[0].0, pair[1].0, x, width),
                    (pair[0].1, pair[1].1, y, height),
                ] {
                    let delta = f64::from(end) - f64::from(start);
                    if delta == 0.0 {
                        if start < minimum
                            || i64::from(start) >= i64::from(minimum) + i64::from(extent)
                        {
                            upper = -1.0;
                        }
                    } else {
                        let a = (f64::from(minimum) - f64::from(start)) / delta;
                        let b = (f64::from(minimum) + f64::from(extent) - f64::from(start)) / delta;
                        lower = lower.max(a.min(b));
                        upper = upper.min(a.max(b));
                    }
                }
                let intersection = (
                    f64::from(pair[0].0) + (f64::from(pair[1].0) - f64::from(pair[0].0)) * lower,
                    f64::from(pair[0].1) + (f64::from(pair[1].1) - f64::from(pair[0].1)) * lower,
                );
                let included_corner = lower == upper
                    && intersection.0 >= f64::from(x)
                    && intersection.0 < f64::from(x) + f64::from(width)
                    && intersection.1 >= f64::from(y)
                    && intersection.1 < f64::from(y) + f64::from(height);
                if lower < upper || included_corner {
                    return Err(ComputerUseError::new(
                        ComputerUseErrorCode::InvalidAction,
                        "drag path crosses a redacted region in the latest observation",
                    ));
                }
            }
        }
    }
    Ok(points)
}

#[cfg(windows)]
fn capture_popup_input_diagnostic(
    diagnostic: &dcc_cua_platform_windows::NativeCapturePopupInputDiagnostic,
) -> ComputerUseCaptureDiagnostic {
    use dcc_cua_platform_windows::NativeCapturePopupRefusal as Native;
    let reason = match diagnostic.reason {
        Native::InvalidSelection => ComputerUseCaptureReason::InvalidSelection,
        Native::RootUnavailable => ComputerUseCaptureReason::RootUnavailable,
        Native::RootNotUsable => ComputerUseCaptureReason::RootNotUsable,
        Native::GuiStateUnavailable => ComputerUseCaptureReason::GuiStateUnavailable,
        Native::FocusOutsideRoot => ComputerUseCaptureReason::FocusOutsideRoot,
        Native::CaptureNotSelected => ComputerUseCaptureReason::CaptureNotSelected,
        Native::PopupUnavailable => ComputerUseCaptureReason::PopupUnavailable,
        Native::PopupIdentityMismatch => ComputerUseCaptureReason::PopupIdentityMismatch,
        Native::PopupNotUsable => ComputerUseCaptureReason::PopupNotUsable,
        Native::EnumerationIncomplete => ComputerUseCaptureReason::EnumerationIncomplete,
        Native::UnselectedOccluder => ComputerUseCaptureReason::UnselectedOccluder,
        Native::NativeStateChanged => ComputerUseCaptureReason::NativeStateChanged,
        Native::NativeStateUnavailable => ComputerUseCaptureReason::NativeStateUnavailable,
    };
    ComputerUseCaptureDiagnostic {
        stage: ComputerUseCaptureStage::NativeCapturePopupInput,
        reason,
        target_process_id: diagnostic.target_process_id,
        target_window_handle: diagnostic.target_window_handle,
        target_bounds: diagnostic.target_bounds,
        blocker_process_id: diagnostic.blocker_process_id,
        blocker_window_handle: diagnostic.blocker_window_handle,
        blocker_bounds: diagnostic.blocker_bounds,
        cloaked: None,
        os_error: None,
        root_bounds_failure: None,
    }
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
        Reason::ExecutionFrameUnavailable => ComputerUseErrorCode::BackendUnavailable,
        Reason::InjectionIncomplete => ComputerUseErrorCode::InputFailed,
    };
    let attempted = failure.outcome.mutation_attempted;
    let capture = failure
        .native_capture_popup
        .as_deref()
        .map(capture_popup_input_diagnostic)
        .or_else(|| {
            failure.native_input_fence.as_ref().and_then(|sample| {
                failure.visibility_failure.as_ref().and_then(|native| {
                    super::exact_capture::map_visible_capture_diagnostic(
                        code,
                        ComputerUseCaptureStage::VisibleDesktopProof,
                        sample.target_process_id,
                        sample.target_window_handle,
                        **native,
                        "native input visibility proof rejected",
                    )
                    .details
                    .and_then(|details| details.capture)
                })
            })
        });
    ComputerUseError::new(code, failure.to_string()).with_details(ComputerUseErrorDetails {
        native_input_fence: failure.native_input_fence.map(|sample| *sample),
        capture,
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
pub(super) struct PixelInputOwner {
    authorization_deadline: Option<Instant>,
    started_generation: u64,
    session_interrupted: bool,
    presentation: dcc_cua_indicator::BannerInputSuppressionSource,
}

#[cfg(windows)]
impl ComputerUseSession {
    pub(super) fn pixel_input_owner(
        &self,
        started_generation: u64,
        authorization_deadline: Option<Instant>,
    ) -> ComputerUseResult<PixelInputOwner> {
        let banner = self.control_banner.as_ref().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "pixel input requires its owning control-banner renderer",
            )
        })?;
        Ok(PixelInputOwner {
            authorization_deadline,
            started_generation,
            session_interrupted: self.control_banner_interrupted(),
            presentation: banner.input_suppression_source(),
        })
    }
}

#[cfg(windows)]
pub(super) async fn perform_exact_pixel_action_with_capture_popup(
    action: &ComputerUseAction,
    observation: &ComputerUseObservation,
    scope: &ComputerUseTargetScope,
    session_id: &str,
    target: &WindowTarget,
    owner: PixelInputOwner,
    capture_popup: Option<super::capture_popup::CapturePopupMetadata>,
) -> ComputerUseResult<ComputerUseToolResult> {
    let PixelInputOwner {
        authorization_deadline,
        started_generation,
        session_interrupted,
        presentation,
    } = owner;
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
    let drag_points = if action.action == "drag" {
        physical_drag_points(&action, observation)?
    } else {
        Vec::new()
    };
    let input_presentation = presentation.clone();
    let require_execution_frame = action.action == "key_sequence";
    let outcome = run_owned_pixel_worker(
        move || {
            // The worker, not its cancellable async waiter, owns both leases
            // through every final fence, native key-up cleanup and postcheck.
            let guard = presentation
                .begin_with_capture(fence.target.process_id, fence.target.window_handle)
                .map_err(pixel_presentation_error)?;
            if require_execution_frame && !presentation.execution_frame_visible() {
                return Err(pixel_presentation_error(IndicatorError::Rendering(
                    "the agent execution frame is unavailable; the foreground target needs a visible exterior edge for a key sequence".into(),
                )));
            }
            Ok(guard)
        },
        move || {
            let gate = || {
                if authorization_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    return Err(dcc_cua_platform_windows::UiaError::PermissionDenied(
                        "task lease expired during native input".into(),
                    ));
                }
                if capture_popup.as_ref().is_some_and(|metadata| {
                    metadata.read_at.elapsed() > Duration::from_secs(5)
                        || Instant::now() >= metadata.authorization_deadline
                }) {
                    return Err(dcc_cua_platform_windows::UiaError::PermissionDenied(
                        "composite popup observation expired".into(),
                    ));
                }
                windows_platform_input_gate("explicit_pixel_final_input")
            };
            let interrupted = || {
                input_presentation.validate_active().is_err()
                    || dcc_cua_interrupt::interrupt_generation_changed(
                        started_generation,
                        dcc_cua_interrupt::interrupt_generation(),
                    )
            };
            match action.action.as_str() {
                "click" | "double_click" | "right_click" | "toggle" => {
                    let button = match action.button.as_deref().unwrap_or(
                        if action.action == "right_click" {
                            "right"
                        } else {
                            "left"
                        },
                    ) {
                        "left" => dcc_cua_platform_windows::WindowsPointerButton::Left,
                        "right" => dcc_cua_platform_windows::WindowsPointerButton::Right,
                        "middle" => dcc_cua_platform_windows::WindowsPointerButton::Middle,
                        _ => unreachable!("validate_action checks button names"),
                    };
                    dcc_cua_platform_windows::send_guarded_click(
                        fence,
                        point.expect("click point was validated"),
                        if action.action == "double_click" {
                            2
                        } else {
                            1
                        },
                        button,
                        &action.modifiers,
                        gate,
                        interrupted,
                    )
                }
                "drag" => {
                    let button = match action.button.as_deref().unwrap_or("left") {
                        "left" => dcc_cua_platform_windows::WindowsPointerButton::Left,
                        "middle" => dcc_cua_platform_windows::WindowsPointerButton::Middle,
                        "right" => dcc_cua_platform_windows::WindowsPointerButton::Right,
                        _ => unreachable!("validate_action checks button names"),
                    };
                    dcc_cua_platform_windows::send_guarded_drag(
                        fence,
                        &drag_points,
                        action.duration_ms.unwrap_or(DEFAULT_PIXEL_DRAG_DURATION_MS),
                        button,
                        gate,
                        interrupted,
                    )
                }
                "key_sequence" => dcc_cua_platform_windows::send_guarded_key_sequence(
                    fence,
                    &action.keys,
                    action.delay_ms.unwrap_or(40),
                    || input_presentation.execution_frame_visible(),
                    gate,
                    interrupted,
                ),
                "keypress" | "keyboard_shortcut" => {
                    let keys = keyboard_shortcut_keys(&action);
                    if let Some(metadata) = &capture_popup {
                        dcc_cua_platform_windows::send_guarded_capture_popup_keypress(
                            fence,
                            &metadata.proof,
                            &keys,
                            action.duration_ms,
                            gate,
                            interrupted,
                        )
                    } else {
                        dcc_cua_platform_windows::send_guarded_keypress(
                            fence,
                            &keys,
                            action.duration_ms,
                            gate,
                            interrupted,
                        )
                    }
                }
                "type" | "type_chars" => {
                    if let Some(metadata) = &capture_popup {
                        dcc_cua_platform_windows::send_guarded_capture_popup_text(
                            fence,
                            &metadata.proof,
                            action.text.as_deref().unwrap_or_default(),
                            gate,
                            interrupted,
                        )
                    } else {
                        dcc_cua_platform_windows::send_guarded_text(
                            fence,
                            action.text.as_deref().unwrap_or_default(),
                            gate,
                            interrupted,
                        )
                    }
                }
                _ => unreachable!("explicit pixel action names were validated"),
            }
            .map_err(input_error)
        },
    )
    .await?;
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

#[cfg(any(windows, test))]
async fn run_owned_pixel_worker<Lease, Outcome>(
    acquire: impl FnOnce() -> ComputerUseResult<Lease> + Send + 'static,
    dispatch: impl FnOnce() -> ComputerUseResult<Outcome> + Send + 'static,
) -> ComputerUseResult<Outcome>
where
    Lease: Send + 'static,
    Outcome: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let _owned_presentation_and_cursor = acquire()?;
        dispatch()
    }).await.map_err(|_| ComputerUseError::new(ComputerUseErrorCode::CompletionUnknown,
        "the guarded native input worker did not return; take a fresh observation before any further input")
        .with_details(ComputerUseErrorDetails { phase: Some(ComputerUseErrorPhase::LocalMutationDispatch),
            action_attempted: Some(true), input_sent: Some(ComputerUseInputState::Unknown),
            completion: Some(ComputerUseCompletionState::Unknown), effect_unknown: Some(true),
            blind_retry: Some(false), fresh_observation_required: Some(true), ..Default::default() }))?
}

#[cfg(windows)]
fn pixel_presentation_error(error: IndicatorError) -> ComputerUseError {
    map_indicator_error("owned indicator input suppression", error).with_details(
        ComputerUseErrorDetails {
            phase: Some(ComputerUseErrorPhase::PreDispatch),
            action_attempted: Some(false),
            input_sent: Some(ComputerUseInputState::NotSent),
            completion: Some(ComputerUseCompletionState::Known),
            effect_unknown: Some(false),
            blind_retry: Some(false),
            fresh_observation_required: Some(true),
            ..Default::default()
        },
    )
}

#[cfg(test)]
mod tests;
