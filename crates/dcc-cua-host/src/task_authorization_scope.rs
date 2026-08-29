use std::collections::BTreeSet;

use crate::task_authorization::{TaskAuthorizationBinding, issue_task_authorization};
use crate::{
    ConfirmationWindowIdentity, ConnectionSessions, HostError, HostProtocolErrorCode,
    HostSecurityServices, Request, TaskGrant, TrustedTaskAuthorizationLease,
    session_with_capability,
};

pub(crate) async fn preauthorize_task_session(
    security_services: &HostSecurityServices,
    connection_id: &str,
    grant: &TaskGrant,
    session_id: &str,
    window_capability: &str,
    activate_before: bool,
) -> Result<Option<TrustedTaskAuthorizationLease>, HostError> {
    grant.reject_task_authorization_activation(activate_before)?;
    let Some(target) = grant.task_authorization_preflight_target()? else {
        return Ok(None);
    };
    let authorization_id = grant.task_authorization_id.as_deref().ok_or_else(|| {
        HostError::coded_protocol(
            HostProtocolErrorCode::TaskAuthorizationRequired,
            "task authorization preflight lost its authorization identity",
        )
    })?;
    let lease = issue_task_authorization(
        security_services.task_authorization_host.as_deref(),
        TaskAuthorizationBinding::window(
            connection_id,
            authorization_id,
            session_id,
            &grant.task_grant_id,
            &grant.application_label,
            window_capability,
            target,
        ),
    )
    .await?;
    validate_grant_against_task_authorization(grant, &lease)?;
    Ok(Some(lease))
}

pub(crate) async fn finalize_task_session_authorization(
    security_services: &HostSecurityServices,
    connection_id: &str,
    grant: &TaskGrant,
    session_id: &str,
    window_capability: &str,
    observed_target: ConfirmationWindowIdentity,
    preauthorized: Option<TrustedTaskAuthorizationLease>,
) -> Result<Option<TrustedTaskAuthorizationLease>, HostError> {
    if let Some(lease) = preauthorized {
        if lease.target_process_id != observed_target.process_id
            || lease.target_window_handle != observed_target.window_handle
        {
            return Err(HostError::coded_protocol(
                HostProtocolErrorCode::TaskAuthorizationDenied,
                "the opened session target does not match the preauthorized exact window",
            ));
        }
        return Ok(Some(lease));
    }
    let Some(authorization_id) = grant.task_authorization_id.as_deref() else {
        return Ok(None);
    };
    let lease = issue_task_authorization(
        security_services.task_authorization_host.as_deref(),
        TaskAuthorizationBinding::window(
            connection_id,
            authorization_id,
            session_id,
            &grant.task_grant_id,
            &grant.application_label,
            window_capability,
            observed_target,
        ),
    )
    .await?;
    validate_grant_against_task_authorization(grant, &lease)?;
    Ok(Some(lease))
}

fn validate_grant_against_task_authorization(
    grant: &TaskGrant,
    lease: &TrustedTaskAuthorizationLease,
) -> Result<(), HostError> {
    let granted_origins = grant
        .allowed_browser_origins
        .iter()
        .collect::<BTreeSet<_>>();
    let authorized_origins = lease
        .allowed_browser_origins
        .iter()
        .collect::<BTreeSet<_>>();
    if granted_origins != authorized_origins {
        return Err(browser_scope_denied(
            "task grant browser origins do not match the trusted authorization",
        ));
    }
    if grant.showcase_output_dir.is_some()
        && !lease
            .allowed_host_methods
            .iter()
            .any(|method| method == "recording_start")
    {
        return Err(browser_scope_denied(
            "automatic showcase recording is outside the trusted task authorization",
        ));
    }
    Ok(())
}

pub(crate) fn is_task_authorizable_host_method(method: &str) -> bool {
    matches!(
        method,
        "browser_extension_status"
            | "browser_extension_call"
            | "terminate_app"
            | "clipboard_read"
            | "clipboard_capture_secret"
            | "clipboard_write"
            | "recording_start"
            | "recording_stop"
            | "recording_state"
            | "live_observation_start"
            | "live_observation_state"
            | "live_observation_stop"
            | "get_window_state"
            | "change_window_state"
            | "set_window_frame"
            | "invoke_menu"
            | "snapshot"
            | "zoom"
            | "accessibility_snapshot"
            | "verify_state"
            | "call_tool"
            | "browser_snapshot"
            | "browser_prepare"
            | "browser_navigate"
            | "browser_click"
            | "browser_type"
            | "browser_pointer"
            | "browser_set_input_files"
            | "browser_download"
            | "browser_dialog"
            | "find"
            | "wait_for"
            | "execute_action"
            | "resume_session"
            | "get_session_state"
            | "get_input_state"
            | "session_health"
            | "poll_session_events"
            | "cursor_tool"
            | "escalate_session"
    )
}

pub(crate) fn enforce_task_authorized_method(
    sessions: &mut ConnectionSessions,
    request: &Request,
) -> Result<(), HostError> {
    let Some((session_id, task_grant_id, window_capability, method)) =
        request.window_method_scope()
    else {
        return Ok(());
    };
    let host = session_with_capability(
        &mut sessions.windows,
        session_id,
        task_grant_id,
        window_capability,
    )?;
    host.require_task_authorized_method(method)?;
    enforce_task_authorized_browser_scope(host, request)
}

fn enforce_task_authorized_browser_scope(
    host: &crate::HostSession,
    request: &Request,
) -> Result<(), HostError> {
    if host
        .task_authorization
        .as_ref()
        .and_then(|lease| lease.browser_scope.as_ref())
        .is_none()
    {
        return Ok(());
    }
    match request {
        Request::BrowserSnapshot { request, .. } => {
            match (request.target_id.as_deref(), request.tab_id.as_deref()) {
                (Some(target_id), Some(tab_id)) => {
                    host.require_task_authorized_browser_target(target_id, tab_id)
                }
                _ => Err(browser_scope_denied(
                    "an exact task-authorized browser snapshot requires target_id and tab_id",
                )),
            }
        }
        Request::BrowserPrepare { .. } => Err(browser_scope_denied(
            "browser_prepare is outside a final exact-browser task authorization",
        )),
        Request::BrowserNavigate { request, .. } => {
            host.require_task_authorized_browser_target(&request.target_id, &request.tab_id)?;
            host.require_current_task_authorized_browser_document()
        }
        Request::BrowserClick { request, .. } => host.require_task_authorized_browser_document(
            &request.target_id,
            &request.tab_id,
            &request.snapshot_id,
        ),
        Request::BrowserType { request, .. } => host.require_task_authorized_browser_document(
            &request.target_id,
            &request.tab_id,
            request.snapshot_id(),
        ),
        Request::BrowserPointer { request, .. } => host.require_task_authorized_browser_document(
            &request.target_id,
            &request.tab_id,
            &request.snapshot_id,
        ),
        Request::BrowserSetInputFiles { request, .. } => host
            .require_task_authorized_browser_document(
                &request.target_id,
                &request.tab_id,
                &request.snapshot_id,
            ),
        Request::BrowserDownload { request, .. } => host.require_task_authorized_browser_document(
            &request.target_id,
            &request.tab_id,
            &request.snapshot_id,
        ),
        Request::BrowserDialog { request, .. } => {
            host.require_task_authorized_browser_target(&request.target_id, &request.tab_id)?;
            host.require_current_task_authorized_browser_document()
        }
        Request::Hello(_)
        | Request::Ping {}
        | Request::Doctor {}
        | Request::RegisterBrowserExtension { .. }
        | Request::BrowserExtensionNext { .. }
        | Request::CompleteBrowserExtension { .. }
        | Request::UnregisterBrowserExtension { .. }
        | Request::BrowserExtensionStatus { .. }
        | Request::BrowserExtensionCall { .. }
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
        | Request::TerminateApp { .. }
        | Request::ClipboardRead { .. }
        | Request::ClipboardCaptureSecret { .. }
        | Request::ClipboardWrite { .. }
        | Request::RecordingStart { .. }
        | Request::RecordingStop { .. }
        | Request::RecordingState { .. }
        | Request::LiveObservationStart { .. }
        | Request::LiveObservationState { .. }
        | Request::LiveObservationStop { .. }
        | Request::OpenSession { .. }
        | Request::GetWindowState { .. }
        | Request::ChangeWindowState { .. }
        | Request::SetWindowFrame { .. }
        | Request::InvokeMenu { .. }
        | Request::Snapshot { .. }
        | Request::Zoom { .. }
        | Request::AccessibilitySnapshot { .. }
        | Request::VerifyState { .. }
        | Request::CallTool { .. }
        | Request::CallGlobalTool { .. }
        | Request::Find { .. }
        | Request::WaitFor { .. }
        | Request::ExecuteAction { .. }
        | Request::ResumeSession { .. }
        | Request::GetSessionState { .. }
        | Request::GetInputState { .. }
        | Request::SessionHealth { .. }
        | Request::PollSessionEvents { .. }
        | Request::CursorTool { .. }
        | Request::EscalateSession { .. }
        | Request::StopSession { .. }
        | Request::Cancel { .. }
        | Request::CancelWindowWait { .. } => Ok(()),
    }
}

fn browser_scope_denied(message: &str) -> HostError {
    HostError::coded_protocol(HostProtocolErrorCode::TaskAuthorizationDenied, message)
}

impl Request {
    fn window_method_scope(&self) -> Option<(&str, &str, &str, &'static str)> {
        let scope = match self {
            Self::TerminateApp {
                session_id,
                task_grant_id,
                window_capability,
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "terminate_app",
            ),
            Self::ClipboardRead {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "clipboard_read",
            ),
            Self::ClipboardCaptureSecret {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "clipboard_capture_secret",
            ),
            Self::ClipboardWrite {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "clipboard_write",
            ),
            Self::RecordingStart {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "recording_start",
            ),
            Self::RecordingStop {
                session_id,
                task_grant_id,
                window_capability,
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "recording_stop",
            ),
            Self::RecordingState {
                session_id,
                task_grant_id,
                window_capability,
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "recording_state",
            ),
            Self::LiveObservationStart {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "live_observation_start",
            ),
            Self::LiveObservationState {
                session_id,
                task_grant_id,
                window_capability,
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "live_observation_state",
            ),
            Self::LiveObservationStop {
                session_id,
                task_grant_id,
                window_capability,
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "live_observation_stop",
            ),
            Self::GetWindowState {
                session_id,
                task_grant_id,
                window_capability,
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "get_window_state",
            ),
            Self::ChangeWindowState {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "change_window_state",
            ),
            Self::SetWindowFrame {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "set_window_frame",
            ),
            Self::InvokeMenu {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (session_id, task_grant_id, window_capability, "invoke_menu"),
            Self::Snapshot {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (session_id, task_grant_id, window_capability, "snapshot"),
            Self::Zoom {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (session_id, task_grant_id, window_capability, "zoom"),
            Self::AccessibilitySnapshot {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "accessibility_snapshot",
            ),
            Self::VerifyState {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (session_id, task_grant_id, window_capability, "verify_state"),
            Self::CallTool {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (session_id, task_grant_id, window_capability, "call_tool"),
            Self::BrowserSnapshot {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "browser_snapshot",
            ),
            Self::BrowserPrepare {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "browser_prepare",
            ),
            Self::BrowserNavigate {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "browser_navigate",
            ),
            Self::BrowserClick {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "browser_click",
            ),
            Self::BrowserType {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (session_id, task_grant_id, window_capability, "browser_type"),
            Self::BrowserPointer {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "browser_pointer",
            ),
            Self::BrowserSetInputFiles {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "browser_set_input_files",
            ),
            Self::BrowserDownload {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "browser_download",
            ),
            Self::BrowserDialog {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "browser_dialog",
            ),
            Self::Find {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (session_id, task_grant_id, window_capability, "find"),
            Self::WaitFor {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (session_id, task_grant_id, window_capability, "wait_for"),
            Self::ExecuteAction {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "execute_action",
            ),
            Self::ResumeSession {
                session_id,
                task_grant_id,
                window_capability,
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "resume_session",
            ),
            Self::GetSessionState {
                session_id,
                task_grant_id,
                window_capability,
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "get_session_state",
            ),
            Self::GetInputState {
                session_id,
                task_grant_id,
                window_capability,
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "get_input_state",
            ),
            Self::SessionHealth {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "session_health",
            ),
            Self::PollSessionEvents {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "poll_session_events",
            ),
            Self::CursorTool {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (session_id, task_grant_id, window_capability, "cursor_tool"),
            Self::EscalateSession {
                session_id,
                task_grant_id,
                window_capability,
                ..
            } => (
                session_id,
                task_grant_id,
                window_capability,
                "escalate_session",
            ),
            Self::Hello(_)
            | Self::Ping {}
            | Self::Doctor {}
            | Self::RegisterBrowserExtension { .. }
            | Self::BrowserExtensionNext { .. }
            | Self::CompleteBrowserExtension { .. }
            | Self::UnregisterBrowserExtension { .. }
            | Self::BrowserExtensionStatus { .. }
            | Self::BrowserExtensionCall { .. }
            | Self::InterruptAll {}
            | Self::ListApps {}
            | Self::ListTools {}
            | Self::ListWindows { .. }
            | Self::WaitForWindow(_)
            | Self::DesktopSnapshot {}
            | Self::ScreenSize {}
            | Self::CursorPosition {}
            | Self::OpenDesktopSession { .. }
            | Self::DesktopSessionSnapshot { .. }
            | Self::ExecuteDesktopAction { .. }
            | Self::StopDesktopSession { .. }
            | Self::LaunchApp { .. }
            | Self::OpenSession { .. }
            | Self::CallGlobalTool { .. }
            | Self::StopSession { .. }
            | Self::Cancel { .. }
            | Self::CancelWindowWait { .. } => return None,
        };
        Some((scope.0, scope.1, scope.2, scope.3))
    }
}
