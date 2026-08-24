use rstest::rstest;

use super::*;

#[rstest]
fn session_health_policy_is_optional_for_older_clients() {
    let request = serde_json::from_value::<Request>(json!({
        "method": "session_health",
        "params": {
            "session_id": "session-1",
            "task_grant_id": "task-1",
            "window_capability": "cap-1"
        }
    }))
    .expect("parse compatible session health request");

    assert!(matches!(
        request,
        Request::SessionHealth { policy, .. }
            if policy == dcc_cua_core::ComputerUseSessionHealthPolicy::default()
    ));
}

#[rstest]
fn session_health_parses_an_explicit_recording_progress_policy() {
    let request = serde_json::from_value::<Request>(json!({
        "method": "session_health",
        "params": {
            "session_id": "session-1",
            "task_grant_id": "task-1",
            "window_capability": "cap-1",
            "policy": {
                "require_recording_healthy": true,
                "require_recording_progress": true,
                "previous_recording_progress": {
                    "lane": "video",
                    "trajectory_turn": 7,
                    "finalized_segments": 2,
                    "current_partial_size_bytes": 1000,
                    "current_partial_modified_at_unix_ms": 80
                }
            }
        }
    }))
    .expect("parse explicit session health policy");

    assert!(matches!(
        request,
        Request::SessionHealth { policy, .. }
            if policy.require_recording_healthy
                && policy.require_recording_progress
                && policy.previous_recording_progress.is_some()
    ));
}

#[rstest]
fn app_requests_parse_with_host_params_frames() {
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "ping",
            "params": {}
        })),
        Ok(Request::Ping {})
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "doctor",
            "params": {}
        })),
        Ok(Request::Doctor {})
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "list_apps",
            "params": {}
        })),
        Ok(Request::ListApps {})
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "list_tools",
            "params": {}
        })),
        Ok(Request::ListTools {})
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "list_windows",
            "params": {
                "app": "chrome.exe",
                "window_id": 42,
                "window_title": "PCG Fab"
            }
        })),
        Ok(Request::ListWindows {
            window_id: Some(42),
            window_title: Some(title),
            ..
        }) if title == "PCG Fab"
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "wait_for_window",
            "params": {"query": {"app": "UE5Editor.exe"}, "timeout_ms": 1000}
        })),
        Ok(Request::WaitForWindow(..))
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "cancel_window_wait",
            "params": {"wait_id": "window-wait-1"}
        })),
        Ok(Request::CancelWindowWait { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "desktop_snapshot",
            "params": {}
        })),
        Ok(Request::DesktopSnapshot {})
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "snapshot",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "activate_before": true
            }
        })),
        Ok(Request::Snapshot {
            max_depth: 0,
            max_nodes: 0,
            activate_before: true,
            ..
        })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "zoom",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {
                    "observation_id": "session-1-obs-1",
                    "x1": 10,
                    "y1": 20,
                    "x2": 400,
                    "y2": 200
                }
            }
        })),
        Ok(Request::Zoom { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "set_window_frame",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "frame": {"x": -20.5, "y": 10, "width": 1280, "height": 720}
            }
        })),
        Ok(Request::SetWindowFrame { frame, .. }) if frame.x == -20.5 && frame.width == 1280.0
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "invoke_menu",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {"path": ["Window", "Arrange", "Left"]}
            }
        })),
        Ok(Request::InvokeMenu { request, .. }) if request.path[2] == "Left"
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "verify_state",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "expect": [{"window": {"exists": true}}],
                "stable_samples": 2
            }
        })),
        Ok(Request::VerifyState { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "execute_action",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "observation_id": "obs-1",
                "accessibility_state_id": "obs-1",
                "action": {
                    "action": "click",
                    "input_kind": "semantic",
                    "intent": "ordinary_edit",
                    "element_token": "token-1"
                },
                "capture_after": true,
                "post_snapshot_delay_ms": 1500,
                "post_snapshot_max_nodes": 256,
                "post_snapshot_max_depth": 12
            }
        })),
        Ok(Request::ExecuteAction {
            capture_after: true,
            post_snapshot_delay_ms: 1500,
            post_snapshot_max_nodes: 256,
            post_snapshot_max_depth: 12,
            ..
        })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "call_tool",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "tool": "debug_window_info",
                "arguments": {}
            }
        })),
        Ok(Request::CallTool { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "call_global_tool",
            "params": {
                "grant": {
                    "task_grant_id": "task-1",
                    "application_label": "Desktop",
                    "allow_native_tool": true
                },
                "tool": "health_report",
                "arguments": {}
            }
        })),
        Ok(Request::CallGlobalTool { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "screen_size",
            "params": {}
        })),
        Ok(Request::ScreenSize {})
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "get_session_state",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1"
            }
        })),
        Ok(Request::GetSessionState { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "escalate_session",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "reason": "foreground_ineffective",
                "detail": "window route exhausted"
            }
        })),
        Ok(Request::EscalateSession { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "cursor_tool",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "tool": "set_agent_cursor_enabled",
                "arguments": {"enabled": true}
            }
        })),
        Ok(Request::CursorTool { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "cursor_position",
            "params": {}
        })),
        Ok(Request::CursorPosition {})
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "open_desktop_session",
            "params": {
                "session_id": "desktop-1",
                "grant": {
                    "task_grant_id": "task-1",
                    "application_label": "Desktop",
                    "allow_raw_input": true
                }
            }
        })),
        Ok(Request::OpenDesktopSession { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "execute_desktop_action",
            "params": {
                "session_id": "desktop-1",
                "task_grant_id": "task-1",
                "desktop_capability": "cap-1",
                "observation_id": "desktop-obs-1",
                "capture_after": true,
                "post_snapshot_delay_ms": 750,
                "action": {
                    "action": "click",
                    "input_kind": "raw_input",
                    "intent": "ordinary_edit",
                    "x": 10,
                    "y": 20
                }
            }
        })),
        Ok(Request::ExecuteDesktopAction {
            capture_after: true,
            post_snapshot_delay_ms: 750,
            ..
        })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "launch_app",
            "params": {
                "session_id": "session-launch",
                "grant": {
                    "task_grant_id": "task-1",
                    "application_label": "Unreal Editor",
                    "allow_app_launch": true
                },
                "launch": {"name": "Calculator"}
            }
        })),
        Ok(Request::LaunchApp { .. })
    ));
    assert!(
        serde_json::from_value::<Request>(json!({
            "method": "launch_app",
            "params": {
                "grant": {
                    "task_grant_id": "task-1",
                    "application_label": "Unreal Editor",
                    "allow_app_launch": true
                },
                "launch": {"name": "Calculator"}
            }
        }))
        .is_err()
    );
    let request = serde_json::from_value::<Request>(json!({
        "method": "open_session",
        "params": {
            "session_id": "session-title",
            "grant": {
                "task_grant_id": "task-1",
                "application_label": "Unreal Editor",
                "window_title": "PCG Fab"
            }
        }
    }))
    .unwrap();
    let Request::OpenSession { grant, .. } = request else {
        panic!("expected open_session request");
    };
    assert_eq!(grant.window_title.as_deref(), Some("PCG Fab"));
    for grant in [
        json!({
            "task_grant_id": "task-1",
            "application_label": "Unreal Editor",
            "process_id": 42,
            "window_handle": 7,
            "task_authorization_id": "task-auth-1"
        }),
        json!({
            "task_grant_id": "task-1",
            "application_label": "Unreal Editor",
            "process_id": 42,
            "window_handle": 7,
            "task_authorization_window_capability": "cua-window-1"
        }),
    ] {
        let Request::OpenSession { grant, .. } = serde_json::from_value(json!({
            "method": "open_session",
            "params": {"session_id": "session-auth", "grant": grant}
        }))
        .unwrap() else {
            panic!("expected open_session request");
        };
        assert!(grant.validate_identity().is_err());
    }
    let Request::OpenSession { grant, .. } = serde_json::from_value(json!({
        "method": "open_session",
        "params": {
            "session_id": "session-auth",
            "grant": {
                "task_grant_id": "task-1",
                "application_label": "Unreal Editor",
                "process_id": 42,
                "window_handle": 7,
                "task_authorization_id": "task-auth-1",
                "task_authorization_window_capability": "cua-window-1"
            }
        }
    }))
    .unwrap() else {
        panic!("expected open_session request");
    };
    grant.validate_identity().unwrap();
    assert_eq!(
        grant.task_authorization_window_capability.as_deref(),
        Some("cua-window-1")
    );
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "terminate_app",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1"
            }
        })),
        Ok(Request::TerminateApp { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "wait_for",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "condition": {
                    "kind": "text_contains",
                    "element_index": 3,
                    "text": "Ready"
                }
            }
        })),
        Ok(Request::WaitFor { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "find",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "query": {"text": "Ready", "max_results": 3}
            }
        })),
        Ok(Request::Find { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "browser_snapshot",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {"snapshot_format": "semantic_v2"}
            }
        })),
        Ok(Request::BrowserSnapshot { request, .. })
            if request.scope_ancestor_role.is_none()
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "browser_snapshot",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {
                    "snapshot_format": "semantic_v2",
                    "scope_ref": "p1:7",
                    "scope_ancestor_role": "row",
                    "query": "View release options"
                }
            }
        })),
        Ok(Request::BrowserSnapshot { request, .. })
            if request.scope_ref.as_deref() == Some("p1:7")
                && request.scope_ancestor_role.as_deref() == Some("row")
                && request.query.as_deref() == Some("View release options")
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "clipboard_write",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "write": {"text": "hello"}
            }
        })),
        Ok(Request::ClipboardWrite { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "clipboard_capture_secret",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "observation_id": "observation-1",
                "secret_handle": "edge.api-key"
            }
        })),
        Ok(Request::ClipboardCaptureSecret { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "recording_start",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {"output_dir": "C:/tmp/cua"}
            }
        })),
        Ok(Request::RecordingStart { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "browser_prepare",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {"allow_launch": false}
            }
        })),
        Ok(Request::BrowserPrepare { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "browser_type",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {
                    "target_id": "target-1",
                    "tab_id": "tab-1",
                    "snapshot_id": "p1",
                    "ref": "p1:2",
                    "text": "Fab"
                }
            }
        })),
        Ok(Request::BrowserType { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "browser_set_input_files",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {
                    "target_id": "target-1",
                    "tab_id": "tab-1",
                    "snapshot_id": "p1",
                    "ref": "p1:4",
                    "files": ["C:/tmp/input.fbx"]
                }
            }
        })),
        Ok(Request::BrowserSetInputFiles { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "browser_download",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {
                    "target_id": "target-1",
                    "tab_id": "tab-1",
                    "snapshot_id": "p1",
                    "ref": "p1:5",
                    "destination_root": "C:/tmp/downloads"
                }
            }
        })),
        Ok(Request::BrowserDownload { .. })
    ));
    assert!(matches!(
        serde_json::from_value::<Request>(json!({
            "method": "browser_dialog",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "task-1",
                "window_capability": "cap-1",
                "request": {
                    "target_id": "target-1",
                    "tab_id": "tab-1",
                    "action": "inspect"
                }
            }
        })),
        Ok(Request::BrowserDialog { .. })
    ));
}
