use super::*;
use crate::request_contract::*;

pub(super) async fn acquire_raw_input_turn(
    enabled: bool,
) -> Option<tokio::sync::MutexGuard<'static, ()>> {
    if enabled {
        Some(RAW_INPUT_QUEUE.lock().await)
    } else {
        None
    }
}

pub(super) fn ensure_connection_session_capacity(
    active_session_count: usize,
) -> Result<(), HostError> {
    if active_session_count < MAX_SESSIONS_PER_CONNECTION {
        return Ok(());
    }
    Err(HostError::Protocol(format!(
        "connection session limit reached (maximum {})",
        MAX_SESSIONS_PER_CONNECTION
    )))
}

pub(super) fn finish_window_mutation_attempt<T, E>(
    result: Result<T, E>,
    invalidate: impl FnOnce(),
) -> Result<T, E> {
    invalidate();
    result
}

pub(super) fn session_stopped_response(
    session_id: &str,
    result: ComputerUseSessionStopResult,
) -> Value {
    json!({
        "type": "session_stopped",
        "session_id": session_id,
        "success": result.success,
        "active": result.active,
        "cleanup_pending": result.cleanup_pending,
        "cleanup_issues": result.cleanup_issues,
        "marker": result.marker,
    })
}

pub(super) fn observed_window_state_response(
    host: &mut HostSession,
    session_id: &str,
    state: Value,
) -> Value {
    host.observe_target_state(&state);
    json!({"type":"window_state", "session_id":session_id, "state":state})
}

pub(super) fn finish_target_sensitive_cached_read(
    host: &mut HostSession,
    availability: ComputerUseResult<dcc_cua_core::ComputerUseTargetAvailability>,
) -> ComputerUseResult<()> {
    let availability = host.finish_observation_sensitive_attempt(availability)?;
    let status = availability.status;
    host.observe_target_availability(availability);
    if host.input_events.current().status == dcc_cua_core::ComputerUseInputStatus::Suspended {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
            host.input_events
                .current()
                .reason
                .clone()
                .unwrap_or_else(|| "interactive desktop is unavailable".into()),
        ));
    }
    match status {
        dcc_cua_core::ComputerUseTargetStatus::Available => Ok(()),
        dcc_cua_core::ComputerUseTargetStatus::Minimized => Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetMinimized,
            "target_minimized: cached accessibility evidence is stale; automatic_input=false",
        )),
        dcc_cua_core::ComputerUseTargetStatus::Unavailable => Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetUnavailable,
            "target_unavailable: cached accessibility evidence is stale; automatic_input=false",
        )),
    }
}

pub(super) fn bind_launched_process(
    launched: &HostLaunchSession,
    grant: &mut TaskGrant,
) -> Result<(), HostError> {
    if grant.task_grant_id != launched.task_grant_id
        || grant.application_label != launched.application_label
    {
        return Err(HostError::Protocol(
            "launch and window session grants do not match".into(),
        ));
    }
    if grant
        .process_id
        .is_some_and(|process_id| process_id != launched.process_id)
    {
        return Err(HostError::Protocol(
            "window session does not target the launched process".into(),
        ));
    }
    grant.process_id = Some(launched.process_id);
    Ok(())
}

pub(super) fn restore_activate_available(grant: &TaskGrant) -> bool {
    cfg!(windows) && grant.process_id.is_some() && grant.window_handle.is_some()
}

pub(super) fn session_health_state_changed(
    evidence_epoch_before: dcc_cua_core::ActionEvidenceEpoch,
    transition_sequence_before: u64,
    action_evidence_epoch: dcc_cua_core::ActionEvidenceEpoch,
    transition_sequence: u64,
) -> bool {
    evidence_epoch_before != action_evidence_epoch
        || transition_sequence_before != transition_sequence
}

async fn refresh_session_health_input_and_target(host: &mut HostSession) -> bool {
    host.refresh_input_readiness();
    let availability = host.session.target_availability().await;
    let target_probe_failed = availability.is_err();
    if let Ok(availability) = host.finish_observation_sensitive_attempt(availability) {
        host.observe_target_availability(availability);
    }
    target_probe_failed
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WindowEvidenceEpochRoute {
    pub(super) session_id: String,
    pub(super) publication: HostEvidencePublication,
}

pub(super) fn window_evidence_epoch_route(request: &Request) -> Option<WindowEvidenceEpochRoute> {
    let standard = |session_id: &str| WindowEvidenceEpochRoute {
        session_id: session_id.to_owned(),
        publication: HostEvidencePublication::None,
    };
    match request {
        Request::BrowserSnapshot { session_id, .. } => Some(WindowEvidenceEpochRoute {
            session_id: session_id.clone(),
            publication: HostEvidencePublication::BrowserSnapshotAttempt,
        }),
        Request::TerminateApp { session_id, .. }
        | Request::ClipboardRead { session_id, .. }
        | Request::ClipboardWrite { session_id, .. }
        | Request::RecordingStart { session_id, .. }
        | Request::RecordingStop { session_id, .. }
        | Request::RecordingState { session_id, .. }
        | Request::LiveObservationStart { session_id, .. }
        | Request::LiveObservationState { session_id, .. }
        | Request::LiveObservationStop { session_id, .. }
        | Request::GetWindowState { session_id, .. }
        | Request::ChangeWindowState { session_id, .. }
        | Request::SetWindowFrame { session_id, .. }
        | Request::InvokeMenu { session_id, .. }
        | Request::Snapshot { session_id, .. }
        | Request::Zoom { session_id, .. }
        | Request::AccessibilitySnapshot { session_id, .. }
        | Request::VerifyState { session_id, .. }
        | Request::CallTool { session_id, .. }
        | Request::BrowserPrepare { session_id, .. }
        | Request::BrowserNavigate { session_id, .. }
        | Request::BrowserClick { session_id, .. }
        | Request::BrowserType { session_id, .. }
        | Request::BrowserPointer { session_id, .. }
        | Request::BrowserSetInputFiles { session_id, .. }
        | Request::BrowserDownload { session_id, .. }
        | Request::BrowserDialog { session_id, .. }
        | Request::BrowserExtensionStatus { session_id, .. }
        | Request::BrowserExtensionCall { session_id, .. }
        | Request::Find { session_id, .. }
        | Request::WaitFor { session_id, .. }
        | Request::ExecuteAction { session_id, .. }
        | Request::ResumeSession { session_id, .. }
        | Request::GetSessionState { session_id, .. }
        | Request::GetInputState { session_id, .. }
        | Request::SessionHealth { session_id, .. }
        | Request::PollSessionEvents { session_id, .. }
        | Request::CursorTool { session_id, .. }
        | Request::EscalateSession { session_id, .. } => Some(standard(session_id)),
        Request::Hello(_)
        | Request::Ping {}
        | Request::Doctor {}
        | Request::RegisterBrowserExtension { .. }
        | Request::BrowserExtensionNext { .. }
        | Request::CompleteBrowserExtension { .. }
        | Request::UnregisterBrowserExtension { .. }
        | Request::InterruptAll {}
        | Request::ListApps {}
        | Request::ListTools {}
        | Request::ListWindows { .. }
        | Request::WaitForWindow(_)
        | Request::DesktopSnapshot {}
        | Request::ScreenSize {}
        | Request::CursorPosition {}
        | Request::OpenDesktopSession { .. }
        | Request::DesktopSessionSnapshot { .. }
        | Request::ExecuteDesktopAction { .. }
        | Request::StopDesktopSession { .. }
        | Request::LaunchApp { .. }
        | Request::OpenSession { .. }
        | Request::CallGlobalTool { .. }
        | Request::StopSession { .. }
        | Request::Cancel { .. }
        | Request::CancelWindowWait { .. } => None,
    }
}

fn prepare_window_evidence_request(
    sessions: &mut ConnectionSessions,
    route: Option<&WindowEvidenceEpochRoute>,
) {
    if let Some(host) = route.and_then(|route| sessions.windows.get_mut(&route.session_id)) {
        host.synchronize_action_evidence_epoch();
    }
}

pub(super) fn finish_window_evidence_request<T>(
    sessions: &mut ConnectionSessions,
    route: Option<WindowEvidenceEpochRoute>,
    result: Result<T, HostError>,
) -> Result<T, HostError> {
    if let Some(route) = route
        && let Some(host) = sessions.windows.get_mut(&route.session_id)
    {
        if !host.interrupted {
            host.mark_activity();
        }
        if result.is_ok() {
            host.synchronize_action_evidence_epoch_with(route.publication);
        } else {
            host.synchronize_action_evidence_epoch();
            if matches!(
                route.publication,
                HostEvidencePublication::BrowserSnapshot
                    | HostEvidencePublication::BrowserSnapshotAttempt
            ) {
                host.discard_browser_evidence();
            }
        }
    }
    result
}

pub(super) async fn handle_request_with_confirmation_host(
    driver: &ComputerUseDriver,
    confirmation_host: Option<&dyn TrustedActionConfirmationHost>,
    sessions: &mut ConnectionSessions,
    snapshot_transport: &mut Option<SnapshotTransport>,
    desktop_shared_image: &mut Option<SharedImage>,
    cancellation_registry: &CancellationRegistry,
    request: Request,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let evidence_route = window_evidence_epoch_route(&request);
    prepare_window_evidence_request(sessions, evidence_route.as_ref());
    let result = handle_request_inner(
        driver,
        confirmation_host,
        sessions,
        snapshot_transport,
        desktop_shared_image,
        cancellation_registry,
        request,
    )
    .await;
    finish_window_evidence_request(sessions, evidence_route, result)
}

async fn handle_request_inner(
    driver: &ComputerUseDriver,
    confirmation_host: Option<&dyn TrustedActionConfirmationHost>,
    sessions: &mut ConnectionSessions,
    snapshot_transport: &mut Option<SnapshotTransport>,
    desktop_shared_image: &mut Option<SharedImage>,
    cancellation_registry: &CancellationRegistry,
    request: Request,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    if let Request::Hello(params) = &request {
        if snapshot_transport.is_some() {
            return Err(HostError::Protocol(
                "hello has already completed on this connection".into(),
            ));
        }
        let transport = SnapshotTransport::from_hello(params)?;
        sessions.agent_name = params.client_name.clone();
        *snapshot_transport = Some(transport);
        return Ok((
            json!({
                "type": "hello",
                "protocol_version": HOST_PROTOCOL_VERSION,
                "client_name": params.client_name,
                "snapshot_transport": match transport {
                    SnapshotTransport::SharedMemory => "shared_memory",
                    SnapshotTransport::BinaryFrame => "binary_frame",
                },
                "capabilities": host_capabilities(cursor_render_backend(
                    driver.upstream_cursor_renderer_enabled(),
                ) != "unavailable"),
            }),
            None,
        ));
    }
    let mode = snapshot_transport
        .ok_or_else(|| HostError::Protocol("hello is required before stateful requests".into()))?;
    let agent_name = sessions.agent_name.clone();
    let ConnectionSessions {
        agent_name: _,
        windows: sessions,
        desktops: desktop_sessions,
        launches: launch_sessions,
    } = sessions;

    let request = match browser_extension::route_host_request(request, sessions).await {
        browser_extension::RoutedBrowserExtensionRequest::Unhandled(request) => *request,
        browser_extension::RoutedBrowserExtensionRequest::Handled(result) => return result,
    };
    match request {
        Request::Hello(_) => unreachable!(),
        Request::Ping {} => Ok((ping_response(), None)),
        Request::Doctor {} => Ok((driver.diagnostics().await, None)),
        Request::RegisterBrowserExtension { .. }
        | Request::BrowserExtensionNext { .. }
        | Request::CompleteBrowserExtension { .. }
        | Request::UnregisterBrowserExtension { .. }
        | Request::BrowserExtensionStatus { .. }
        | Request::BrowserExtensionCall { .. } => unreachable!("extension request was routed"),
        Request::InterruptAll {} => {
            let generation = broadcast_interrupt();
            let (window_sessions, desktop_sessions, launch_sessions) =
                stop_sessions(driver, sessions, desktop_sessions, launch_sessions).await;
            Ok((
                json!({
                    "type": "interrupt_broadcast",
                    "scope": "host_process",
                    "generation": generation,
                    "stopped_window_sessions": window_sessions,
                    "stopped_desktop_sessions": desktop_sessions,
                    "stopped_launch_sessions": launch_sessions,
                }),
                None,
            ))
        }
        Request::Cancel {
            session_id,
            task_grant_id,
            window_capability,
        } => Ok((
            cancel_wait(
                cancellation_registry,
                &session_id,
                &task_grant_id,
                &window_capability,
            )?,
            None,
        )),
        Request::CancelWindowWait { wait_id } => {
            Ok((cancel_window_wait(cancellation_registry, &wait_id)?, None))
        }
        Request::ListApps {} => {
            let apps = driver.list_apps().await?;
            Ok((json!({"type":"apps", "apps":apps}), None))
        }
        Request::ListTools {} => Ok((
            json!({"type":"tools", "tools":driver.list_tools().await?}),
            None,
        )),
        Request::ListWindows {
            app,
            pid,
            window_id,
            window_title,
            on_screen_only,
        } => Ok((
            list_windows_response(
                driver,
                app.as_deref(),
                pid,
                window_id,
                window_title.as_deref(),
                on_screen_only,
            )
            .await?,
            None,
        )),
        Request::WaitForWindow(request) => Ok((
            json!({"type":"window_ready", "result":driver.wait_for_window(&request).await?}),
            None,
        )),
        Request::DesktopSnapshot {} => {
            let snapshot = driver.desktop_snapshot().await?;
            let (image, attachment) = match mode {
                SnapshotTransport::SharedMemory => {
                    let shared = SharedImage::from_bytes(&snapshot.data, "image/png")
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
                    let mut descriptor = serde_json::to_value(shared.descriptor())
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
                    descriptor["encoding"] = Value::String("shared_memory".into());
                    *desktop_shared_image = Some(shared);
                    (descriptor, None)
                }
                SnapshotTransport::BinaryFrame => (
                    json!({
                        "name": "",
                        "id": format!("desktop-{}", Uuid::new_v4()),
                        "length": snapshot.data.len(),
                        "mime_type": "image/png",
                        "encoding": "binary_frame",
                    }),
                    Some(snapshot.data),
                ),
            };
            Ok((
                json!({"type":"desktop_snapshot", "state":snapshot.state, "image":image}),
                attachment,
            ))
        }
        Request::ScreenSize {} => Ok((
            json!({"type":"screen_size", "result":driver.screen_size().await?}),
            None,
        )),
        Request::CursorPosition {} => Ok((
            json!({"type":"cursor_position", "result":driver.cursor_position().await?}),
            None,
        )),
        Request::OpenDesktopSession { session_id, grant } => {
            if desktop_sessions.contains_key(&session_id) {
                return Err(HostError::Protocol("desktop session already exists".into()));
            }
            ensure_connection_session_capacity(
                sessions.len() + desktop_sessions.len() + launch_sessions.len(),
            )?;
            grant.validate_identity()?;
            let session_generation = interrupt_generation();
            let runtime_session_id = new_runtime_session_id("desktop");
            let mut session = driver
                .desktop_session_with_agent(agent_name.clone(), runtime_session_id.clone())?;
            let started = session.start().await?;
            let capability = format!("cua-desktop-{}", Uuid::new_v4());
            desktop_sessions.insert(
                session_id.clone(),
                HostDesktopSession {
                    runtime_session_id,
                    task_grant_id: grant.task_grant_id,
                    allow_raw_input: grant.allow_raw_input,
                    allow_trusted_confirmation: grant.allow_trusted_confirmation,
                    capability: capability.clone(),
                    interrupt_generation: session_generation,
                    interrupted: false,
                    session,
                    latest_observation_id: None,
                    latest_shared_image: None,
                },
            );
            Ok((
                json!({
                    "type":"desktop_session_opened",
                    "session_id":session_id,
                    "desktop_capability":capability,
                    "started":started,
                }),
                None,
            ))
        }
        Request::DesktopSessionSnapshot {
            session_id,
            task_grant_id,
            desktop_capability,
        } => {
            let host = authorized_desktop_session(
                desktop_sessions,
                &session_id,
                &task_grant_id,
                &desktop_capability,
            )
            .await?;
            let snapshot = host.session.screenshot().await?;
            let current_observation_id = snapshot.observation_id.clone();
            let (image, attachment) = match mode {
                SnapshotTransport::SharedMemory => {
                    let shared = SharedImage::from_bytes(&snapshot.data, "image/png")
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
                    let mut descriptor = serde_json::to_value(shared.descriptor())
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
                    descriptor["encoding"] = Value::String("shared_memory".into());
                    host.latest_shared_image = Some(shared);
                    (descriptor, None)
                }
                SnapshotTransport::BinaryFrame => (
                    json!({
                        "name": "",
                        "id": current_observation_id,
                        "length": snapshot.data.len(),
                        "mime_type": "image/png",
                        "encoding": "binary_frame",
                    }),
                    Some(snapshot.data),
                ),
            };
            host.latest_observation_id = Some(snapshot.observation_id.clone());
            Ok((
                json!({
                    "type":"desktop_snapshot",
                    "session_id":session_id,
                    "observation_id":snapshot.observation_id,
                    "state":snapshot.state,
                    "image":image,
                }),
                attachment,
            ))
        }
        Request::ExecuteDesktopAction {
            session_id,
            task_grant_id,
            desktop_capability,
            observation_id,
            action,
            capture_after,
            post_snapshot_delay_ms,
        } => {
            let post_snapshot_delay = post_snapshot_delay(capture_after, post_snapshot_delay_ms)?;
            let host = authorized_desktop_session(
                desktop_sessions,
                &session_id,
                &task_grant_id,
                &desktop_capability,
            )
            .await?;
            if !host.allow_raw_input {
                return Err(HostError::Protocol("raw input is not granted".into()));
            }
            if let Some((code, message)) = action.reject_policy() {
                return Ok((
                    json!({
                        "type":"action_completed",
                        "success":false,
                        "policy_tier":code,
                        "message":message,
                        "error":code,
                    }),
                    None,
                ));
            }
            if host.latest_observation_id.as_deref() != Some(observation_id.as_str()) {
                return Err(HostError::ComputerUse(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "desktop action observation_id does not match the latest host snapshot",
                )));
            }
            if action.requires_approval() {
                let request = TrustedActionConfirmationRequest::for_desktop_action(
                    &session_id,
                    &task_grant_id,
                    &desktop_capability,
                    &observation_id,
                    &action,
                )?;
                let outcome = authorize_action_confirmation(
                    confirmation_host,
                    host.allow_trusted_confirmation,
                    request,
                )
                .await;
                if outcome != ActionConfirmationOutcome::Allowed {
                    return Ok(action_confirmation_refusal(outcome));
                }
            }
            let action = action.into_computer_use(observation_id)?;
            let input_turn = acquire_raw_input_turn(true).await;
            host.latest_observation_id = None;
            let result = host.session.perform_action(&action).await?;
            let action_id = format!("cua-desktop-action-{}", Uuid::new_v4());
            if capture_after {
                tokio::time::sleep(post_snapshot_delay).await;
                let snapshot = host.session.screenshot().await;
                drop(input_turn);
                return match snapshot {
                    Ok(snapshot) => {
                        host.latest_observation_id = Some(snapshot.observation_id.clone());
                        desktop_action_completed_with_snapshot_response(
                            &session_id,
                            action_id,
                            result,
                            snapshot,
                            mode,
                            &mut host.latest_shared_image,
                        )
                    }
                    Err(error) => {
                        let code = error_code(&HostError::ComputerUse(error.clone()));
                        let (mut response, attachment) = action_completed_response(
                            &session_id,
                            action_id,
                            "desktop CUA action completed, but the post-action snapshot failed",
                            result,
                            mode,
                            &mut host.latest_shared_image,
                        )?;
                        response["post_snapshot"] = json!({
                            "success": false,
                            "action_was_executed": true,
                            "code": code,
                            "message": error.message,
                        });
                        response["observation_required"] = Value::Bool(true);
                        Ok((response, attachment))
                    }
                };
            }
            drop(input_turn);
            let (mut response, attachment) = action_completed_response(
                &session_id,
                action_id,
                "desktop CUA action completed",
                result,
                mode,
                &mut host.latest_shared_image,
            )?;
            response["observation_required"] = Value::Bool(true);
            Ok((response, attachment))
        }
        Request::StopDesktopSession { session_id } => {
            let mut host = desktop_sessions
                .remove(&session_id)
                .ok_or_else(|| HostError::Protocol("desktop session not found".into()))?;
            let result = host.session.stop().await?;
            Ok((
                json!({"type":"desktop_session_stopped", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::LaunchApp {
            session_id,
            grant,
            launch,
        } => {
            if session_id.trim().is_empty() {
                return Err(HostError::Protocol(
                    "launch session_id must not be empty".into(),
                ));
            }
            if sessions.contains_key(&session_id) || launch_sessions.contains_key(&session_id) {
                return Err(HostError::Protocol("session already exists".into()));
            }
            ensure_connection_session_capacity(
                sessions.len() + desktop_sessions.len() + launch_sessions.len(),
            )?;
            grant.validate_identity()?;
            if !grant.allow_app_launch {
                return Err(HostError::Protocol(
                    "application launch is not granted".into(),
                ));
            }
            let runtime_session_id = new_runtime_session_id("launch");
            let result = driver
                .launch_app_for_session(&launch, &runtime_session_id)
                .await?;
            let process_id = result["structuredContent"]["pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok());
            if let Some(process_id) = process_id {
                launch_sessions.insert(
                    session_id.clone(),
                    HostLaunchSession {
                        runtime_session_id,
                        task_grant_id: grant.task_grant_id.clone(),
                        application_label: grant.application_label.clone(),
                        process_id,
                    },
                );
            } else {
                let _ = driver.end_launch_session(&runtime_session_id).await;
            }
            Ok((
                json!({
                    "type":"app_launched",
                    "session_id":session_id,
                    "task_grant_id":grant.task_grant_id,
                    "lifecycle_bound":process_id.is_some(),
                    "result":result,
                }),
                None,
            ))
        }
        Request::TerminateApp {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let result = {
                let host =
                    authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                        .await?;
                if !host.allow_app_terminate {
                    return Err(HostError::Protocol(
                        "application termination is not granted".into(),
                    ));
                }
                let result = host.session.terminate_app().await;
                host.finish_observation_sensitive_attempt(result)?
            };
            sessions.remove(&session_id);
            Ok((
                json!({
                    "type":"app_terminated",
                    "session_id":session_id,
                    "result":result,
                }),
                None,
            ))
        }
        Request::ClipboardRead {
            session_id,
            task_grant_id,
            window_capability,
            include_text,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_clipboard_read {
                return Err(HostError::Protocol("clipboard read is not granted".into()));
            }
            let result = host.session.clipboard_read(include_text).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            Ok((
                json!({"type":"clipboard_read", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::ClipboardWrite {
            session_id,
            task_grant_id,
            window_capability,
            write,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_clipboard_write {
                return Err(HostError::Protocol("clipboard write is not granted".into()));
            }
            let result = host.session.clipboard_write(&write).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            Ok((
                json!({"type":"clipboard_written", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::RecordingStart {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_recording {
                return Err(HostError::Protocol("recording is not granted".into()));
            }
            let result = host.session.recording_start(&request).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            Ok((
                json!({"type":"recording_started", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::RecordingStop {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_recording {
                return Err(HostError::Protocol("recording is not granted".into()));
            }
            let result = host.session.recording_stop().await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            Ok((
                json!({"type":"recording_stopped", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::RecordingState {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_recording {
                return Err(HostError::Protocol("recording is not granted".into()));
            }
            let result = host.session.recording_state().await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            Ok((
                json!({"type":"recording_state", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::LiveObservationStart {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_live_observation {
                return Err(HostError::Protocol(
                    "live observation is not granted".into(),
                ));
            }
            let result = host.session.start_live_observation(&request).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            host.invalidate_observations();
            Ok((
                json!({"type":"live_observation_started", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::LiveObservationState {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_live_observation {
                return Err(HostError::Protocol(
                    "live observation is not granted".into(),
                ));
            }
            let result = host.session.live_observation_state();
            Ok((
                json!({"type":"live_observation_state", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::LiveObservationStop {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_live_observation {
                return Err(HostError::Protocol(
                    "live observation is not granted".into(),
                ));
            }
            let result = host.session.stop_live_observation().await;
            host.invalidate_observations();
            Ok((
                json!({"type":"live_observation_stopped", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::OpenSession {
            session_id,
            mut grant,
            activate_before,
            indicator_motion,
            idle_timeout_ms,
        } => {
            if sessions.contains_key(&session_id) {
                return Err(HostError::Protocol("session already exists".into()));
            }
            grant.validate_identity()?;
            if !(MIN_SESSION_IDLE_TIMEOUT_MS..=MAX_SESSION_IDLE_TIMEOUT_MS)
                .contains(&idle_timeout_ms)
            {
                return Err(HostError::Protocol(format!(
                    "idle_timeout_ms must be between {MIN_SESSION_IDLE_TIMEOUT_MS} and {MAX_SESSION_IDLE_TIMEOUT_MS}"
                )));
            }
            let launched = launch_sessions.get(&session_id).cloned();
            if launched.is_none() {
                ensure_connection_session_capacity(
                    sessions.len() + desktop_sessions.len() + launch_sessions.len(),
                )?;
            }
            if let Some(launched) = &launched {
                bind_launched_process(launched, &mut grant)?;
            }
            let allow_restore_activate = restore_activate_available(&grant);
            let scope = ComputerUseTargetScope {
                process_id: grant.process_id,
                window_handle: grant.window_handle,
                window_title: grant.window_title,
            };
            let runtime_session_id = launched
                .as_ref()
                .map(|session| session.runtime_session_id.clone())
                .unwrap_or_else(|| new_runtime_session_id("window"));
            let mut session = driver.session_with_agent(
                scope,
                grant.application_label.clone(),
                agent_name.clone(),
                runtime_session_id.clone(),
            )?;
            let started = session
                .start_with_request(&ComputerUseSessionStartRequest {
                    activate_before,
                    indicator_motion,
                })
                .await?;
            let showcase = if let Some(output_dir) = grant.showcase_output_dir.as_deref() {
                if !grant.allow_recording {
                    let _ = session.stop().await;
                    return Err(HostError::Protocol(
                        "showcase recording requires allow_recording".into(),
                    ));
                }
                match session
                    .recording_start(&ComputerUseRecordingStartRequest {
                        output_dir: output_dir.to_owned(),
                        record_video: true,
                    })
                    .await
                {
                    Ok(result) => Some(result),
                    Err(error) => {
                        let _ = session.stop().await;
                        return Err(error.into());
                    }
                }
            } else {
                None
            };
            launch_sessions.remove(&session_id);
            let Some(target) = session.target() else {
                let _ = session.stop().await;
                return Err(HostError::Protocol("CUA did not return a target".into()));
            };
            let status = session.status();
            let marker = status["marker"].clone();
            let banner = status["banner"].clone();
            let upstream_session = status["upstream_session"].clone();
            let cursor = json!({
                "visible": marker["visible"],
                "shape": "mouse_pointer",
                "theme": started["cursor_theme"],
                "render_backend": cursor_render_backend(
                    driver.upstream_cursor_renderer_enabled(),
                ),
                "motion_backend": "cua-driver-sdk",
            });
            let capability = format!("cua-window-{}", Uuid::new_v4());
            let input_target = match input_target_from_cua(&session_id, &target) {
                Ok(target) => target,
                Err(error) => {
                    let _ = session.stop().await;
                    return Err(error);
                }
            };
            let target_process_id = input_target.process_id;
            let target_window_handle = input_target.window_handle;
            let (input_readiness, observed_at) = crate::session_events::input_readiness_sample();
            let input_events =
                crate::session_events::SessionInputEventQueue::new_with_restore_capability(
                    input_target,
                    input_readiness,
                    dcc_cua_core::ComputerUseTargetAvailability::from_window_state(&target),
                    allow_restore_activate,
                    observed_at,
                );
            let initial_input_state = input_events.current().clone();
            let initial_target_state = input_events.target_state().clone();
            let initial_sequence = input_events.latest_sequence();
            let synchronized_action_evidence_epoch = session.action_evidence_epoch();
            sessions.insert(
                session_id.clone(),
                HostSession {
                    runtime_session_id,
                    target_process_id,
                    target_window_handle,
                    task_grant_id: grant.task_grant_id,
                    allow_raw_input: grant.allow_raw_input,
                    allow_app_terminate: grant.allow_app_terminate,
                    allow_clipboard_read: grant.allow_clipboard_read,
                    allow_clipboard_write: grant.allow_clipboard_write,
                    allow_recording: grant.allow_recording,
                    allow_live_observation: grant.allow_live_observation,
                    allow_browser_input: grant.allow_browser_input,
                    allow_browser_prepare: grant.allow_browser_prepare,
                    allow_browser_download: grant.allow_browser_download,
                    allow_native_tool: grant.allow_native_tool,
                    allow_menu_invoke: grant.allow_menu_invoke,
                    allow_session_escalation: grant.allow_session_escalation,
                    allow_trusted_confirmation: grant.allow_trusted_confirmation,
                    allow_restore_activate,
                    capability: capability.clone(),
                    interrupted: false,
                    session,
                    synchronized_action_evidence_epoch,
                    browser_evidence_epoch: None,
                    browser: BrowserSession::default(),
                    latest_observation_id: None,
                    latest_accessibility_state_id: None,
                    latest_accessibility_root: None,
                    latest_shared_image: None,
                    input_events,
                    idle_timeout: Duration::from_millis(idle_timeout_ms),
                    last_activity: Instant::now(),
                },
            );
            let mut response = json!({
                "type": "session_opened",
                "session_id": session_id,
                "window_capability": capability,
                "target": target_wire(&target),
                "marker": marker,
                "banner": banner,
                "upstream_session": upstream_session,
                "cursor": cursor,
                "showcase": showcase,
                "input_state": initial_input_state,
                "target_state": initial_target_state,
                "latest_sequence": initial_sequence,
                "idle_timeout_ms": idle_timeout_ms,
            });
            if let Some(activation) = started.get("activation") {
                response["activation"] = activation.clone();
            }
            Ok((response, None))
        }
        Request::GetWindowState {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            let state = host.session.window_state().await;
            let state = host.finish_observation_sensitive_attempt(state)?;
            Ok((
                observed_window_state_response(host, &session_id, state),
                None,
            ))
        }
        Request::ChangeWindowState {
            session_id,
            task_grant_id,
            window_capability,
            operation,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            let _input_turn = RAW_INPUT_QUEUE.lock().await;
            let (operation, result) = match operation {
                WindowOperation::Activate => {
                    let result = host.session.activate().await;
                    let result = host.finish_observation_sensitive_attempt(result);
                    let result =
                        finish_window_mutation_attempt(result, || host.invalidate_observations())?;
                    ("activate", result)
                }
                WindowOperation::RestoreActivate => {
                    if !host.allow_restore_activate {
                        return Err(HostError::ComputerUse(ComputerUseError::new(
                            ComputerUseErrorCode::InvalidTarget,
                            "restore_activate requires an exact process_id and window_handle grant binding",
                        )));
                    }
                    let result = host.session.restore_activate().await;
                    let result = host.finish_observation_sensitive_attempt(result);
                    let result =
                        finish_window_mutation_attempt(result, || host.invalidate_observations());
                    let availability = host.session.target_availability().await;
                    if let Ok(availability) =
                        host.finish_observation_sensitive_attempt(availability)
                    {
                        host.observe_target_availability(availability);
                    }
                    ("restore_activate", result?)
                }
                WindowOperation::Close => {
                    if !host.allow_trusted_confirmation {
                        return Err(HostError::ComputerUse(ComputerUseError::new(
                            ComputerUseErrorCode::InvalidAction,
                            "close requires explicit trusted action-time confirmation",
                        )));
                    }
                    let result = host.session.close_window().await;
                    let result = host.finish_observation_sensitive_attempt(result);
                    let result =
                        finish_window_mutation_attempt(result, || host.invalidate_observations())?;
                    return Ok((
                        window_state_changed_response(
                            &session_id,
                            "close",
                            json!({"exists": false}),
                            result,
                        ),
                        None,
                    ));
                }
            };
            let state = host.session.window_state().await;
            let state = host.finish_observation_sensitive_attempt(state)?;
            host.observe_target_state(&state);
            Ok((
                window_state_changed_response(&session_id, operation, state, result),
                None,
            ))
        }
        Request::SetWindowFrame {
            session_id,
            task_grant_id,
            window_capability,
            frame,
        } => {
            frame.validate()?;
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            let _input_turn = RAW_INPUT_QUEUE.lock().await;
            let result = host.session.set_window_frame(&frame).await;
            let result = host.finish_observation_sensitive_attempt(result);
            let result = finish_window_mutation_attempt(result, || host.invalidate_observations())?;
            Ok((
                json!({
                    "type":"window_frame_set",
                    "session_id":session_id,
                    "result":result,
                }),
                None,
            ))
        }
        Request::InvokeMenu {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            request.validate()?;
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_menu_invoke {
                return Err(HostError::Protocol(
                    "native menu invocation is not granted".into(),
                ));
            }
            let _input_turn = RAW_INPUT_QUEUE.lock().await;
            let result = host.session.invoke_menu(&request).await;
            let result = host.finish_observation_sensitive_attempt(result);
            let result = finish_window_mutation_attempt(result, || host.invalidate_observations())?;
            Ok((
                json!({
                    "type":"menu_invoked",
                    "session_id":session_id,
                    "result":result,
                }),
                None,
            ))
        }
        Request::Snapshot {
            session_id,
            task_grant_id,
            window_capability,
            max_depth,
            max_nodes,
            activate_before,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            let activation = if activate_before {
                let activation = host.session.activate().await;
                Some(host.finish_observation_sensitive_attempt(activation)?)
            } else {
                None
            };
            let screenshot = host
                .session
                .screenshot_with_bounds(max_nodes, max_depth)
                .await;
            let screenshot = host.finish_observation_sensitive_attempt(screenshot)?;
            let observation_id = screenshot.observation.observation_id.clone();
            host.latest_observation_id = Some(observation_id.clone());
            host.latest_accessibility_state_id = Some(observation_id.clone());
            let accessibility = screenshot.accessibility;
            host.latest_accessibility_root = Some(accessibility.clone());
            let node_count = accessibility["elements"].as_array().map_or(0, Vec::len);
            let target = json!({
                "process_id": screenshot.observation.process_id,
                "window_handle": screenshot.observation.window_handle,
                "window_title": screenshot.observation.window_title,
            });
            let (image, attachment) = match mode {
                SnapshotTransport::SharedMemory => {
                    let shared = SharedImage::from_bytes(&screenshot.data, "image/png")
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
                    let mut descriptor = serde_json::to_value(shared.descriptor())
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
                    descriptor["encoding"] = Value::String("shared_memory".into());
                    host.latest_shared_image = Some(shared);
                    (descriptor, None)
                }
                SnapshotTransport::BinaryFrame => (
                    json!({
                        "name": "",
                        "id": screenshot.observation.observation_id,
                        "length": screenshot.data.len(),
                        "mime_type": "image/png",
                        "encoding": "binary_frame",
                    }),
                    Some(screenshot.data),
                ),
            };
            let response = json!({
                "type": "snapshot",
                "observation_id": observation_id,
                "accessibility_state_id": screenshot.observation.observation_id,
                "target": target,
                "observation": screenshot.observation,
                "root": accessibility,
                "node_count": node_count,
                "image": image,
                "activation": activation,
            });
            Ok((response, attachment))
        }
        Request::Zoom {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            let observation_id = request.observation_id.clone();
            let result = host.session.zoom(&request).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            let (mut response, attachment) = native_tool_response_with_transport(
                Some(&session_id),
                "zoom",
                result,
                mode,
                &mut host.latest_shared_image,
            )?;
            response["type"] = Value::String("zoom".into());
            response["observation_id"] = Value::String(observation_id);
            Ok((response, attachment))
        }
        Request::AccessibilitySnapshot {
            session_id,
            task_grant_id,
            window_capability,
            max_depth,
            max_nodes,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            let root = host
                .session
                .accessibility_snapshot(max_nodes, max_depth)
                .await;
            let root = host.finish_observation_sensitive_attempt(root)?;
            let observation_id = host
                .session
                .latest_observation_id()
                .ok_or_else(|| {
                    HostError::Protocol("accessibility snapshot returned no observation".into())
                })?
                .to_owned();
            let state_id = observation_id.clone();
            host.latest_observation_id = Some(observation_id.clone());
            host.latest_accessibility_state_id = Some(state_id.clone());
            host.latest_accessibility_root = Some(root.clone());
            let target = host
                .session
                .target()
                .ok_or_else(|| HostError::Protocol("CUA did not return a target".into()))?;
            Ok((
                json!({
                    "type":"accessibility_snapshot",
                    "observation_id":observation_id,
                    "accessibility_state_id":state_id,
                    "target":target_wire(&target),
                    "root":root,
                    "node_count":root["elements"].as_array().map_or(0, Vec::len),
                }),
                None,
            ))
        }
        Request::VerifyState {
            session_id,
            task_grant_id,
            window_capability,
            expect,
            timeout_ms,
            stable_samples,
            include_screenshot,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            let verification = host
                .session
                .verify_state(expect, timeout_ms, stable_samples, include_screenshot)
                .await;
            let verification = host.finish_observation_sensitive_attempt(verification)?;
            let image_transport = verification
                .image
                .map(|image| image_response(image, mode, &mut host.latest_shared_image))
                .transpose()?;
            let mut response = json!({
                "type": "state_verified",
                "session_id": session_id,
                "result": verification.value,
            });
            let attachment = if let Some((descriptor, attachment)) = image_transport {
                response["image"] = descriptor;
                attachment
            } else {
                None
            };
            Ok((response, attachment))
        }
        Request::CallTool {
            session_id,
            task_grant_id,
            window_capability,
            tool,
            arguments,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_native_tool {
                return Err(HostError::Protocol(
                    "native CUA tool calls are not granted".into(),
                ));
            }
            let result = host.session.call_tool(&tool, arguments).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            native_tool_response_with_transport(
                Some(&session_id),
                &tool,
                result,
                mode,
                &mut host.latest_shared_image,
            )
        }
        Request::CallGlobalTool {
            grant,
            tool,
            arguments,
        } => {
            grant.validate_identity()?;
            if !grant.allow_native_tool {
                return Err(HostError::Protocol(
                    "global native CUA tool calls are not granted".into(),
                ));
            }
            let result = driver.call_global_tool(&tool, arguments).await?;
            native_tool_response_with_transport(None, &tool, result, mode, desktop_shared_image)
        }
        Request::BrowserSnapshot {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            let result = host.browser.snapshot(&mut host.session, request).await;
            let publishes_snapshot_evidence = result
                .as_ref()
                .is_ok_and(|result| result.publishes_snapshot_evidence());
            let result =
                host.finish_browser_snapshot_attempt(result, publishes_snapshot_evidence)?;
            browser_response(
                "browser_snapshot",
                session_id,
                result,
                mode,
                &mut host.latest_shared_image,
            )
        }
        Request::BrowserPrepare {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_browser_prepare {
                return Err(HostError::Protocol(
                    "browser preparation is not granted".into(),
                ));
            }
            let result = host.browser.prepare(&mut host.session, request).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            browser_response(
                "browser_prepared",
                session_id,
                result,
                mode,
                &mut host.latest_shared_image,
            )
        }
        Request::BrowserNavigate {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_browser_input {
                return Err(HostError::Protocol("browser input is not granted".into()));
            }
            let result = host.browser.navigate(&mut host.session, request).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            browser_response(
                "browser_navigated",
                session_id,
                result,
                mode,
                &mut host.latest_shared_image,
            )
        }
        Request::BrowserClick {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_browser_input {
                return Err(HostError::Protocol("browser input is not granted".into()));
            }
            host.require_current_browser_evidence_epoch()?;
            let result = host.browser.click(&mut host.session, request).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            browser_response(
                "browser_clicked",
                session_id,
                result,
                mode,
                &mut host.latest_shared_image,
            )
        }
        Request::BrowserType {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_browser_input {
                return Err(HostError::Protocol("browser input is not granted".into()));
            }
            host.require_current_browser_evidence_epoch()?;
            let result = host.browser.type_text(&mut host.session, request).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            browser_response(
                "browser_typed",
                session_id,
                result,
                mode,
                &mut host.latest_shared_image,
            )
        }
        Request::BrowserPointer {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_browser_input {
                return Err(HostError::Protocol("browser input is not granted".into()));
            }
            host.require_current_browser_evidence_epoch()?;
            let result = host.browser.pointer(&mut host.session, request).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            browser_response(
                "browser_pointer_completed",
                session_id,
                result,
                mode,
                &mut host.latest_shared_image,
            )
        }
        Request::BrowserSetInputFiles {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_browser_input {
                return Err(HostError::Protocol("browser input is not granted".into()));
            }
            host.require_current_browser_evidence_epoch()?;
            let result = host
                .browser
                .set_input_files(&mut host.session, request)
                .await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            browser_response(
                "browser_files_uploaded",
                session_id,
                result,
                mode,
                &mut host.latest_shared_image,
            )
        }
        Request::BrowserDownload {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_browser_download {
                return Err(HostError::Protocol(
                    "browser download is not granted".into(),
                ));
            }
            host.require_current_browser_evidence_epoch()?;
            let result = host.browser.download(&mut host.session, request).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            browser_response(
                "browser_downloaded",
                session_id,
                result,
                mode,
                &mut host.latest_shared_image,
            )
        }
        Request::BrowserDialog {
            session_id,
            task_grant_id,
            window_capability,
            request,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_browser_input {
                return Err(HostError::Protocol("browser input is not granted".into()));
            }
            let result = host.browser.dialog(&mut host.session, request).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            browser_response(
                "browser_dialog_completed",
                session_id,
                result,
                mode,
                &mut host.latest_shared_image,
            )
        }
        Request::Find {
            session_id,
            task_grant_id,
            window_capability,
            query,
        } => {
            let max_results = query.validate()?;
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            host.refresh_input_readiness();
            let availability = host.session.target_availability().await;
            finish_target_sensitive_cached_read(host, availability)?;
            let root = host.latest_accessibility_root.clone().ok_or_else(|| {
                HostError::ComputerUse(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "take a snapshot before finding accessibility elements",
                ))
            })?;
            let state_id = host.latest_accessibility_state_id.clone().ok_or_else(|| {
                HostError::ComputerUse(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "accessibility state is unavailable; take a snapshot first",
                ))
            })?;
            let matches = find_elements(&root, &query, max_results);
            let target = host
                .session
                .target()
                .ok_or_else(|| HostError::Protocol("CUA did not return a target".into()))?;
            Ok((
                json!({
                    "type":"find_results",
                    "accessibility_state_id":state_id,
                    "target":target_wire(&target),
                    "matches":matches,
                    "node_count":root["elements"].as_array().map_or(0, Vec::len),
                }),
                None,
            ))
        }
        Request::WaitFor {
            session_id,
            task_grant_id,
            window_capability,
            condition,
        } => {
            let (timeout_ms, interval_ms) = condition.validate()?;
            let cancellation = register_wait(
                cancellation_registry,
                &session_id,
                &task_grant_id,
                &window_capability,
            )?;
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            crate::wait::handle_wait_for(
                host,
                &cancellation.handle,
                &session_id,
                &condition,
                timeout_ms,
                interval_ms,
            )
            .await
        }
        Request::ExecuteAction {
            session_id,
            task_grant_id,
            window_capability,
            observation_id,
            accessibility_state_id,
            action,
            capture_after,
            post_snapshot_delay_ms,
            post_snapshot_max_depth,
            post_snapshot_max_nodes,
        } => {
            let post_snapshot_delay = post_snapshot_delay(capture_after, post_snapshot_delay_ms)?;
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if action.input_kind == "raw_input" && !host.allow_raw_input {
                return Err(HostError::Protocol("raw input is not granted".into()));
            }
            if let Some((code, message)) = action.reject_policy() {
                return Ok((
                    json!({
                        "type":"action_completed",
                        "success":false,
                        "policy_tier":code,
                        "message":message,
                        "error":code,
                    }),
                    None,
                ));
            }
            if host.latest_observation_id.as_deref() != Some(observation_id.as_str()) {
                return Err(HostError::ComputerUse(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "action observation_id does not match the latest host snapshot",
                )));
            }
            if (action.element_index.is_some() || action.element_token.is_some())
                && host.latest_accessibility_state_id.as_deref()
                    != Some(accessibility_state_id.as_str())
            {
                return Err(HostError::ComputerUse(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "semantic action requires the latest accessibility_state_id",
                )));
            }
            if action.requires_approval() {
                let request = TrustedActionConfirmationRequest::for_window_action(
                    &session_id,
                    &task_grant_id,
                    &window_capability,
                    &observation_id,
                    &accessibility_state_id,
                    &action,
                )?;
                let outcome = authorize_action_confirmation(
                    confirmation_host,
                    host.allow_trusted_confirmation,
                    request,
                )
                .await;
                if outcome != ActionConfirmationOutcome::Allowed {
                    return Ok(action_confirmation_refusal(outcome));
                }
            }
            let raw_input = action.input_kind == "raw_input";
            let action = action.into_computer_use(observation_id)?;
            let input_turn = acquire_raw_input_turn(raw_input).await;
            let result = host.session.perform_action(&action).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            let action_id = format!("cua-action-{}", Uuid::new_v4());
            if capture_after {
                tokio::time::sleep(post_snapshot_delay).await;
                let screenshot = host
                    .session
                    .screenshot_with_bounds(post_snapshot_max_nodes, post_snapshot_max_depth)
                    .await;
                let screenshot = host.finish_observation_sensitive_attempt(screenshot);
                drop(input_turn);
                return match screenshot {
                    Ok(screenshot) => {
                        let observation_id = screenshot.observation.observation_id.clone();
                        host.latest_observation_id = Some(observation_id.clone());
                        host.latest_accessibility_state_id = Some(observation_id);
                        host.latest_accessibility_root = Some(screenshot.accessibility.clone());
                        action_completed_with_snapshot_response(
                            &session_id,
                            action_id,
                            result,
                            screenshot,
                            mode,
                            &mut host.latest_shared_image,
                        )
                    }
                    Err(error) => {
                        host.latest_observation_id = None;
                        host.latest_accessibility_state_id = None;
                        host.latest_accessibility_root = None;
                        let code = error_code(&HostError::ComputerUse(error.clone()));
                        let (mut response, attachment) = action_completed_response(
                            &session_id,
                            action_id,
                            "CUA action completed, but the post-action snapshot failed",
                            result,
                            mode,
                            &mut host.latest_shared_image,
                        )?;
                        response["post_snapshot"] = json!({
                            "success": false,
                            "action_was_executed": true,
                            "code": code,
                            "message": error.message,
                        });
                        response["observation_required"] = Value::Bool(true);
                        Ok((response, attachment))
                    }
                };
            }
            drop(input_turn);
            host.latest_observation_id = None;
            host.latest_accessibility_state_id = None;
            host.latest_accessibility_root = None;
            let (mut response, attachment) = action_completed_response(
                &session_id,
                action_id,
                "CUA action completed",
                result,
                mode,
                &mut host.latest_shared_image,
            )?;
            response["observation_required"] = Value::Bool(true);
            Ok((response, attachment))
        }
        Request::ResumeSession {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            host.session.resume_after_user_approval().await?;
            Ok((
                json!({"type":"session_resumed", "session_id":session_id}),
                None,
            ))
        }
        Request::GetSessionState {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            let state = host.session.session_state().await;
            let state = host.finish_observation_sensitive_attempt(state)?;
            Ok((
                json!({"type":"session_state", "session_id":session_id, "state":state}),
                None,
            ))
        }
        Request::GetInputState {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let host =
                session_with_capability(sessions, &session_id, &task_grant_id, &window_capability)?;
            host.refresh_input_readiness();
            let availability = host.session.target_availability().await;
            if let Ok(availability) = host.finish_observation_sensitive_attempt(availability) {
                host.observe_target_availability(availability);
            }
            Ok((
                json!({
                    "type": "input_state",
                    "session_id": session_id,
                    "state": host.input_events.current(),
                    "target_state": host.input_events.target_state(),
                    "latest_sequence": host.input_events.latest_sequence(),
                }),
                None,
            ))
        }
        Request::SessionHealth {
            session_id,
            task_grant_id,
            window_capability,
            policy,
        } => {
            let host =
                session_with_capability(sessions, &session_id, &task_grant_id, &window_capability)?;
            let evidence_epoch_before = host.session.action_evidence_epoch();
            let transition_sequence_before = host.input_events.latest_sequence();
            let mut target_probe_failed = refresh_session_health_input_and_target(host).await;
            let recording = if host.allow_recording {
                let state = host.session.recording_state().await;
                match host.finish_observation_sensitive_attempt(state) {
                    Ok(state) => ComputerUseRecordingHealth::from_state(true, &state, &policy),
                    Err(_) => ComputerUseRecordingHealth::from_state(true, &Value::Null, &policy),
                }
            } else {
                ComputerUseRecordingHealth::from_state(false, &Value::Null, &policy)
            };
            // Close the preflight fence after the recording probe so an input
            // or exact-target transition cannot produce a mixed safe snapshot.
            target_probe_failed |= refresh_session_health_input_and_target(host).await;
            let action_evidence_epoch = host.session.action_evidence_epoch();
            let transition_sequence = host.input_events.latest_sequence();
            let interrupted = host.interrupted
                || host.session.control_banner_interrupted()
                || host.session.control_banner_failure().is_some();
            let health = ComputerUseSessionHealth::evaluate(ComputerUseSessionHealthEvaluation {
                policy,
                input_state: host.input_events.current().clone(),
                target_state: host.input_events.target_state().clone(),
                recording,
                action_evidence_epoch,
                transition_sequence,
                state_changed_during_probe: session_health_state_changed(
                    evidence_epoch_before,
                    transition_sequence_before,
                    action_evidence_epoch,
                    transition_sequence,
                ),
                interrupted,
                target_probe_failed,
            });
            Ok((
                json!({"type":"session_health", "session_id":session_id, "health":health}),
                None,
            ))
        }
        Request::PollSessionEvents {
            session_id,
            task_grant_id,
            window_capability,
            after_sequence,
            timeout_ms,
        } => {
            let timeout = poll_session_events_timeout(timeout_ms)?;
            let deadline = tokio::time::Instant::now() + timeout;
            loop {
                let deadline_reached = tokio::time::Instant::now() >= deadline;
                let mut page = {
                    let host = session_with_capability(
                        sessions,
                        &session_id,
                        &task_grant_id,
                        &window_capability,
                    )?;
                    host.refresh_input_readiness();
                    let availability = host.session.target_availability().await;
                    if let Ok(availability) =
                        host.finish_observation_sensitive_attempt(availability)
                    {
                        host.observe_target_availability(availability);
                    }
                    host.input_events
                        .page_after(after_sequence, deadline_reached)
                };
                if page.resync_required || !page.events.is_empty() {
                    page.timed_out = false;
                }
                if page.timed_out || page.resync_required || !page.events.is_empty() {
                    return Ok((session_events_response(&session_id, page)?, None));
                }
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                tokio::time::sleep(remaining.min(std::time::Duration::from_millis(100))).await;
            }
        }
        Request::CursorTool {
            session_id,
            task_grant_id,
            window_capability,
            tool,
            arguments,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            let result = {
                let _input_turn = RAW_INPUT_QUEUE.lock().await;
                let result = host.session.cursor_tool(&tool, arguments).await;
                host.finish_observation_sensitive_attempt(result)?
            };
            let marker = host.session.status()["marker"].clone();
            Ok((
                json!({"type":"cursor_tool_result", "session_id":session_id, "tool":tool, "result":result, "marker":marker}),
                None,
            ))
        }
        Request::EscalateSession {
            session_id,
            task_grant_id,
            window_capability,
            reason,
            detail,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            if !host.allow_session_escalation {
                return Err(HostError::Protocol(
                    "session escalation is not granted".into(),
                ));
            }
            let result = host.session.escalate(&reason, detail.as_deref()).await;
            let result = host.finish_observation_sensitive_attempt(result)?;
            Ok((
                json!({"type":"session_escalated", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::StopSession { session_id } => {
            let mut host = take_connection_session(sessions, &session_id)?;
            let result = host.session.stop().await?;
            Ok((session_stopped_response(&session_id, result), None))
        }
    }
}
