//! Content-free request classification and the outer Host handling boundary.
use super::*;
use crate::timing::{HostTimingRequestKind as Kind, measure_request};

pub(crate) async fn handle_request_with_security_services(
    driver: &ComputerUseDriver,
    security_services: &HostSecurityServices,
    sessions: &mut ConnectionSessions,
    snapshot_transport: &mut Option<SnapshotTransport>,
    desktop_shared_image: &mut Option<SharedImage>,
    cancellation_registry: &CancellationRegistry,
    request: Request,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let kind = match &request {
        Request::Hello(_) => Kind::Handshake,
        Request::OpenSession { .. } | Request::OpenDesktopSession { .. } => Kind::OpenSession,
        Request::Snapshot { .. }
        | Request::AccessibilitySnapshot { .. }
        | Request::DesktopSessionSnapshot { .. } => Kind::Snapshot,
        Request::ExecuteAction { .. } | Request::ExecuteDesktopAction { .. } => Kind::ExecuteAction,
        Request::WaitFor { .. } | Request::WaitForWindow(_) => Kind::Wait,
        Request::VerifyState { .. } => Kind::Verification,
        Request::StopSession { .. } | Request::StopDesktopSession { .. } => Kind::StopSession,
        _ => Kind::Other,
    };
    measure_request(security_services.timing_observer.clone(), kind, async {
        let evidence_route = window_evidence_epoch_route(&request);
        prepare_window_evidence_request(sessions, evidence_route.as_ref());
        // Bound the inline state of both enabled and disabled timing wrappers.
        let result = Box::pin(handle_request_inner(
            driver,
            security_services,
            sessions,
            snapshot_transport,
            desktop_shared_image,
            cancellation_registry,
            request,
        ))
        .await;
        finish_window_evidence_request(sessions, evidence_route, result)
    })
    .await
}
