use super::*;
use crate::endpoint::endpoint_singleton_name;
#[cfg(unix)]
use crate::endpoint::{prepare_unix_endpoint_parent, stale_unix_socket_error};
use crate::request_contract::{
    poll_session_events_timeout, post_snapshot_delay, take_connection_session,
};
use crate::request_handler::acquire_raw_input_turn;
use crate::request_handler::bind_launched_process;
use crate::request_handler::finish_window_mutation_attempt;
use crate::request_handler::session_stopped_response;
use crate::session_events::SessionInputEventQueue;
use dcc_cua_shm::SharedImageReader;
use rstest::rstest;
use serde_json::Value;

async fn handle_request(
    driver: &ComputerUseDriver,
    sessions: &mut ConnectionSessions,
    snapshot_transport: &mut Option<SnapshotTransport>,
    desktop_shared_image: &mut Option<SharedImage>,
    cancellation_registry: &CancellationRegistry,
    request: Request,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    request_handler::handle_request_with_confirmation_host(
        driver,
        None,
        sessions,
        snapshot_transport,
        desktop_shared_image,
        cancellation_registry,
        request,
    )
    .await
}

#[rstest]
fn cursor_render_backend_matches_the_native_platform_owner() {
    let enabled = cfg!(any(windows, target_os = "linux"));
    let expected = if enabled {
        "cua-driver-sdk"
    } else {
        "unavailable"
    };
    assert_eq!(request_contract::cursor_render_backend(enabled), expected);
}

#[rstest]
fn capabilities_follow_the_selected_cursor_runtime() {
    let cursor_available = cfg!(any(windows, target_os = "linux"));
    let capabilities = host_capabilities(cursor_available);
    assert!(capabilities.contains(&"scoped_window_frame"));
    assert!(capabilities.contains(&"open_session_activate_before"));
    assert!(capabilities.contains(&"native_menu_path"));
    assert!(capabilities.contains(&"host_wide_interrupt"));
    assert!(capabilities.contains(&"isolated_runtime_sessions"));
    assert!(capabilities.contains(&"multi_agent_sessions"));
    assert!(capabilities.contains(&"indicator_motion_policy"));
    assert!(capabilities.contains(&"nearest_ancestor_role_v1"));
    assert_eq!(capabilities.contains(&"cursor_controls"), cursor_available);
    assert_eq!(
        capabilities.contains(&"cua_cursor_marker"),
        cursor_available
    );
    assert_eq!(
        capabilities.contains(&"windows_background_uia_fallback"),
        cfg!(windows)
    );
    assert_eq!(
        capabilities.contains(&"input_backend:windows.send_input.v1"),
        cfg!(windows)
    );
    assert_eq!(
        capabilities.contains(&"input_backend:windows.send_input.relative_drag.v1"),
        cfg!(windows)
    );
    assert_eq!(
        capabilities.contains(&"input_backend:windows.send_input.combined_down_drag.v1"),
        cfg!(windows)
    );
    assert_eq!(
        capabilities.contains(&"input_backend:windows.synthetic_touch.v1"),
        cfg!(windows)
    );
    assert!(capabilities.contains(&"live_observation_latest_frame"));
    assert!(capabilities.contains(&"session_input_state_events"));
    assert!(capabilities.contains(&"session_target_state_events"));
    assert!(capabilities.contains(&"session_health"));
    assert_eq!(
        capabilities.contains(&"exact_window_restore_activate"),
        cfg!(windows)
    );
}

#[rstest]
fn get_input_state_is_a_typed_session_scoped_request() {
    let request = serde_json::from_value::<Request>(json!({
        "method": "get_input_state",
        "params": {
            "session_id": "session-1",
            "task_grant_id": "grant-1",
            "window_capability": "capability-1"
        }
    }))
    .unwrap();

    assert!(matches!(
        request,
        Request::GetInputState {
            session_id,
            task_grant_id,
            window_capability,
        } if session_id == "session-1"
            && task_grant_id == "grant-1"
            && window_capability == "capability-1"
    ));
}

#[rstest]
fn poll_session_events_is_a_bounded_session_scoped_long_poll() {
    let request = serde_json::from_value::<Request>(json!({
        "method": "poll_session_events",
        "params": {
            "session_id": "session-1",
            "task_grant_id": "grant-1",
            "window_capability": "capability-1",
            "after_sequence": 9,
            "timeout_ms": 250
        }
    }))
    .unwrap();

    assert!(matches!(
        request,
        Request::PollSessionEvents {
            session_id,
            after_sequence: 9,
            timeout_ms: 250,
            ..
        } if session_id == "session-1"
    ));
    assert!(poll_session_events_timeout(MAX_SESSION_EVENT_POLL_TIMEOUT_MS + 1).is_err());
    assert_eq!(poll_session_events_timeout(250).unwrap().as_millis(), 250);
}

#[rstest]
fn stop_session_ownership_removes_its_event_subscription() {
    let mut subscriptions = HashMap::new();
    subscriptions.insert(
        "session-1".to_owned(),
        SessionInputEventQueue::new_with_restore_capability(
            dcc_cua_core::ComputerUseInputTarget {
                session_id: "session-1".into(),
                process_id: 42,
                window_handle: 77,
            },
            dcc_cua_core::ComputerUseInputReadiness {
                status: dcc_cua_core::ComputerUseInputStatus::Ready,
                code: "interactive_desktop_ready".into(),
                reason: None,
            },
            dcc_cua_core::ComputerUseTargetAvailability {
                status: dcc_cua_core::ComputerUseTargetStatus::Available,
                code: "target_available".into(),
                visible: true,
                minimized: false,
                foreground: true,
            },
            true,
            100,
        ),
    );

    let removed = take_connection_session(&mut subscriptions, "session-1").unwrap();

    assert!(!subscriptions.contains_key("session-1"));
    assert_eq!(removed.current().target.session_id, "session-1");
    assert!(take_connection_session(&mut subscriptions, "session-1").is_err());
}

fn target_availability(
    status: dcc_cua_core::ComputerUseTargetStatus,
) -> dcc_cua_core::ComputerUseTargetAvailability {
    dcc_cua_core::ComputerUseTargetAvailability {
        status,
        code: match status {
            dcc_cua_core::ComputerUseTargetStatus::Available => "target_available",
            dcc_cua_core::ComputerUseTargetStatus::Minimized => "target_minimized",
            dcc_cua_core::ComputerUseTargetStatus::Unavailable => "target_unavailable",
        }
        .into(),
        visible: status == dcc_cua_core::ComputerUseTargetStatus::Available,
        minimized: status == dcc_cua_core::ComputerUseTargetStatus::Minimized,
        foreground: status == dcc_cua_core::ComputerUseTargetStatus::Available,
    }
}

fn cached_host_session(driver: &ComputerUseDriver) -> HostSession {
    let session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(42),
                window_handle: Some(77),
                window_title: None,
            },
            "Test DCC",
            "runtime-session-1",
        )
        .unwrap();
    let synchronized_action_evidence_epoch = session.action_evidence_epoch();
    HostSession {
        runtime_session_id: "runtime-session-1".into(),
        target_process_id: 42,
        target_window_handle: 77,
        task_grant_id: "grant-1".into(),
        allow_raw_input: true,
        allow_app_terminate: false,
        allow_clipboard_read: false,
        allow_clipboard_write: false,
        allow_recording: false,
        allow_live_observation: false,
        allow_browser_input: false,
        allow_browser_prepare: false,
        allow_browser_download: false,
        allow_native_tool: false,
        allow_menu_invoke: false,
        allow_session_escalation: false,
        allow_trusted_confirmation: false,
        allow_restore_activate: cfg!(windows),
        capability: "capability-1".into(),
        interrupted: false,
        session,
        synchronized_action_evidence_epoch,
        browser_evidence_epoch: Some(synchronized_action_evidence_epoch),
        browser: BrowserSession::default(),
        latest_observation_id: Some("observation-before-transition".into()),
        latest_accessibility_state_id: Some("accessibility-before-transition".into()),
        latest_accessibility_root: Some(json!({"elements": [{"element_token": "old-token"}]})),
        latest_shared_image: Some(SharedImage::from_bytes(b"old", "image/png").unwrap()),
        input_events: SessionInputEventQueue::new_with_restore_capability(
            dcc_cua_core::ComputerUseInputTarget {
                session_id: "session-1".into(),
                process_id: 42,
                window_handle: 77,
            },
            dcc_cua_core::ComputerUseInputReadiness {
                status: dcc_cua_core::ComputerUseInputStatus::Ready,
                code: "interactive_desktop_ready".into(),
                reason: None,
            },
            target_availability(dcc_cua_core::ComputerUseTargetStatus::Available),
            cfg!(windows),
            100,
        ),
        idle_timeout: Duration::from_millis(DEFAULT_SESSION_IDLE_TIMEOUT_MS),
        last_activity: Instant::now(),
    }
}

#[rstest]
#[tokio::test]
async fn authorized_session_renews_logical_task_activity() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = HashMap::new();
    let mut host = cached_host_session(&driver);
    host.idle_timeout = Duration::from_secs(60);
    host.last_activity = Instant::now() - Duration::from_secs(30);
    let previous_activity = host.last_activity;
    sessions.insert("task-1".into(), host);

    let session = authorized_session(&mut sessions, "task-1", "grant-1", "capability-1")
        .await
        .unwrap();
    assert!(session.last_activity > previous_activity);
}

#[rstest]
fn completed_request_renews_activity_after_long_running_work() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = ConnectionSessions::default();
    let mut host = cached_host_session(&driver);
    host.last_activity = Instant::now() - Duration::from_secs(30);
    let previous_activity = host.last_activity;
    sessions.windows.insert("task-1".into(), host);

    request_handler::finish_window_evidence_request(
        &mut sessions,
        Some(request_handler::WindowEvidenceEpochRoute {
            session_id: "task-1".into(),
            publication: HostEvidencePublication::None,
        }),
        Ok::<_, HostError>(()),
    )
    .unwrap();

    assert!(sessions.windows["task-1"].last_activity > previous_activity);
}

#[rstest]
#[tokio::test]
async fn expired_logical_task_session_is_stopped_and_rejected() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = HashMap::new();
    let mut host = cached_host_session(&driver);
    host.idle_timeout = Duration::from_millis(1);
    host.last_activity = Instant::now() - Duration::from_secs(1);
    sessions.insert("task-1".into(), host);

    let error = match authorized_session(&mut sessions, "task-1", "grant-1", "capability-1").await {
        Ok(_) => panic!("expired session was unexpectedly authorized"),
        Err(error) => error,
    };
    let HostError::ComputerUse(error) = error else {
        panic!("expected typed session expiry error")
    };
    assert_eq!(error.code, ComputerUseErrorCode::SessionRefreshRequired);
    assert!(error.message.contains("idle timeout"));
    assert!(sessions["task-1"].interrupted);
}

#[rstest]
#[tokio::test]
async fn idle_reaper_removes_expired_sessions_only() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = HashMap::new();
    let mut expired = cached_host_session(&driver);
    expired.idle_timeout = Duration::from_millis(1);
    expired.last_activity = Instant::now() - Duration::from_secs(1);
    sessions.insert("expired".into(), expired);
    sessions.insert("active".into(), cached_host_session(&driver));

    reap_idle_window_sessions(&mut sessions).await;

    assert!(!sessions.contains_key("expired"));
    assert!(sessions.contains_key("active"));
}

fn assert_stale_observation(error: HostError, message_fragment: &str) {
    let HostError::ComputerUse(error) = error else {
        panic!("expected typed Computer Use error, got {error}");
    };
    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
    assert!(
        error.message.contains(message_fragment),
        "{}",
        error.message
    );
}

async fn assert_cached_action_references_are_stale(
    driver: &ComputerUseDriver,
    sessions: &mut ConnectionSessions,
    fresh_observation_id: &str,
) {
    let mut snapshot_transport = Some(SnapshotTransport::BinaryFrame);
    let mut desktop_shared_image = None;
    let cancellation_registry = Arc::new(Mutex::new(HashMap::new()));
    let raw_error = handle_request(
        driver,
        sessions,
        &mut snapshot_transport,
        &mut desktop_shared_image,
        &cancellation_registry,
        serde_json::from_value(json!({
            "method": "execute_action",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "grant-1",
                "window_capability": "capability-1",
                "observation_id": "observation-before-transition",
                "accessibility_state_id": "accessibility-before-transition",
                "action": {
                    "action": "click",
                    "input_kind": "raw_input",
                    "intent": "ordinary_edit",
                    "x": 10,
                    "y": 20
                }
            }
        }))
        .unwrap(),
    )
    .await
    .unwrap_err();
    assert_stale_observation(raw_error, "latest host snapshot");

    sessions
        .windows
        .get_mut("session-1")
        .unwrap()
        .latest_observation_id = Some(fresh_observation_id.into());
    let semantic_error = handle_request(
        driver,
        sessions,
        &mut snapshot_transport,
        &mut desktop_shared_image,
        &cancellation_registry,
        serde_json::from_value(json!({
            "method": "execute_action",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "grant-1",
                "window_capability": "capability-1",
                "observation_id": fresh_observation_id,
                "accessibility_state_id": "accessibility-before-transition",
                "action": {
                    "action": "click",
                    "input_kind": "semantic",
                    "intent": "ordinary_edit",
                    "element_token": "old-token"
                }
            }
        }))
        .unwrap(),
    )
    .await
    .unwrap_err();
    assert_stale_observation(semantic_error, "latest accessibility_state_id");
}

#[rstest]
#[tokio::test]
async fn material_target_transition_invalidates_old_raw_and_accessibility_references() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = ConnectionSessions::default();
    sessions
        .windows
        .insert("session-1".into(), cached_host_session(&driver));

    let host = sessions.windows.get_mut("session-1").unwrap();
    assert!(host.observe_target_availability(target_availability(
        dcc_cua_core::ComputerUseTargetStatus::Minimized
    )));
    assert!(host.observe_target_availability(target_availability(
        dcc_cua_core::ComputerUseTargetStatus::Available
    )));
    assert!(host.latest_shared_image.is_some());
    assert_cached_action_references_are_stale(&driver, &mut sessions, "observation-after-restore")
        .await;
}

#[rstest]
#[tokio::test]
async fn get_window_state_material_transition_invalidates_old_raw_and_accessibility_references() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = ConnectionSessions::default();
    sessions
        .windows
        .insert("session-1".into(), cached_host_session(&driver));

    let host = sessions.windows.get_mut("session-1").unwrap();
    let minimized = request_handler::observed_window_state_response(
        host,
        "session-1",
        json!({
            "process_id": 42,
            "window_handle": 77,
            "exists": true,
            "visible": false,
            "minimized": true,
            "foreground": false,
            "bounds": [-32000, -32000, 800, 600]
        }),
    );
    assert_eq!(minimized["type"], "window_state");
    assert_eq!(minimized["state"]["minimized"], true);
    let restored = request_handler::observed_window_state_response(
        host,
        "session-1",
        json!({
            "process_id": 42,
            "window_handle": 77,
            "exists": true,
            "visible": true,
            "minimized": false,
            "foreground": true,
            "bounds": [100, 100, 800, 600]
        }),
    );
    assert_eq!(restored["state"]["minimized"], false);
    assert!(host.latest_shared_image.is_some());

    assert_cached_action_references_are_stale(
        &driver,
        &mut sessions,
        "observation-after-get-window-state",
    )
    .await;
}

#[rstest]
#[tokio::test]
async fn minimized_action_rejection_then_restore_keeps_old_raw_and_accessibility_references_stale()
{
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = ConnectionSessions::default();
    sessions
        .windows
        .insert("session-1".into(), cached_host_session(&driver));

    let host = sessions.windows.get_mut("session-1").unwrap();
    let rejection = host
        .finish_observation_sensitive_attempt::<()>(Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetMinimized,
            "the action observed the exact target minimized",
        )))
        .unwrap_err();
    assert_eq!(rejection.code, ComputerUseErrorCode::TargetMinimized);
    let restored = request_handler::observed_window_state_response(
        host,
        "session-1",
        json!({
            "process_id": 42,
            "window_handle": 77,
            "exists": true,
            "visible": true,
            "minimized": false,
            "foreground": true,
            "bounds": [100, 100, 800, 600]
        }),
    );
    assert_eq!(restored["state"]["minimized"], false);

    assert_cached_action_references_are_stale(
        &driver,
        &mut sessions,
        "observation-after-action-target-rejection",
    )
    .await;
}

#[rstest]
#[tokio::test]
async fn unavailable_snapshot_rejection_then_restore_keeps_old_observation_references_stale() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = ConnectionSessions::default();
    sessions
        .windows
        .insert("session-1".into(), cached_host_session(&driver));

    let host = sessions.windows.get_mut("session-1").unwrap();
    let rejection = host
        .finish_observation_sensitive_attempt::<ComputerUseScreenshot>(Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetUnavailable,
            "the snapshot observed the exact target unavailable",
        )))
        .unwrap_err();
    assert_eq!(rejection.code, ComputerUseErrorCode::TargetUnavailable);
    request_handler::observed_window_state_response(
        host,
        "session-1",
        json!({
            "process_id": 42,
            "window_handle": 77,
            "exists": true,
            "visible": true,
            "minimized": false,
            "foreground": true,
            "bounds": [100, 100, 800, 600]
        }),
    );

    assert_cached_action_references_are_stale(
        &driver,
        &mut sessions,
        "observation-after-snapshot-target-rejection",
    )
    .await;
}

#[rstest]
#[tokio::test]
async fn locked_action_rejection_then_resume_keeps_old_observation_references_stale() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = ConnectionSessions::default();
    sessions
        .windows
        .insert("session-1".into(), cached_host_session(&driver));

    let host = sessions.windows.get_mut("session-1").unwrap();
    let rejection = host
        .finish_observation_sensitive_attempt::<()>(Err(ComputerUseError::new(
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
            "the action observed a locked interactive desktop",
        )))
        .unwrap_err();
    assert_eq!(
        rejection.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert_eq!(
        host.input_events.current().status,
        dcc_cua_core::ComputerUseInputStatus::Suspended
    );
    host.observe_input_readiness(
        dcc_cua_core::ComputerUseInputReadiness {
            status: dcc_cua_core::ComputerUseInputStatus::Ready,
            code: "interactive_desktop_ready".into(),
            reason: None,
        },
        400,
    );

    assert_cached_action_references_are_stale(
        &driver,
        &mut sessions,
        "observation-after-action-input-rejection",
    )
    .await;
}

#[rstest]
fn invalid_target_rejection_invalidates_evidence_without_synthesizing_target_unavailable() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut host = cached_host_session(&driver);

    let rejection = host
        .finish_observation_sensitive_attempt::<()>(Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTarget,
            "the requested native tool is not bindable to this session",
        )))
        .unwrap_err();

    assert_eq!(rejection.code, ComputerUseErrorCode::InvalidTarget);
    assert!(host.latest_observation_id.is_none());
    assert!(host.latest_accessibility_state_id.is_none());
    assert!(host.latest_shared_image.is_some());
    assert_eq!(
        host.input_events.target_state().status,
        dcc_cua_core::ComputerUseTargetStatus::Available
    );
    assert_eq!(host.input_events.latest_sequence(), 1);
    assert!(host.input_events.events_after(1).is_empty());
}

#[rstest]
fn session_refresh_required_invalidates_evidence_without_synthesizing_availability_events() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut host = cached_host_session(&driver);

    let rejection = host
        .finish_observation_sensitive_attempt::<()>(Err(ComputerUseError::new(
            ComputerUseErrorCode::SessionRefreshRequired,
            "session_refresh_required: action_attempted=false",
        )))
        .unwrap_err();

    assert_eq!(rejection.code, ComputerUseErrorCode::SessionRefreshRequired);
    assert!(host.latest_observation_id.is_none());
    assert!(host.latest_accessibility_state_id.is_none());
    assert!(host.latest_accessibility_root.is_none());
    assert!(host.latest_shared_image.is_some());
    assert_eq!(
        host.input_events.current().status,
        dcc_cua_core::ComputerUseInputStatus::Ready
    );
    assert_eq!(
        host.input_events.target_state().status,
        dcc_cua_core::ComputerUseTargetStatus::Available
    );
    assert_eq!(host.input_events.latest_sequence(), 1);
    assert!(host.input_events.events_after(1).is_empty());
}

#[rstest]
fn successful_core_refresh_epoch_reconciles_all_host_observation_caches() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut host = cached_host_session(&driver);
    let previous_epoch = host.session.action_evidence_epoch();

    host.session.invalidate_action_observations();
    host.finish_observation_sensitive_attempt(Ok(())).unwrap();

    assert!(host.session.action_evidence_epoch() > previous_epoch);
    assert!(host.latest_observation_id.is_none());
    assert!(host.latest_accessibility_state_id.is_none());
    assert!(host.latest_accessibility_root.is_none());
    assert!(host.latest_shared_image.is_some());
}

#[rstest]
fn successful_get_session_state_route_reconciles_epoch_at_the_public_request_boundary() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = ConnectionSessions::default();
    sessions
        .windows
        .insert("session-1".into(), cached_host_session(&driver));
    let request = Request::GetSessionState {
        session_id: "session-1".into(),
        task_grant_id: "grant-1".into(),
        window_capability: "capability-1".into(),
    };
    let route = request_handler::window_evidence_epoch_route(&request);
    sessions
        .windows
        .get_mut("session-1")
        .unwrap()
        .session
        .invalidate_action_observations();

    request_handler::finish_window_evidence_request(
        &mut sessions,
        route,
        Ok((json!({"type": "session_state"}), None::<Vec<u8>>)),
    )
    .unwrap();

    let host = sessions.windows.get("session-1").unwrap();
    assert!(host.latest_observation_id.is_none());
    assert!(host.latest_accessibility_state_id.is_none());
    assert!(host.latest_accessibility_root.is_none());
    assert!(host.latest_shared_image.is_some());
}

#[rstest]
fn successful_live_observation_start_reconciles_cross_domain_evidence_at_the_public_boundary() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = ConnectionSessions::default();
    sessions
        .windows
        .insert("session-1".into(), cached_host_session(&driver));
    let request: Request = serde_json::from_value(json!({
        "method": "live_observation_start",
        "params": {
            "session_id": "session-1",
            "task_grant_id": "grant-1",
            "window_capability": "capability-1",
            "request": {}
        }
    }))
    .unwrap();
    let route = request_handler::window_evidence_epoch_route(&request);
    sessions
        .windows
        .get_mut("session-1")
        .unwrap()
        .session
        .invalidate_action_observations();

    request_handler::finish_window_evidence_request(
        &mut sessions,
        route,
        Ok((json!({"type": "live_observation_started"}), None::<Vec<u8>>)),
    )
    .unwrap();

    let host = &sessions.windows["session-1"];
    assert!(host.latest_observation_id.is_none());
    assert!(host.latest_accessibility_state_id.is_none());
    assert!(host.latest_accessibility_root.is_none());
    assert!(host.latest_shared_image.is_some());
    assert!(host.browser_evidence_epoch.is_none());
}

#[rstest]
#[tokio::test]
async fn successful_session_state_refresh_rejects_previous_raw_uia_and_browser_evidence() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = ConnectionSessions::default();
    let mut host = cached_host_session(&driver);
    host.allow_browser_input = true;
    host.browser_evidence_epoch = Some(host.session.action_evidence_epoch());
    sessions.windows.insert("session-1".into(), host);
    let request = Request::GetSessionState {
        session_id: "session-1".into(),
        task_grant_id: "grant-1".into(),
        window_capability: "capability-1".into(),
    };
    let route = request_handler::window_evidence_epoch_route(&request);
    sessions
        .windows
        .get_mut("session-1")
        .unwrap()
        .session
        .invalidate_action_observations();
    request_handler::finish_window_evidence_request(
        &mut sessions,
        route,
        Ok((json!({"type": "session_state"}), None::<Vec<u8>>)),
    )
    .unwrap();

    assert!(sessions.windows["session-1"].latest_shared_image.is_some());
    assert_cached_action_references_are_stale(
        &driver,
        &mut sessions,
        "observation-after-session-refresh",
    )
    .await;

    let mut snapshot_transport = Some(SnapshotTransport::BinaryFrame);
    let mut desktop_shared_image = None;
    let cancellation_registry = Arc::new(Mutex::new(HashMap::new()));
    let error = handle_request(
        &driver,
        &mut sessions,
        &mut snapshot_transport,
        &mut desktop_shared_image,
        &cancellation_registry,
        serde_json::from_value(json!({
            "method": "browser_click",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "grant-1",
                "window_capability": "capability-1",
                "request": {
                    "target_id": "browser-target",
                    "tab_id": "tab-1",
                    "snapshot_id": "p1",
                    "ref": "button-1"
                }
            }
        }))
        .unwrap(),
    )
    .await
    .unwrap_err();

    assert_stale_observation(
        error,
        "fresh browser snapshot after action evidence changed",
    );
}

#[rstest]
fn successful_browser_snapshot_mint_binds_the_new_epoch_without_reusing_native_evidence() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut host = cached_host_session(&driver);
    let previous_epoch = host.session.action_evidence_epoch();

    host.session.invalidate_action_observations();
    let plan = session_state::evidence_epoch_sync_plan(
        host.synchronized_action_evidence_epoch,
        host.session.action_evidence_epoch(),
        HostEvidencePublication::BrowserSnapshot,
    );
    assert!(plan.epoch_changed);
    assert!(!plan.invalidate_browser_snapshot);
    assert!(plan.bind_browser_snapshot);
    host.finish_browser_snapshot_attempt(Ok(()), true).unwrap();

    let current_epoch = host.session.action_evidence_epoch();
    assert!(current_epoch > previous_epoch);
    assert_eq!(host.synchronized_action_evidence_epoch, current_epoch);
    assert_eq!(host.browser_evidence_epoch, Some(current_epoch));
    assert!(host.latest_observation_id.is_none());
    assert!(host.latest_accessibility_state_id.is_none());
    assert!(host.latest_accessibility_root.is_none());
    assert!(host.latest_shared_image.is_some());
}

#[rstest]
fn structured_browser_refusal_does_not_publish_host_snapshot_evidence() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut host = cached_host_session(&driver);
    host.finish_browser_snapshot_attempt(Ok(()), false).unwrap();
    assert!(host.browser_evidence_epoch.is_none());

    let request: Request = serde_json::from_value(json!({
        "method": "browser_snapshot",
        "params": {
            "session_id": "session-1",
            "task_grant_id": "grant-1",
            "window_capability": "capability-1",
            "request": {
                "target_id": "target-1",
                "tab_id": "tab-1",
                "snapshot_format": "semantic_v2"
            }
        }
    }))
    .unwrap();
    let route = request_handler::window_evidence_epoch_route(&request);
    let mut sessions = ConnectionSessions::default();
    sessions.windows.insert("session-1".into(), host);
    request_handler::finish_window_evidence_request(
        &mut sessions,
        route,
        Ok((json!({"type": "browser_snapshot"}), None::<Vec<u8>>)),
    )
    .unwrap();
    assert!(
        sessions.windows["session-1"]
            .browser_evidence_epoch
            .is_none()
    );
}

#[rstest]
fn successful_native_snapshot_mint_survives_outer_epoch_reconciliation() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut host = cached_host_session(&driver);
    host.session.invalidate_action_observations();
    host.finish_observation_sensitive_attempt(Ok(())).unwrap();
    host.latest_observation_id = Some("observation-after-refresh".into());
    host.latest_accessibility_state_id = Some("accessibility-after-refresh".into());
    host.latest_accessibility_root = Some(json!({"elements": [{"name": "new"}]}));
    host.latest_shared_image = Some(SharedImage::from_bytes(b"new", "image/png").unwrap());
    let mut sessions = ConnectionSessions::default();
    sessions.windows.insert("session-1".into(), host);
    let route = request_handler::window_evidence_epoch_route(&Request::Snapshot {
        session_id: "session-1".into(),
        task_grant_id: "grant-1".into(),
        window_capability: "capability-1".into(),
        max_depth: 5,
        max_nodes: 100,
        activate_before: false,
    });

    request_handler::finish_window_evidence_request(
        &mut sessions,
        route,
        Ok((json!({"type": "snapshot"}), None::<Vec<u8>>)),
    )
    .unwrap();

    let host = &sessions.windows["session-1"];
    assert_eq!(
        host.latest_observation_id.as_deref(),
        Some("observation-after-refresh")
    );
    assert_eq!(
        host.latest_accessibility_state_id.as_deref(),
        Some("accessibility-after-refresh")
    );
    assert!(host.latest_accessibility_root.is_some());
    assert!(host.latest_shared_image.is_some());
    assert!(host.browser_evidence_epoch.is_none());
}

#[rstest]
fn action_capture_after_discards_old_browser_evidence_and_keeps_new_native_evidence() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut host = cached_host_session(&driver);
    assert!(host.browser_evidence_epoch.is_some());
    host.session.invalidate_action_observations();
    host.finish_observation_sensitive_attempt(Ok(())).unwrap();
    host.latest_observation_id = Some("post-action-observation".into());
    host.latest_accessibility_state_id = Some("post-action-accessibility".into());
    host.latest_accessibility_root = Some(json!({"elements": [{"name": "post-action"}]}));
    host.latest_shared_image = Some(SharedImage::from_bytes(b"post-action", "image/png").unwrap());
    let mut sessions = ConnectionSessions::default();
    sessions.windows.insert("session-1".into(), host);
    let request: Request = serde_json::from_value(json!({
        "method": "execute_action",
        "params": {
            "session_id": "session-1",
            "task_grant_id": "grant-1",
            "window_capability": "capability-1",
            "observation_id": "observation-before-transition",
            "accessibility_state_id": "accessibility-before-transition",
            "capture_after": true,
            "action": {
                "action": "click",
                "input_kind": "raw_input",
                "intent": "ordinary_edit",
                "x": 10,
                "y": 20
            }
        }
    }))
    .unwrap();
    let route = request_handler::window_evidence_epoch_route(&request);

    request_handler::finish_window_evidence_request(
        &mut sessions,
        route,
        Ok((json!({"type": "action_completed"}), None::<Vec<u8>>)),
    )
    .unwrap();

    let host = &sessions.windows["session-1"];
    assert!(host.browser_evidence_epoch.is_none());
    assert_eq!(
        host.latest_observation_id.as_deref(),
        Some("post-action-observation")
    );
    assert_eq!(
        host.latest_accessibility_state_id.as_deref(),
        Some("post-action-accessibility")
    );
    assert!(host.latest_accessibility_root.is_some());
    assert!(host.latest_shared_image.is_some());
}

#[rstest]
fn post_preflight_action_failure_reconciles_epoch_before_error_return() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut host = cached_host_session(&driver);
    host.session.invalidate_action_observations();
    let mut sessions = ConnectionSessions::default();
    sessions.windows.insert("session-1".into(), host);
    let request: Request = serde_json::from_value(json!({
        "method": "execute_action",
        "params": {
            "session_id": "session-1",
            "task_grant_id": "grant-1",
            "window_capability": "capability-1",
            "observation_id": "observation-before-transition",
            "accessibility_state_id": "accessibility-before-transition",
            "action": {
                "action": "click",
                "input_kind": "raw_input",
                "intent": "ordinary_edit",
                "x": 10,
                "y": 20
            }
        }
    }))
    .unwrap();
    let route = request_handler::window_evidence_epoch_route(&request);

    let error = request_handler::finish_window_evidence_request::<(Value, Option<Vec<u8>>)>(
        &mut sessions,
        route,
        Err(HostError::ComputerUse(ComputerUseError::new(
            ComputerUseErrorCode::InputFailed,
            "local mutation was attempted before failing",
        ))),
    )
    .unwrap_err();

    assert!(matches!(error, HostError::ComputerUse(_)));
    let host = &sessions.windows["session-1"];
    assert!(host.latest_observation_id.is_none());
    assert!(host.latest_accessibility_state_id.is_none());
    assert!(host.latest_accessibility_root.is_none());
    assert!(host.latest_shared_image.is_some());
    assert!(host.browser_evidence_epoch.is_none());
}

#[rstest]
fn pre_dispatch_not_started_failure_preserves_current_cross_domain_evidence() {
    let driver = ComputerUseDriver::create().unwrap();
    let host = cached_host_session(&driver);
    let current_epoch = host.session.action_evidence_epoch();
    let mut sessions = ConnectionSessions::default();
    sessions.windows.insert("session-1".into(), host);
    let route = request_handler::window_evidence_epoch_route(&Request::CallTool {
        session_id: "session-1".into(),
        task_grant_id: "grant-1".into(),
        window_capability: "capability-1".into(),
        tool: "debug_window_info".into(),
        arguments: json!({}),
    });

    request_handler::finish_window_evidence_request::<(Value, Option<Vec<u8>>)>(
        &mut sessions,
        route,
        Err(HostError::ComputerUse(ComputerUseError::new(
            ComputerUseErrorCode::InputFailed,
            "phase=pre_dispatch; action_attempted=false; completion_unknown=false",
        ))),
    )
    .unwrap_err();

    let host = &sessions.windows["session-1"];
    assert_eq!(host.synchronized_action_evidence_epoch, current_epoch);
    assert!(host.latest_observation_id.is_some());
    assert!(host.latest_accessibility_state_id.is_some());
    assert!(host.latest_accessibility_root.is_some());
    assert!(host.latest_shared_image.is_some());
    assert_eq!(host.browser_evidence_epoch, Some(current_epoch));
}

#[rstest]
fn find_cache_read_rejects_a_newly_minimized_target_and_invalidates_old_uia_evidence() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut host = cached_host_session(&driver);

    let error = request_handler::finish_target_sensitive_cached_read(
        &mut host,
        Ok(target_availability(
            dcc_cua_core::ComputerUseTargetStatus::Minimized,
        )),
    )
    .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::TargetMinimized);
    assert!(host.latest_observation_id.is_none());
    assert!(host.latest_accessibility_state_id.is_none());
    assert!(host.latest_accessibility_root.is_none());
    assert!(host.latest_shared_image.is_some());
    assert_eq!(
        host.input_events.target_state().status,
        dcc_cua_core::ComputerUseTargetStatus::Minimized
    );
}

#[rstest]
#[tokio::test]
async fn input_suspend_resume_invalidates_old_raw_and_accessibility_references() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = ConnectionSessions::default();
    sessions
        .windows
        .insert("session-1".into(), cached_host_session(&driver));

    let host = sessions.windows.get_mut("session-1").unwrap();
    assert!(host.observe_input_readiness(
        dcc_cua_core::ComputerUseInputReadiness {
            status: dcc_cua_core::ComputerUseInputStatus::Suspended,
            code: "interactive_desktop_unavailable".into(),
            reason: Some("workstation locked".into()),
        },
        200,
    ));
    assert!(host.observe_input_readiness(
        dcc_cua_core::ComputerUseInputReadiness {
            status: dcc_cua_core::ComputerUseInputStatus::Ready,
            code: "interactive_desktop_ready".into(),
            reason: None,
        },
        300,
    ));
    assert!(host.latest_shared_image.is_some());
    assert_cached_action_references_are_stale(&driver, &mut sessions, "observation-after-unlock")
        .await;
}

#[rstest]
#[tokio::test]
async fn future_poll_cursor_returns_an_immediate_session_resync_page() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut sessions = ConnectionSessions::default();
    sessions
        .windows
        .insert("session-1".into(), cached_host_session(&driver));
    let mut snapshot_transport = Some(SnapshotTransport::BinaryFrame);
    let mut desktop_shared_image = None;
    let cancellation_registry = Arc::new(Mutex::new(HashMap::new()));

    let (response, attachment) = tokio::time::timeout(
        Duration::from_millis(250),
        handle_request(
            &driver,
            &mut sessions,
            &mut snapshot_transport,
            &mut desktop_shared_image,
            &cancellation_registry,
            serde_json::from_value(json!({
                "method": "poll_session_events",
                "params": {
                    "session_id": "session-1",
                    "task_grant_id": "grant-1",
                    "window_capability": "capability-1",
                    "after_sequence": 99,
                    "timeout_ms": 30_000
                }
            }))
            .unwrap(),
        ),
    )
    .await
    .expect("future cursor must never enter the long-poll wait")
    .unwrap();

    assert!(attachment.is_none());
    assert_eq!(response["type"], "session_events");
    assert_eq!(response["resync_required"], true);
    assert_eq!(response["timed_out"], false);
    assert_eq!(response["after_sequence"], 99);
    let latest_sequence = sessions.windows["session-1"].input_events.latest_sequence();
    assert_eq!(response["latest_sequence"], latest_sequence);
    assert!(latest_sequence < 99);
    assert_eq!(
        response["current_state"]["target"]["session_id"],
        "session-1"
    );
    assert_eq!(
        response["current_target_state"]["target"]["session_id"],
        "session-1"
    );
}

#[rstest]
fn exact_restore_recovery_is_advertised_only_when_the_platform_can_execute_it() {
    let exact_grant: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": "grant-1",
        "application_label": "Test DCC",
        "process_id": 42,
        "window_handle": 77
    }))
    .unwrap();

    assert_eq!(
        request_handler::restore_activate_available(&exact_grant),
        cfg!(windows)
    );
}

#[rstest]
#[case(ComputerUseErrorCode::InvalidTarget)]
#[case(ComputerUseErrorCode::BackendUnavailable)]
fn banner_failure_cleanup_keeps_the_original_typed_error_code(#[case] code: ComputerUseErrorCode) {
    let error = stopped_banner_failure(
        ComputerUseError::new(code, "presenter stopped"),
        "; CUA cleanup also failed: worker unavailable",
    );

    assert_eq!(error.code, code);
    assert!(error.message.contains("presenter stopped"));
    assert!(error.message.contains("cleanup also failed"));
}

#[rstest]
fn endpoint_singletons_are_case_insensitive_and_endpoint_scoped() {
    let first = endpoint_singleton_name(r"\\.\pipe\dcc-cua-v1-session-42");
    assert_eq!(
        first,
        endpoint_singleton_name(r"\\.\PIPE\DCC-CUA-V1-SESSION-42")
    );
    assert_ne!(
        first,
        endpoint_singleton_name(r"\\.\pipe\dcc-cua-v1-session-43")
    );
}

#[rstest]
fn runtime_session_ids_are_opaque_and_unique() {
    let first = new_runtime_session_id("window");
    let second = new_runtime_session_id("window");
    assert_ne!(first, second);
    assert!(first.starts_with("dcc-cua-window-"));
    assert!(!first.contains("public-session"));
}

#[rstest]
fn runtime_session_ids_are_rewritten_in_nested_host_responses() {
    let mut response = json!({
        "session": "runtime-a",
        "observation_id": "runtime-a-observation-7",
        "nested": [{"session_id": "runtime-b"}],
    });
    rewrite_session_aliases(
        &mut response,
        &[
            ("runtime-a", "agent-session"),
            ("runtime-b", "desktop-session"),
        ],
    );
    assert_eq!(response["session"], "agent-session");
    assert_eq!(response["observation_id"], "runtime-a-observation-7");
    assert_eq!(response["nested"][0]["session_id"], "desktop-session");
}

#[rstest]
fn private_worker_enables_the_upstream_cursor_backend() {
    assert_eq!(
        request_contract::cursor_render_backend(true),
        "cua-driver-sdk"
    );
}

#[rstest]
fn shared_interrupt_has_a_typed_host_request() {
    assert!(matches!(
        serde_json::from_value::<Request>(json!({"method":"interrupt_all", "params":{}})),
        Ok(Request::InterruptAll {})
    ));
}

#[rstest]
fn frame_prefix_is_big_endian_and_bounded() {
    assert_eq!(u32::from_be_bytes((42_u32).to_be_bytes()), 42);
    const { assert!(MAX_BINARY_FRAME_BYTES > MAX_JSON_FRAME_BYTES) };
}

#[rstest]
fn default_endpoint_uses_the_shared_protocol_contract() {
    assert_eq!(
        HostTransport::default_endpoint(),
        dcc_cua_protocol::default_endpoint()
    );
}

#[rstest]
fn endpoint_connection_limit_applies_backpressure() {
    let limiter = endpoint::connection_limiter();
    let mut permits = Vec::with_capacity(MAX_HOST_CONNECTIONS);
    for _ in 0..MAX_HOST_CONNECTIONS {
        permits.push(limiter.clone().try_acquire_owned().unwrap());
    }
    assert!(limiter.clone().try_acquire_owned().is_err());
    permits.pop();
    assert!(limiter.try_acquire_owned().is_ok());
}

#[cfg(windows)]
#[rstest]
fn windows_pipe_acl_is_scoped_to_the_current_logon() {
    let logon_sid = endpoint::current_logon_sid_string().unwrap();
    assert!(logon_sid.starts_with("S-1-5-5-"));
    assert_eq!(
        endpoint::windows_pipe_sddl(&logon_sid),
        format!("D:P(A;;GA;;;SY)(A;;GA;;;{logon_sid})")
    );
}

#[cfg(unix)]
#[rstest]
fn only_refused_or_missing_unix_sockets_are_replaceable() {
    assert!(stale_unix_socket_error(&std::io::Error::from(
        std::io::ErrorKind::ConnectionRefused,
    )));
    assert!(stale_unix_socket_error(&std::io::Error::from(
        std::io::ErrorKind::NotFound,
    )));
    assert!(!stale_unix_socket_error(&std::io::Error::from(
        std::io::ErrorKind::PermissionDenied,
    )));
}

#[cfg(unix)]
#[rstest]
fn unix_endpoint_parent_must_be_private() {
    use std::os::unix::fs::PermissionsExt;

    assert!(matches!(
        prepare_unix_endpoint_parent(std::path::Path::new("host.sock")),
        Err(HostError::Protocol(message)) if message.contains("absolute")
    ));

    let runtime_dir = std::env::temp_dir().join(format!(
        "dcc-cua-host-{}-{}",
        dcc_cua_protocol::effective_user_id(),
        Uuid::new_v4()
    ));
    let endpoint = runtime_dir.join("host.sock");
    prepare_unix_endpoint_parent(&endpoint).unwrap();
    assert!(dcc_cua_protocol::is_private_runtime_directory(&runtime_dir));

    std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        prepare_unix_endpoint_parent(&endpoint),
        Err(HostError::Protocol(message)) if message.contains("mode 0700")
    ));

    std::fs::remove_dir(runtime_dir).unwrap();
}

#[rstest]
fn request_ids_are_optional_bounded_and_echoable() {
    assert_eq!(request_id_from(&json!({})).unwrap(), None);
    assert_eq!(
        request_id_from(&json!({"request_id":"req-1"})).unwrap(),
        Some("req-1".into())
    );
    assert!(request_id_from(&json!({"request_id":""})).is_err());
    assert!(
        request_id_from(&json!({
            "request_id": "x".repeat(MAX_REQUEST_ID_CHARS + 1)
        }))
        .is_err()
    );
    assert_eq!(
        with_request_id(json!({"type":"ok"}), Some("req-1")),
        json!({"type":"ok", "request_id":"req-1"})
    );
}

#[rstest]
fn wait_cancellation_requires_exact_credentials() {
    let registry = Arc::new(Mutex::new(HashMap::new()));
    let guard = register_wait(&registry, "session-1", "grant-1", "cap-1").unwrap();
    assert!(cancel_wait(&registry, "session-1", "grant-1", "wrong-cap").is_err());
    let response = cancel_wait(&registry, "session-1", "grant-1", "cap-1").unwrap();
    assert_eq!(response["type"], "wait_cancel_requested");
    assert!(guard.handle.cancelled.load(Ordering::Acquire));
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn wait_probe_obeys_the_absolute_request_deadline() {
    let registry = Arc::new(Mutex::new(HashMap::new()));
    let guard = register_wait(&registry, "session-1", "grant-1", "cap-1").unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(25);

    let outcome = wait::wait_for_probe_until(&guard.handle, deadline, async {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        7_u8
    })
    .await;

    assert!(matches!(outcome, wait::WaitProbeOutcome::TimedOut));
}

#[rstest]
fn window_wait_cancellation_uses_the_request_id_handle() {
    let registry = Arc::new(Mutex::new(HashMap::new()));
    let guard = register_window_wait(&registry, "window-wait-1").unwrap();
    let response = cancel_window_wait(&registry, "window-wait-1").unwrap();
    assert_eq!(response["type"], "window_wait_cancel_requested");
    assert_eq!(response["wait_id"], "window-wait-1");
    assert!(guard.handle.cancelled.load(Ordering::Acquire));
    assert!(cancel_window_wait(&registry, "missing").is_err());
}

#[rstest]
fn request_frame_preserves_correlation_on_deserialization_errors() {
    let parsed = parse_request_frame(br#"{"request_id":"req-7","method":"unknown","params":{}}"#);
    assert_eq!(parsed.unwrap_err().0, Some("req-7".into()));
}

#[rstest]
fn window_state_wire_surface_matches_cua_capability() {
    assert!(serde_json::from_value::<WindowOperation>(json!("activate")).is_ok());
    assert!(serde_json::from_value::<WindowOperation>(json!("restore_activate")).is_ok());
    assert!(serde_json::from_value::<WindowOperation>(json!("close")).is_ok());
    assert!(serde_json::from_value::<WindowOperation>(json!("restore")).is_err());
    assert!(serde_json::from_value::<WindowOperation>(json!("show")).is_err());
}

#[rstest]
fn restore_activate_is_an_explicit_exact_session_scoped_request() {
    let request = serde_json::from_value::<Request>(json!({
        "method": "change_window_state",
        "params": {
            "session_id": "session-1",
            "task_grant_id": "grant-1",
            "window_capability": "capability-1",
            "operation": "restore_activate"
        }
    }))
    .unwrap();

    assert!(matches!(
        request,
        Request::ChangeWindowState {
            session_id,
            task_grant_id,
            window_capability,
            operation: WindowOperation::RestoreActivate,
        } if session_id == "session-1"
            && task_grant_id == "grant-1"
            && window_capability == "capability-1"
    ));
}

#[rstest]
fn restore_activate_response_preserves_core_recovery_fences() {
    let response = request_contract::window_state_changed_response(
        "session-1",
        "restore_activate",
        json!({"minimized": false, "foreground": true}),
        json!({
            "success": true,
            "automatic_input": false,
            "fresh_observation_required": true,
        }),
    );

    assert_eq!(response["type"], "window_state_changed");
    assert_eq!(response["operation"], "restore_activate");
    assert_eq!(response["result"]["automatic_input"], false);
    assert_eq!(response["result"]["fresh_observation_required"], true);
}

#[rstest]
fn failed_window_mutation_still_invalidates_host_observation_cache() {
    let invalidated = std::cell::Cell::new(false);

    let result =
        finish_window_mutation_attempt(Err::<(), _>("foreground denied"), || invalidated.set(true));

    assert_eq!(result, Err("foreground denied"));
    assert!(invalidated.get());
}

#[rstest]
fn hard_denied_intents_do_not_reach_cua() {
    let action = HostAction {
        action: "keypress".into(),
        element_index: None,
        element_token: None,
        delivery_mode: None,
        input_backend_id: None,
        input_kind: "raw_input".into(),
        intent: "terminal_or_run_dialog".into(),
        x: None,
        y: None,
        button: None,
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: Vec::new(),
        text: None,
        delay_ms: None,
        type_chars_only: false,
        checked: None,
        keys: vec!["ENTER".into()],
        modifiers: Vec::new(),
        duration_ms: None,
        steps: None,
    };
    assert!(action.reject_policy().is_some());
    assert!(action.requires_approval());
}

#[rstest]
#[case("windows_security_or_privacy")]
#[case("human_verification")]
fn trusted_confirmation_intents_require_the_explicit_grant(#[case] intent: &str) {
    let action = HostAction {
        action: "click".into(),
        element_index: None,
        element_token: None,
        delivery_mode: None,
        input_backend_id: None,
        input_kind: "raw_input".into(),
        intent: intent.into(),
        x: Some(10.0),
        y: Some(10.0),
        button: Some("left".into()),
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: Vec::new(),
        text: None,
        delay_ms: None,
        type_chars_only: false,
        checked: None,
        keys: Vec::new(),
        modifiers: Vec::new(),
        duration_ms: None,
        steps: None,
    };
    assert!(action.reject_policy().is_none());
    assert!(action.requires_approval());
}

struct EchoingConfirmationHost;

#[async_trait::async_trait]
impl TrustedActionConfirmationHost for EchoingConfirmationHost {
    async fn confirm(
        &self,
        request: TrustedActionConfirmationRequest,
    ) -> Result<TrustedActionConfirmationDecision, TrustedActionConfirmationHostError> {
        Ok(TrustedActionConfirmationDecision {
            action: TrustedActionConfirmationAction::Allow,
            request_digest: request.request_digest,
        })
    }
}

struct ReplayingConfirmationHost {
    request_digest: String,
}

struct UnexpectedConfirmationHost;

#[async_trait::async_trait]
impl TrustedActionConfirmationHost for UnexpectedConfirmationHost {
    async fn confirm(
        &self,
        _request: TrustedActionConfirmationRequest,
    ) -> Result<TrustedActionConfirmationDecision, TrustedActionConfirmationHostError> {
        panic!("the constructor-owned host must not run without the task-grant gate")
    }
}

#[async_trait::async_trait]
impl TrustedActionConfirmationHost for ReplayingConfirmationHost {
    async fn confirm(
        &self,
        _request: TrustedActionConfirmationRequest,
    ) -> Result<TrustedActionConfirmationDecision, TrustedActionConfirmationHostError> {
        Ok(TrustedActionConfirmationDecision {
            action: TrustedActionConfirmationAction::Allow,
            request_digest: self.request_digest.clone(),
        })
    }
}

fn confirmation_action() -> HostAction {
    HostAction {
        action: "click".into(),
        element_index: Some(7),
        element_token: Some("submit-button".into()),
        delivery_mode: Some("foreground".into()),
        input_backend_id: None,
        input_kind: "semantic".into(),
        intent: "confirm".into(),
        x: None,
        y: None,
        button: Some("left".into()),
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: Vec::new(),
        text: None,
        delay_ms: None,
        type_chars_only: false,
        checked: None,
        keys: Vec::new(),
        modifiers: Vec::new(),
        duration_ms: None,
        steps: None,
    }
}

#[rstest]
#[tokio::test]
async fn trusted_confirmation_requires_a_constructor_owned_host() {
    let outcome = authorize_action_confirmation(
        None,
        true,
        TrustedActionConfirmationRequest::for_window_action(
            "session-1",
            "grant-1",
            "capability-1",
            "observation-1",
            "accessibility-1",
            &confirmation_action(),
        )
        .unwrap(),
    )
    .await;

    assert_eq!(outcome, ActionConfirmationOutcome::Required);
}

#[rstest]
#[tokio::test]
async fn trusted_confirmation_task_grant_gate_cannot_be_bypassed_by_the_host() {
    let host: Arc<dyn TrustedActionConfirmationHost> = Arc::new(UnexpectedConfirmationHost);
    let outcome = authorize_action_confirmation(
        Some(host.as_ref()),
        false,
        TrustedActionConfirmationRequest::for_window_action(
            "session-1",
            "grant-1",
            "capability-1",
            "observation-1",
            "accessibility-1",
            &confirmation_action(),
        )
        .unwrap(),
    )
    .await;

    assert_eq!(outcome, ActionConfirmationOutcome::Required);
}

#[rstest]
#[tokio::test]
async fn trusted_confirmation_accepts_an_exact_action_bound_decision() {
    let host: Arc<dyn TrustedActionConfirmationHost> = Arc::new(EchoingConfirmationHost);
    let outcome = authorize_action_confirmation(
        Some(host.as_ref()),
        true,
        TrustedActionConfirmationRequest::for_window_action(
            "session-1",
            "grant-1",
            "capability-1",
            "observation-1",
            "accessibility-1",
            &confirmation_action(),
        )
        .unwrap(),
    )
    .await;

    assert_eq!(outcome, ActionConfirmationOutcome::Allowed);
}

#[rstest]
#[tokio::test]
async fn trusted_confirmation_rejects_a_replayed_decision_for_new_evidence() {
    let first = TrustedActionConfirmationRequest::for_window_action(
        "session-1",
        "grant-1",
        "capability-1",
        "observation-1",
        "accessibility-1",
        &confirmation_action(),
    )
    .unwrap();
    let host: Arc<dyn TrustedActionConfirmationHost> = Arc::new(ReplayingConfirmationHost {
        request_digest: first.request_digest,
    });
    let second = TrustedActionConfirmationRequest::for_window_action(
        "session-1",
        "grant-1",
        "capability-1",
        "observation-2",
        "accessibility-2",
        &confirmation_action(),
    )
    .unwrap();

    let outcome = authorize_action_confirmation(Some(host.as_ref()), true, second).await;

    assert_eq!(outcome, ActionConfirmationOutcome::Required);
}

#[rstest]
fn semantic_actions_require_element_locator() {
    let action = HostAction {
        action: "set_checked".into(),
        element_index: None,
        element_token: None,
        delivery_mode: None,
        input_backend_id: None,
        input_kind: "semantic".into(),
        intent: "ordinary_edit".into(),
        x: None,
        y: None,
        button: None,
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: Vec::new(),
        text: None,
        delay_ms: None,
        type_chars_only: false,
        checked: Some(true),
        keys: Vec::new(),
        modifiers: Vec::new(),
        duration_ms: None,
        steps: None,
    };
    let error = action.into_computer_use("obs-1".into()).unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::InvalidAction);
}

#[rstest]
fn semantic_actions_forward_element_tokens_and_delivery_mode() {
    let action = HostAction {
        action: "click".into(),
        element_index: None,
        element_token: Some("element-token".into()),
        delivery_mode: Some("background".into()),
        input_backend_id: None,
        input_kind: "semantic".into(),
        intent: "ordinary_edit".into(),
        x: None,
        y: None,
        button: None,
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: Vec::new(),
        text: None,
        delay_ms: None,
        type_chars_only: false,
        checked: None,
        keys: Vec::new(),
        modifiers: Vec::new(),
        duration_ms: None,
        steps: None,
    };
    let action = action.into_computer_use("obs-1".into()).unwrap();
    assert_eq!(action.element_token.as_deref(), Some("element-token"));
    assert_eq!(action.delivery_mode.as_deref(), Some("background"));
}

#[rstest]
#[case("press", "keypress")]
#[case("press_key", "keypress")]
#[case("hotkey", "keyboard_shortcut")]
fn host_action_normalizes_documented_keyboard_aliases(
    #[case] alias: &str,
    #[case] canonical: &str,
) {
    let action: HostAction = serde_json::from_value(json!({
        "action": alias,
        "input_kind": "raw_input",
        "intent": "navigate",
        "keys": ["SPACE"]
    }))
    .unwrap();

    let action = action.into_computer_use("obs-1".into()).unwrap();

    assert_eq!(action.action, canonical);
    assert_eq!(action.keys, ["SPACE"]);
}

#[rstest]
fn host_action_forwards_the_explicit_input_backend_id() {
    let action: HostAction = serde_json::from_value(json!({
        "action": "drag",
        "input_kind": "raw_input",
        "intent": "ordinary_edit",
        "delivery_mode": "foreground",
        "input_backend_id": "windows.synthetic_touch.v1",
        "path": [{"x": 1.0, "y": 2.0}, {"x": 3.0, "y": 4.0}]
    }))
    .unwrap();
    let action = action.into_computer_use("obs-1".into()).unwrap();
    assert_eq!(
        action.input_backend_id.as_deref(),
        Some("windows.synthetic_touch.v1")
    );
}

#[rstest]
fn host_action_forwards_the_combined_down_drag_backend_id() {
    let action: HostAction = serde_json::from_value(json!({
        "action": "drag",
        "input_kind": "raw_input",
        "intent": "ordinary_edit",
        "delivery_mode": "foreground",
        "input_backend_id": "windows.send_input.combined_down_drag.v1",
        "button": "left",
        "path": [{"x": 1.0, "y": 2.0}, {"x": 3.0, "y": 4.0}]
    }))
    .unwrap();
    let action = action.into_computer_use("obs-1".into()).unwrap();
    assert_eq!(
        action.input_backend_id.as_deref(),
        Some("windows.send_input.combined_down_drag.v1")
    );
}

#[rstest]
fn hello_selects_snapshot_transport() {
    let shared_memory = HelloParams {
        protocol_version: HOST_PROTOCOL_VERSION,
        client_name: "test-client".into(),
        snapshot_transport: Some("shared_memory".into()),
    };
    assert_eq!(
        SnapshotTransport::from_hello(&shared_memory).unwrap(),
        SnapshotTransport::SharedMemory
    );
    let binary_frame = HelloParams {
        protocol_version: HOST_PROTOCOL_VERSION,
        client_name: "test-client".into(),
        snapshot_transport: None,
    };
    assert_eq!(
        SnapshotTransport::from_hello(&binary_frame).unwrap(),
        SnapshotTransport::BinaryFrame
    );
}

#[rstest]
fn app_launch_grant_defaults_to_denied() {
    let grant: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": "task-1",
        "application_label": "Unreal Editor"
    }))
    .expect("minimal grants should be readable");
    grant.validate_identity().unwrap();
    assert!(!grant.allow_app_launch);
    assert!(!grant.allow_app_terminate);
    assert!(!grant.allow_clipboard_read);
    assert!(!grant.allow_clipboard_write);
    assert!(!grant.allow_recording);
    assert!(grant.showcase_output_dir.is_none());
    assert!(!grant.allow_live_observation);
    assert!(!grant.allow_browser_input);
    assert!(!grant.allow_browser_prepare);
    assert!(!grant.allow_browser_download);
    assert!(!grant.allow_native_tool);
    assert!(!grant.allow_menu_invoke);
    assert!(!grant.allow_session_escalation);
    assert!(!grant.allow_trusted_confirmation);
    assert_eq!(
        error_code(&HostError::coded_protocol(
            HostProtocolErrorCode::BrowserDownloadNotGranted,
            "browser download is not granted",
        )),
        "browser_download_not_granted"
    );
    assert_eq!(
        error_code(&HostError::coded_protocol(
            HostProtocolErrorCode::MenuInvokeNotGranted,
            "native menu invocation is not granted",
        )),
        "menu_invoke_not_granted"
    );
    assert_eq!(
        error_code(&HostError::ComputerUse(ComputerUseError::new(
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
            "session disconnected",
        ))),
        "interactive_desktop_unavailable"
    );
    assert_eq!(
        error_code(&HostError::ComputerUse(ComputerUseError::new(
            ComputerUseErrorCode::CompletionUnknown,
            "activation completion is unknown",
        ))),
        "completion_unknown"
    );
    assert_eq!(
        error_code(&HostError::ComputerUse(ComputerUseError::new(
            ComputerUseErrorCode::SessionRefreshRequired,
            "refresh before taking a new observation",
        ))),
        "session_refresh_required"
    );
}

#[rstest]
fn observation_invalidation_keeps_the_published_shared_image_handoff_alive() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut host = cached_host_session(&driver);
    let descriptor = host
        .latest_shared_image
        .as_ref()
        .unwrap()
        .descriptor()
        .clone();

    host.invalidate_observations();

    let reader = SharedImageReader::open(descriptor)
        .expect("observation invalidation must not revoke a published descriptor");
    assert_eq!(reader.read().unwrap(), b"old");
}

#[cfg(windows)]
#[rstest]
#[tokio::test]
async fn first_named_pipe_instance_rejects_a_precreated_endpoint() {
    let endpoint_name = format!(
        r"\\.\pipe\dcc-cua-first-instance-test-{}",
        uuid::Uuid::new_v4()
    );
    let _first = endpoint::create_secure_named_pipe(&endpoint_name, true)
        .expect("create the first protected pipe instance");

    let error = match endpoint::create_secure_named_pipe(&endpoint_name, true) {
        Ok(_) => panic!("a second first instance must fail closed"),
        Err(error) => error,
    };
    assert!(matches!(error, HostError::EndpointHijacked { .. }));

    endpoint::create_secure_named_pipe(&endpoint_name, false)
        .expect("the accept loop may add a non-first instance");
}

#[rstest]
fn protocol_wire_codes_are_explicit_and_never_inferred_from_prose() {
    assert_eq!(
        error_code(&HostError::Protocol(
            "recording path contains an invalid character".into()
        )),
        "invalid_request"
    );
    assert_eq!(
        error_code(&HostError::coded_protocol(
            HostProtocolErrorCode::RecordingNotGranted,
            "recording is not granted",
        )),
        "recording_not_granted"
    );
}

mod browser_extension;
mod connection;
mod request_contracts;
mod request_parsing;
mod response_contracts;
mod session_concurrency;
mod session_health;
