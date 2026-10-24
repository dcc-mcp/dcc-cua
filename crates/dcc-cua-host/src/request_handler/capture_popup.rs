//! Snapshot orchestration for independently authorized secondary roots.
use super::*;

pub(super) async fn capture_snapshot(
    host: &mut HostSession,
    session_id: &str,
    selected: &[u64],
    capture_diagnostics: bool,
    max_nodes: u32,
    max_depth: u32,
) -> Result<(ComputerUseScreenshot, Vec<ComputerUseScreenshot>, Value), HostError> {
    let screenshots = if !selected.is_empty() {
        let expires = host.require_native_capture_popup_keyboard_grant(session_id)?;
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_millis(
                expires.saturating_sub(crate::task_authorization::unix_time_millis()),
            );
        host.session
            .screenshot_native_capture_popups(selected, capture_diagnostics, deadline)
            .await
    } else if host.observation_mode == TaskObservationMode::PixelsOnly {
        host.session
            .screenshot_pixels_only_with_diagnostics(capture_diagnostics)
            .await
            .map(|root| (root, Vec::new(), Value::Null))
    } else {
        host.session
            .screenshot_with_bounds(max_nodes, max_depth)
            .await
            .map(|root| (root, Vec::new(), Value::Null))
    };
    let result = host.finish_observation_sensitive_attempt(screenshots)?;
    Ok(result)
}
