use super::*;

pub(super) fn bind_launched_process(
    launched: &HostLaunchSession,
    grant: &mut TaskGrant,
) -> Result<(), HostError> {
    if grant.task_grant_id != launched.task_grant_id || grant.dcc_type != launched.dcc_type {
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

pub(super) async fn handle_request(
    driver: &ComputerUseDriver,
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
    let ConnectionSessions {
        windows: sessions,
        desktops: desktop_sessions,
        launches: launch_sessions,
    } = sessions;

    match request {
        Request::Hello(_) => unreachable!(),
        Request::Ping {} => Ok((ping_response(), None)),
        Request::Doctor {} => Ok((driver.diagnostics().await, None)),
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
            if grant.task_grant_id.trim().is_empty() || grant.dcc_type.trim().is_empty() {
                return Err(HostError::Protocol("task grant is incomplete".into()));
            }
            let session_generation = interrupt_generation();
            let runtime_session_id = new_runtime_session_id("desktop");
            let mut session = driver.desktop_session(runtime_session_id.clone())?;
            let started = session.start().await?;
            let capability = format!("cua-desktop-{}", Uuid::new_v4());
            desktop_sessions.insert(
                session_id.clone(),
                HostDesktopSession {
                    runtime_session_id,
                    task_grant_id: grant.task_grant_id,
                    allow_raw_input: grant.allow_raw_input,
                    capability: capability.clone(),
                    interrupt_generation: session_generation,
                    session,
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
                        "id": snapshot.observation_id,
                        "length": snapshot.data.len(),
                        "mime_type": "image/png",
                        "encoding": "binary_frame",
                    }),
                    Some(snapshot.data),
                ),
            };
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
        } => {
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
            if action.requires_approval() {
                return Ok((
                    json!({
                        "type":"action_completed",
                        "success":false,
                        "policy_tier":"action_confirmation",
                        "message":"trusted action-time confirmation is required",
                        "error":"approval_required",
                    }),
                    None,
                ));
            }
            let action = action.into_computer_use(observation_id)?;
            let result = {
                let _input_turn = RAW_INPUT_QUEUE.lock().await;
                host.session.perform_action(&action).await?
            };
            let action_id = format!("cua-desktop-action-{}", Uuid::new_v4());
            if capture_after {
                return match host.session.screenshot().await {
                    Ok(snapshot) => desktop_action_completed_with_snapshot_response(
                        &session_id,
                        action_id,
                        result,
                        snapshot,
                        mode,
                        &mut host.latest_shared_image,
                    ),
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
            action_completed_response(
                &session_id,
                action_id,
                "desktop CUA action completed",
                result,
                mode,
                &mut host.latest_shared_image,
            )
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
            if grant.task_grant_id.trim().is_empty() || grant.dcc_type.trim().is_empty() {
                return Err(HostError::Protocol("task grant is incomplete".into()));
            }
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
                        dcc_type: grant.dcc_type.clone(),
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
                host.session.terminate_app().await?
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
            let result = host.session.clipboard_read(include_text).await?;
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
            let result = host.session.clipboard_write(&write).await?;
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
            let result = host.session.recording_start(&request).await?;
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
            let result = host.session.recording_stop().await?;
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
            let result = host.session.recording_state().await?;
            Ok((
                json!({"type":"recording_state", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::OpenSession {
            session_id,
            mut grant,
        } => {
            if sessions.contains_key(&session_id) {
                return Err(HostError::Protocol("session already exists".into()));
            }
            if grant.task_grant_id.trim().is_empty() || grant.dcc_type.trim().is_empty() {
                return Err(HostError::Protocol("task grant is incomplete".into()));
            }
            let launched = launch_sessions.get(&session_id).cloned();
            if let Some(launched) = &launched {
                bind_launched_process(launched, &mut grant)?;
            }
            let scope = ComputerUseTargetScope {
                process_id: grant.process_id,
                window_handle: grant.window_handle,
                window_title: grant.window_title,
            };
            let runtime_session_id = launched
                .as_ref()
                .map(|session| session.runtime_session_id.clone())
                .unwrap_or_else(|| new_runtime_session_id("window"));
            let mut session =
                driver.session(scope, grant.dcc_type.clone(), runtime_session_id.clone())?;
            let started = session.start().await?;
            launch_sessions.remove(&session_id);
            let Some(target) = session.target() else {
                let _ = session.stop().await;
                return Err(HostError::Protocol("CUA did not return a target".into()));
            };
            let marker = session.status()["marker"].clone();
            let banner_target = (|| {
                Ok(BannerTarget {
                    process_id: target["pid"]
                        .as_u64()
                        .and_then(|value| value.try_into().ok())
                        .ok_or_else(|| {
                            HostError::Protocol("CUA target has an invalid process id".into())
                        })?,
                    window_handle: target["window_id"].as_u64().ok_or_else(|| {
                        HostError::Protocol("CUA target has an invalid window handle".into())
                    })?,
                    label: marker["label"]
                        .as_str()
                        .ok_or_else(|| HostError::Protocol("CUA marker has no label".into()))?
                        .to_owned(),
                })
            })();
            let banner_target = match banner_target {
                Ok(target) => target,
                Err(error) => {
                    let _ = session.stop().await;
                    return Err(error);
                }
            };
            let banner = match ControlBanner::start(banner_target) {
                Ok(banner) => banner,
                Err(error) => {
                    let cleanup = session.stop().await;
                    let cleanup_note = cleanup
                        .err()
                        .map(|cleanup_error| format!("; CUA cleanup also failed: {cleanup_error}"))
                        .unwrap_or_default();
                    return Err(ComputerUseError::new(
                        ComputerUseErrorCode::BackendUnavailable,
                        format!("start visible control banner: {error}{cleanup_note}"),
                    )
                    .into());
                }
            };
            let banner_status = banner.status();
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
            sessions.insert(
                session_id.clone(),
                HostSession {
                    runtime_session_id,
                    task_grant_id: grant.task_grant_id,
                    allow_raw_input: grant.allow_raw_input,
                    allow_app_terminate: grant.allow_app_terminate,
                    allow_clipboard_read: grant.allow_clipboard_read,
                    allow_clipboard_write: grant.allow_clipboard_write,
                    allow_recording: grant.allow_recording,
                    allow_browser_input: grant.allow_browser_input,
                    allow_browser_prepare: grant.allow_browser_prepare,
                    allow_browser_download: grant.allow_browser_download,
                    allow_native_tool: grant.allow_native_tool,
                    allow_menu_invoke: grant.allow_menu_invoke,
                    allow_session_escalation: grant.allow_session_escalation,
                    capability: capability.clone(),
                    session,
                    banner,
                    browser: BrowserSession::default(),
                    latest_observation_id: None,
                    latest_accessibility_state_id: None,
                    latest_accessibility_root: None,
                    latest_shared_image: None,
                },
            );
            Ok((
                json!({
                    "type": "session_opened",
                    "session_id": session_id,
                    "window_capability": capability,
                    "target": target_wire(&target),
                    "marker": marker,
                    "banner": banner_status,
                    "cursor": cursor,
                }),
                None,
            ))
        }
        Request::GetWindowState {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await?;
            let state = host.session.window_state().await?;
            Ok((
                json!({"type":"window_state", "session_id":session_id, "state":state}),
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
            let WindowOperation::Activate = operation;
            host.session.activate().await?;
            let state = host.session.window_state().await?;
            Ok((
                json!({
                    "type":"window_state_changed",
                    "session_id":session_id,
                    "operation":"activate",
                    "state":state,
                }),
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
            host.invalidate_observations();
            let result = host.session.set_window_frame(&frame).await?;
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
            host.invalidate_observations();
            let result = host.session.invoke_menu(&request).await?;
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
                Some(host.session.activate().await?)
            } else {
                None
            };
            let screenshot = host
                .session
                .screenshot_with_bounds(max_nodes, max_depth)
                .await?;
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
            let result = host.session.zoom(&request).await?;
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
                .await?;
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
                .await?;
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
            let result = host.session.call_tool(&tool, arguments).await?;
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
            if grant.task_grant_id.trim().is_empty() || grant.dcc_type.trim().is_empty() {
                return Err(HostError::Protocol("task grant is incomplete".into()));
            }
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
            let result = host.browser.snapshot(&host.session, request).await?;
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
            let result = host.browser.prepare(&host.session, request).await?;
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
            let result = host.browser.navigate(&host.session, request).await?;
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
            let result = host.browser.click(&host.session, request).await?;
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
            let result = host.browser.type_text(&host.session, request).await?;
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
            let result = host.browser.pointer(&host.session, request).await?;
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
            let result = host.browser.set_input_files(&host.session, request).await?;
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
            let result = host.browser.download(&host.session, request).await?;
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
            let result = host.browser.dialog(&host.session, request).await?;
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
            let started = Instant::now();
            loop {
                ensure_session_not_interrupted(host).await?;
                let root = tokio::select! {
                    _ = cancellation.handle.cancelled() => {
                        return Ok((json!({
                            "type":"wait_cancelled",
                            "success":false,
                            "session_id":session_id,
                            "error_code":"cancelled",
                            "elapsed_ms":started.elapsed().as_millis(),
                        }), None));
                    }
                    result = host.session.accessibility_snapshot(5_000, 25) => result?,
                };
                ensure_session_not_interrupted(host).await?;
                if wait_condition_matches(&root, &condition) {
                    return Ok((
                        json!({
                            "type":"wait_completed",
                            "success":true,
                            "session_id":session_id,
                            "condition":condition.kind,
                            "elapsed_ms":started.elapsed().as_millis(),
                        }),
                        None,
                    ));
                }
                if started.elapsed().as_millis() >= u128::from(timeout_ms) {
                    return Ok((
                        json!({
                            "type":"wait_completed",
                            "success":false,
                            "session_id":session_id,
                            "condition":condition.kind,
                            "error_code":"timeout",
                            "elapsed_ms":started.elapsed().as_millis(),
                        }),
                        None,
                    ));
                }
                tokio::select! {
                    _ = cancellation.handle.cancelled() => {
                        return Ok((json!({
                            "type":"wait_cancelled",
                            "success":false,
                            "session_id":session_id,
                            "error_code":"cancelled",
                            "elapsed_ms":started.elapsed().as_millis(),
                        }), None));
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(interval_ms)) => {}
                }
            }
        }
        Request::ExecuteAction {
            session_id,
            task_grant_id,
            window_capability,
            observation_id,
            accessibility_state_id,
            action,
            capture_after,
            post_snapshot_max_depth,
            post_snapshot_max_nodes,
        } => {
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
            if action.requires_approval() {
                return Ok((
                    json!({
                        "type":"action_completed",
                        "success":false,
                        "policy_tier":"action_confirmation",
                        "message":"trusted action-time confirmation is required",
                        "error":"approval_required",
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
            let raw_input = action.input_kind == "raw_input";
            let action = action.into_computer_use(observation_id)?;
            let result = if raw_input {
                let _input_turn = RAW_INPUT_QUEUE.lock().await;
                host.session.perform_action(&action).await?
            } else {
                host.session.perform_action(&action).await?
            };
            let action_id = format!("cua-action-{}", Uuid::new_v4());
            if capture_after {
                return match host
                    .session
                    .screenshot_with_bounds(post_snapshot_max_nodes, post_snapshot_max_depth)
                    .await
                {
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
            host.latest_observation_id = None;
            host.latest_accessibility_state_id = None;
            host.latest_accessibility_root = None;
            action_completed_response(
                &session_id,
                action_id,
                "CUA action completed",
                result,
                mode,
                &mut host.latest_shared_image,
            )
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
            let state = host.session.session_state().await?;
            Ok((
                json!({"type":"session_state", "session_id":session_id, "state":state}),
                None,
            ))
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
            let cursor_position = (tool == "move_cursor").then(|| {
                (
                    arguments["x"].as_f64().unwrap_or_default(),
                    arguments["y"].as_f64().unwrap_or_default(),
                )
            });
            let result = {
                let _input_turn = RAW_INPUT_QUEUE.lock().await;
                host.session.cursor_tool(&tool, arguments).await?
            };
            if let Some((x, y)) = cursor_position {
                host.banner.set_cursor_position(x, y);
            }
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
            let result = host.session.escalate(&reason, detail.as_deref()).await?;
            Ok((
                json!({"type":"session_escalated", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::StopSession { session_id } => {
            let mut host = sessions
                .remove(&session_id)
                .ok_or_else(|| HostError::Protocol("session not found".into()))?;
            let result = host.session.stop().await?;
            Ok((
                json!({"type":"session_stopped", "session_id":session_id, "cleanup_pending":result["cleanup_pending"].as_bool().unwrap_or(false)}),
                None,
            ))
        }
    }
}

pub(super) const fn cursor_render_backend(upstream_cursor_renderer_enabled: bool) -> &'static str {
    if cfg!(windows) {
        "host-native-overlay"
    } else if upstream_cursor_renderer_enabled {
        "cua-driver-sdk"
    } else {
        "unavailable"
    }
}
