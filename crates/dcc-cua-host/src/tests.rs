use rstest::rstest;
use serde_json::Value;
use tokio::io::{AsyncWrite, DuplexStream};

use super::*;
use crate::endpoint::endpoint_singleton_name;
#[cfg(unix)]
use crate::endpoint::{prepare_unix_endpoint_parent, stale_unix_socket_error};
use crate::request_handler::acquire_raw_input_turn;
use crate::request_handler::bind_launched_process;
use crate::request_handler::finish_window_mutation_attempt;
use crate::request_handler::poll_session_events_timeout;
use crate::request_handler::post_snapshot_delay;
use crate::request_handler::session_stopped_response;
use crate::request_handler::take_connection_session;
use crate::session_events::SessionInputEventQueue;

#[rstest]
fn cursor_render_backend_matches_the_native_platform_owner() {
    let enabled = cfg!(any(windows, target_os = "linux"));
    let expected = if enabled {
        "cua-driver-sdk"
    } else {
        "unavailable"
    };
    assert_eq!(request_handler::cursor_render_backend(enabled), expected);
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
    assert!(capabilities.contains(&"indicator_motion_policy"));
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
    }
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
    assert!(host.latest_shared_image.is_none());
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
    assert!(host.latest_shared_image.is_none());

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
    assert!(host.latest_shared_image.is_none());
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
    assert!(host.latest_shared_image.is_none());
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
    assert!(host.latest_shared_image.is_none());
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
    assert!(host.latest_shared_image.is_none());
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
    assert!(host.latest_shared_image.is_none());
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

    assert!(sessions.windows["session-1"].latest_shared_image.is_none());
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
    host.finish_browser_snapshot_attempt(Ok(())).unwrap();

    let current_epoch = host.session.action_evidence_epoch();
    assert!(current_epoch > previous_epoch);
    assert_eq!(host.synchronized_action_evidence_epoch, current_epoch);
    assert_eq!(host.browser_evidence_epoch, Some(current_epoch));
    assert!(host.latest_observation_id.is_none());
    assert!(host.latest_accessibility_state_id.is_none());
    assert!(host.latest_accessibility_root.is_none());
    assert!(host.latest_shared_image.is_none());
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
    assert!(host.latest_shared_image.is_none());
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
    assert!(host.latest_shared_image.is_none());
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
    assert!(host.latest_shared_image.is_none());
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
        request_handler::cursor_render_backend(true),
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

async fn write_json_request(
    writer: &mut (impl AsyncWrite + Unpin),
    value: Value,
) -> Result<(), HostError> {
    write_frame(
        writer,
        &serde_json::to_vec(&value).unwrap(),
        MAX_JSON_FRAME_BYTES,
    )
    .await
}

#[rstest]
#[tokio::test]
async fn process_connection_requires_hello_pings_and_rejects_duplicate_hello() {
    let (mut client, server_stream): (DuplexStream, DuplexStream) = tokio::io::duplex(16 * 1024);
    let server = tokio::spawn(process_connection(
        ComputerUseDriver::create().unwrap(),
        server_stream,
    ));

    write_json_request(
        &mut client,
        json!({"request_id":"pre-hello", "method":"ping", "params":{}}),
    )
    .await
    .unwrap();
    let response = read_frame(&mut client, MAX_JSON_FRAME_BYTES)
        .await
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "protocol_error");
    assert_eq!(response["request_id"], "pre-hello");

    let hello = json!({
        "request_id": "hello-1",
        "method": "hello",
        "params": {
            "protocol_version": HOST_PROTOCOL_VERSION,
            "client_name": "host-integration-test",
            "snapshot_transport": "binary_frame"
        }
    });
    write_json_request(&mut client, hello).await.unwrap();
    let response = read_frame(&mut client, MAX_JSON_FRAME_BYTES)
        .await
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["type"], "hello");
    assert_eq!(response["request_id"], "hello-1");
    assert_eq!(response["protocol_version"], HOST_PROTOCOL_VERSION);
    assert!(
        response["capabilities"]
            .as_array()
            .is_some_and(|items| { items.iter().any(|item| item == "pipelined_read_requests") })
    );
    assert!(
        response["capabilities"]
            .as_array()
            .is_some_and(|items| { items.iter().any(|item| item == "window_inventory_filters") })
    );
    assert!(
        response["capabilities"]
            .as_array()
            .is_some_and(|items| { items.iter().any(|item| item == "host_ping") })
    );
    assert!(
        response["capabilities"]
            .as_array()
            .is_some_and(|items| { items.iter().any(|item| item == "host_diagnostics") })
    );
    assert!(
        response["capabilities"]
            .as_array()
            .is_some_and(|items| { items.iter().any(|item| item == "serialized_raw_input") })
    );

    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(300),
            read_frame(&mut client, MAX_JSON_FRAME_BYTES),
        )
        .await
        .is_err(),
        "session event monitoring must never emit an unsolicited Host frame",
    );

    write_json_request(
        &mut client,
        json!({"request_id":"ping-1", "method":"ping", "params":{}}),
    )
    .await
    .unwrap();
    let response = read_frame(&mut client, MAX_JSON_FRAME_BYTES)
        .await
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["type"], "pong");
    assert_eq!(response["request_id"], "ping-1");
    assert_eq!(response["protocol_version"], HOST_PROTOCOL_VERSION);

    write_json_request(
        &mut client,
        json!({
            "request_id": "hello-2",
            "method": "hello",
            "params": {
                "protocol_version": HOST_PROTOCOL_VERSION,
                "client_name": "host-integration-test",
                "snapshot_transport": "shared_memory"
            }
        }),
    )
    .await
    .unwrap();
    let response = read_frame(&mut client, MAX_JSON_FRAME_BYTES)
        .await
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["type"], "error");
    assert_eq!(response["request_id"], "hello-2");
    assert_eq!(response["code"], "invalid_request");

    drop(client);
    assert!(server.await.unwrap().is_ok());
}

#[rstest]
#[tokio::test]
async fn connection_closes_when_hello_misses_its_absolute_deadline() {
    let (mut client, server_stream): (DuplexStream, DuplexStream) = tokio::io::duplex(4096);
    let (reader, writer) = tokio::io::split(server_stream);
    let server = tokio::spawn(process_connection_parts(
        ComputerUseDriver::create().unwrap(),
        reader,
        writer,
        std::time::Duration::from_millis(25),
    ));

    write_json_request(
        &mut client,
        json!({"request_id":"pre-hello", "method":"ping", "params":{}}),
    )
    .await
    .unwrap();
    let response = read_frame(&mut client, MAX_JSON_FRAME_BYTES)
        .await
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["code"], "protocol_error");

    let error = tokio::time::timeout(std::time::Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("hello was not completed within 25 ms")
    );
}

#[rstest]
#[tokio::test]
async fn connection_finalizer_aborts_tasks_and_cleans_up_after_errors() {
    let cleaned = Arc::new(AtomicBool::new(false));
    let cleanup_flag = cleaned.clone();
    let mut tasks = JoinSet::new();
    tasks.spawn(std::future::pending::<Result<(), HostError>>());

    let result = finalize_connection(
        Err(HostError::Protocol("broken connection".into())),
        &mut tasks,
        async move {
            cleanup_flag.store(true, Ordering::Release);
            Ok(())
        },
    )
    .await;

    assert!(matches!(result, Err(HostError::Protocol(_))));
    assert!(cleaned.load(Ordering::Acquire));
    assert!(tasks.is_empty());
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
    let response = request_handler::window_state_changed_response(
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
    assert!(action.requires_approval(true));
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
    assert!(action.requires_approval(false));
    assert!(!action.requires_approval(true));
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
        error_code(&HostError::Protocol(
            "browser download is not granted".into()
        )),
        "browser_download_not_granted"
    );
    assert_eq!(
        error_code(&HostError::Protocol(
            "native menu invocation is not granted".into()
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
#[case(ComputerUseErrorCode::InvalidTarget, "invalid_target")]
#[case(ComputerUseErrorCode::TargetMinimized, "target_minimized")]
#[case(ComputerUseErrorCode::TargetUnavailable, "target_unavailable")]
#[case(ComputerUseErrorCode::MissingWindow, "target_unavailable")]
fn target_error_codes_keep_wire_contract(
    #[case] code: ComputerUseErrorCode,
    #[case] expected: &str,
) {
    assert_eq!(
        error_code(&HostError::ComputerUse(ComputerUseError::new(code, "test"))),
        expected
    );
}

#[rstest]
fn post_snapshot_delay_is_bounded_and_requires_capture() {
    assert_eq!(post_snapshot_delay(true, 1_500).unwrap().as_millis(), 1_500);
    assert!(post_snapshot_delay(true, MAX_POST_SNAPSHOT_DELAY_MS + 1).is_err());
    assert!(post_snapshot_delay(false, 1).is_err());
}

#[rstest]
fn live_observation_requests_parse() {
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "live_observation_start",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {"fps": 15}
            }
        })),
        Ok(Request::LiveObservationStart { request, .. })
            if request.fps == 15 && request.max_dimension == 1_568
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "live_observation_state",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1"
            }
        })),
        Ok(Request::LiveObservationState { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "live_observation_stop",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1"
            }
        })),
        Ok(Request::LiveObservationStop { .. })
    ));
}

#[rstest]
fn open_session_bootstrap_activation_is_explicit_and_defaults_off() {
    let default_request = serde_json::from_value::<Request>(json!({
        "method": "open_session",
        "params": {
            "session_id": "session-default",
            "grant": {
                "task_grant_id": "task-1",
                "application_label": "Synthetic Test App",
                "process_id": 4242,
                "window_handle": 31337
            }
        }
    }))
    .unwrap();
    assert!(matches!(
        default_request,
        Request::OpenSession {
            activate_before: false,
            indicator_motion: IndicatorMotionPolicy::Auto,
            ..
        }
    ));

    let bootstrap_request = serde_json::from_value::<Request>(json!({
        "method": "open_session",
        "params": {
            "session_id": "session-bootstrap",
            "activate_before": true,
            "grant": {
                "task_grant_id": "task-1",
                "application_label": "Synthetic Test App",
                "process_id": 4242,
                "window_handle": 31337
            }
        }
    }))
    .unwrap();
    assert!(matches!(
        bootstrap_request,
        Request::OpenSession {
            activate_before: true,
            indicator_motion: IndicatorMotionPolicy::Auto,
            ..
        }
    ));

    let animated_request = serde_json::from_value::<Request>(json!({
        "method": "open_session",
        "params": {
            "session_id": "session-animated",
            "indicator_motion": "animate",
            "grant": {
                "task_grant_id": "task-1",
                "application_label": "Synthetic Test App",
                "process_id": 4242,
                "window_handle": 31337
            }
        }
    }))
    .unwrap();
    assert!(matches!(
        animated_request,
        Request::OpenSession {
            indicator_motion: IndicatorMotionPolicy::Animate,
            ..
        }
    ));
}

#[rstest]
#[case("", "Application")]
#[case(" task-1", "Application")]
#[case("task-1", "")]
#[case("task-1", "Application\nspoof")]
fn grant_identity_is_generic_bounded_and_banner_safe(
    #[case] task_grant_id: &str,
    #[case] application_label: &str,
) {
    let grant: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": task_grant_id,
        "application_label": application_label
    }))
    .unwrap();
    assert!(grant.validate_identity().is_err());
}

#[rstest]
fn grant_identity_rejects_oversized_and_legacy_fields() {
    let oversized: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": "task-1",
        "application_label": "x".repeat(crate::task_grant::MAX_APPLICATION_LABEL_CHARS + 1)
    }))
    .unwrap();
    assert!(oversized.validate_identity().is_err());
    assert!(
        serde_json::from_value::<TaskGrant>(json!({
            "task_grant_id": "task-1",
            "application_label": "Application",
            "dcc_type": "legacy"
        }))
        .is_err()
    );
}

#[rstest]
fn launch_ownership_requires_the_same_grant_and_process() {
    let launched = HostLaunchSession {
        runtime_session_id: "private-launch-session".into(),
        task_grant_id: "task-1".into(),
        application_label: "Unreal Editor".into(),
        process_id: 4242,
    };
    let mut matching: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": "task-1",
        "application_label": "Unreal Editor"
    }))
    .unwrap();
    bind_launched_process(&launched, &mut matching).unwrap();
    assert_eq!(matching.process_id, Some(4242));

    let mut wrong_process: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": "task-1",
        "application_label": "Unreal Editor",
        "process_id": 7
    }))
    .unwrap();
    assert!(bind_launched_process(&launched, &mut wrong_process).is_err());

    let mut wrong_grant: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": "task-2",
        "application_label": "Unreal Editor"
    }))
    .unwrap();
    assert!(bind_launched_process(&launched, &mut wrong_grant).is_err());

    let mut wrong_label: TaskGrant = serde_json::from_value(json!({
        "task_grant_id": "task-1",
        "application_label": "Maya"
    }))
    .unwrap();
    assert!(bind_launched_process(&launched, &mut wrong_label).is_err());
}

mod request_parsing;
mod response_contracts;
mod session_health;
