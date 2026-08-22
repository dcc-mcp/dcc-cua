use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowEvidenceEpochRoute {
    pub(crate) session_id: String,
    pub(crate) publication: HostEvidencePublication,
}

pub(crate) fn window_evidence_epoch_route(request: &Request) -> Option<WindowEvidenceEpochRoute> {
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

pub(crate) fn prepare_window_evidence_request(
    sessions: &mut ConnectionSessions,
    route: Option<&WindowEvidenceEpochRoute>,
) {
    if let Some(host) = route.and_then(|route| sessions.windows.get_mut(&route.session_id)) {
        host.synchronize_action_evidence_epoch();
    }
}

pub(crate) fn finish_window_evidence_request<T>(
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
