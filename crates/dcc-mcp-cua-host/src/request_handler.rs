use super::*;

pub(super) async fn handle_request(
    driver: &ComputerUseDriver,
    sessions: &mut HashMap<String, HostSession>,
    desktop_sessions: &mut HashMap<String, HostDesktopSession>,
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
                "capabilities": HOST_CAPABILITIES,
            }),
            None,
        ));
    }
    let mode = snapshot_transport
        .ok_or_else(|| HostError::Protocol("hello is required before stateful requests".into()))?;

    match request {
        Request::Hello(_) => unreachable!(),
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
                    let descriptor = serde_json::to_value(shared.descriptor())
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
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
            let mut session = driver.desktop_session(session_id.clone())?;
            let started = session.start().await?;
            let capability = format!("cua-desktop-{}", Uuid::new_v4());
            desktop_sessions.insert(
                session_id.clone(),
                HostDesktopSession {
                    task_grant_id: grant.task_grant_id,
                    allow_raw_input: grant.allow_raw_input,
                    capability: capability.clone(),
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
            )?;
            let snapshot = host.session.screenshot().await?;
            let (image, attachment) = match mode {
                SnapshotTransport::SharedMemory => {
                    let shared = SharedImage::from_bytes(&snapshot.data, "image/png")
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
                    let descriptor = serde_json::to_value(shared.descriptor())
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
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
        } => {
            let host = authorized_desktop_session(
                desktop_sessions,
                &session_id,
                &task_grant_id,
                &desktop_capability,
            )?;
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
            let result = host.session.perform_action(&action).await?;
            action_completed_response(
                &session_id,
                format!("cua-desktop-action-{}", Uuid::new_v4()),
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
        Request::LaunchApp { grant, launch } => {
            if grant.task_grant_id.trim().is_empty() || grant.dcc_type.trim().is_empty() {
                return Err(HostError::Protocol("task grant is incomplete".into()));
            }
            if !grant.allow_app_launch {
                return Err(HostError::Protocol(
                    "application launch is not granted".into(),
                ));
            }
            let result = driver.launch_app(&launch).await?;
            Ok((
                json!({
                    "type":"app_launched",
                    "task_grant_id":grant.task_grant_id,
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
                    authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            if !host.allow_recording {
                return Err(HostError::Protocol("recording is not granted".into()));
            }
            let result = host.session.recording_state().await?;
            Ok((
                json!({"type":"recording_state", "session_id":session_id, "result":result}),
                None,
            ))
        }
        Request::OpenSession { session_id, grant } => {
            if sessions.contains_key(&session_id) {
                return Err(HostError::Protocol("session already exists".into()));
            }
            if grant.task_grant_id.trim().is_empty() || grant.dcc_type.trim().is_empty() {
                return Err(HostError::Protocol("task grant is incomplete".into()));
            }
            let scope = ComputerUseTargetScope {
                process_id: grant.process_id,
                window_handle: grant.window_handle,
                window_title: grant.window_title,
            };
            let mut session = driver.session(scope, grant.dcc_type.clone(), session_id.clone())?;
            session.start().await?;
            let target = session
                .target()
                .ok_or_else(|| HostError::Protocol("CUA did not return a target".into()))?;
            let marker = session.status()["marker"].clone();
            let capability = format!("cua-window-{}", Uuid::new_v4());
            sessions.insert(
                session_id.clone(),
                HostSession {
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
                    allow_session_escalation: grant.allow_session_escalation,
                    capability: capability.clone(),
                    session,
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
        Request::Snapshot {
            session_id,
            task_grant_id,
            window_capability,
            max_depth,
            max_nodes,
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                    let descriptor = serde_json::to_value(shared.descriptor())
                        .map_err(|error| HostError::Protocol(error.to_string()))?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            let root = host
                .session
                .accessibility_snapshot(max_nodes, max_depth)
                .await?;
            let state_id = format!("{}-accessibility-{}", session_id, Uuid::new_v4());
            host.latest_accessibility_state_id = Some(state_id.clone());
            host.latest_accessibility_root = Some(root.clone());
            let target = host
                .session
                .target()
                .ok_or_else(|| HostError::Protocol("CUA did not return a target".into()))?;
            Ok((
                json!({
                    "type":"accessibility_snapshot",
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            let started = Instant::now();
            loop {
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
        } => {
            let host =
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
            let action = action.into_computer_use(observation_id)?;
            let result = host.session.perform_action(&action).await?;
            host.latest_observation_id = None;
            host.latest_accessibility_state_id = None;
            host.latest_accessibility_root = None;
            action_completed_response(
                &session_id,
                format!("cua-action-{}", Uuid::new_v4()),
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
            let result = host.session.cursor_tool(&tool, arguments).await?;
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
                authorized_session(sessions, &session_id, &task_grant_id, &window_capability)?;
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
