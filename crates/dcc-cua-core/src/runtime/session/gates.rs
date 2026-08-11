use super::*;

pub(crate) async fn gated_desktop_observation<T, Operation, Capture>(
    availability: ComputerUseResult<()>,
    capture: Operation,
) -> ComputerUseResult<T>
where
    Operation: FnOnce() -> Capture,
    Capture: Future<Output = ComputerUseResult<T>>,
{
    availability?;
    capture().await
}

pub(crate) async fn gated_upstream_session_refresh<T, Operation, Refresh>(
    input_availability: ComputerUseResult<()>,
    refresh: Operation,
) -> ComputerUseResult<T>
where
    Operation: FnOnce() -> Refresh,
    Refresh: Future<Output = ComputerUseResult<T>>,
{
    input_availability?;
    refresh().await
}

#[allow(dead_code)]
pub(crate) async fn gated_exact_window_observation<T, Check, Operation, Capture>(
    mut check: Check,
    operation: Operation,
) -> ComputerUseResult<T>
where
    Check: FnMut() -> ComputerUseResult<()>,
    Operation: FnOnce() -> Capture,
    Capture: Future<Output = ComputerUseResult<T>>,
{
    check()?;
    let result = operation().await?;
    check()?;
    Ok(result)
}

#[allow(dead_code)]
pub(crate) async fn gated_exact_window_publication<
    Captured,
    Finalized,
    Published,
    Check,
    Capture,
    CaptureOutput,
    Finalize,
    FinalizeOutput,
    Publish,
>(
    mut check: Check,
    capture: Capture,
    finalize: Finalize,
    publish: Publish,
) -> ComputerUseResult<Published>
where
    Check: FnMut() -> ComputerUseResult<()>,
    Capture: FnOnce() -> CaptureOutput,
    CaptureOutput: Future<Output = ComputerUseResult<Captured>>,
    Finalize: FnOnce(Captured) -> FinalizeOutput,
    FinalizeOutput: Future<Output = ComputerUseResult<Finalized>>,
    Publish: FnOnce(Finalized) -> Published,
{
    check()?;
    let captured = capture().await?;
    check()?;
    let finalized = finalize(captured).await?;
    check()?;
    Ok(publish(finalized))
}

pub(crate) async fn gated_cursor_operation<T, CheckInput, Operation, Output>(
    moves_cursor: bool,
    check_input: CheckInput,
    operation: Operation,
) -> ComputerUseResult<T>
where
    CheckInput: FnOnce() -> ComputerUseResult<()>,
    Operation: FnOnce() -> Output,
    Output: Future<Output = ComputerUseResult<T>>,
{
    if moves_cursor {
        check_input()?;
    }
    operation().await
}

pub(crate) fn ensure_target_available_for_action(target: &WindowTarget) -> ComputerUseResult<()> {
    if target.is_minimized {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetMinimized,
            "target_minimized: automatic_input=false; issue the explicit restore_activate window operation, then take a fresh observation before retrying",
        ));
    }
    if !target.is_on_screen {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetUnavailable,
            "target_unavailable: automatic_input=false; wait for a typed target_available event and take a fresh observation before retrying",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BrowserToolDisposition {
    ReadOnlyEvidence,
    PotentialMutation,
}

pub(super) fn browser_tool_route(name: &str, arguments: &Value) -> Option<BrowserToolDisposition> {
    match name {
        "get_browser_state" => Some(BrowserToolDisposition::ReadOnlyEvidence),
        "browser_dialog" => match arguments["action"].as_str() {
            Some("inspect") => Some(BrowserToolDisposition::ReadOnlyEvidence),
            Some("accept" | "dismiss") => Some(BrowserToolDisposition::PotentialMutation),
            _ => None,
        },
        "browser_prepare"
        | "browser_navigate"
        | "browser_click"
        | "browser_type"
        | "browser_pointer"
        | "browser_set_input_files" => Some(BrowserToolDisposition::PotentialMutation),
        _ => None,
    }
}

pub(super) fn browser_tool_requires_input(name: &str, arguments: &Value) -> bool {
    browser_tool_route(name, arguments) == Some(BrowserToolDisposition::PotentialMutation)
}

#[cfg(any(windows, test))]
pub(crate) fn run_preinvalidated_window_mutation<T, E>(
    invalidate: impl FnOnce(),
    mutation: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    invalidate();
    mutation()
}

#[cfg(any(windows, test))]
pub(crate) fn run_gated_preinvalidated_window_mutation<T, E>(
    gate: impl FnOnce() -> Result<(), E>,
    invalidate: impl FnOnce(),
    mutation: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    gate()?;
    run_preinvalidated_window_mutation(invalidate, mutation)
}

#[allow(dead_code)]
pub(crate) async fn preflight_live_observation_start<T, Revalidate, Output>(
    existing_state: Option<&Value>,
    observation_availability: ComputerUseResult<()>,
    revalidate_target: Revalidate,
) -> ComputerUseResult<(LiveObservationStartDisposition, T)>
where
    Revalidate: FnOnce() -> Output,
    Output: Future<Output = ComputerUseResult<T>>,
{
    observation_availability?;
    let target = revalidate_target().await?;
    Ok((live_observation_start_disposition(existing_state), target))
}
