//! Bounded semantic waits that share one absolute request deadline.

use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WaitProbeOutcome<T> {
    Cancelled,
    TimedOut,
    Completed(T),
}

pub(super) async fn wait_for_probe_until<T>(
    cancellation: &CancellationHandle,
    deadline: tokio::time::Instant,
    probe: impl std::future::Future<Output = T>,
) -> WaitProbeOutcome<T> {
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => WaitProbeOutcome::Cancelled,
        _ = tokio::time::sleep_until(deadline) => WaitProbeOutcome::TimedOut,
        result = probe => WaitProbeOutcome::Completed(result),
    }
}

pub(super) async fn handle_wait_for(
    host: &mut HostSession,
    cancellation: &CancellationHandle,
    session_id: &str,
    condition: &WaitCondition,
    timeout_ms: u64,
    interval_ms: u64,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let started = Instant::now();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        ensure_session_not_interrupted(host).await?;
        let root = match wait_for_probe_until(
            cancellation,
            deadline,
            host.session.accessibility_snapshot(5_000, 25),
        )
        .await
        {
            WaitProbeOutcome::Cancelled => {
                host.abandon_wait_probe();
                return Ok((cancelled_response(session_id, started), None));
            }
            WaitProbeOutcome::TimedOut => {
                host.abandon_wait_probe();
                return Ok((timeout_response(session_id, condition, started), None));
            }
            WaitProbeOutcome::Completed(result) => result,
        };
        let root = host.finish_observation_sensitive_attempt(root)?;
        ensure_session_not_interrupted(host).await?;
        if wait_condition_matches(&root, condition) {
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
        match wait_for_probe_until(
            cancellation,
            deadline,
            tokio::time::sleep(std::time::Duration::from_millis(interval_ms)),
        )
        .await
        {
            WaitProbeOutcome::Cancelled => {
                return Ok((cancelled_response(session_id, started), None));
            }
            WaitProbeOutcome::TimedOut => {
                return Ok((timeout_response(session_id, condition, started), None));
            }
            WaitProbeOutcome::Completed(()) => {}
        }
    }
}

fn cancelled_response(session_id: &str, started: Instant) -> Value {
    json!({
        "type":"wait_cancelled",
        "success":false,
        "session_id":session_id,
        "error_code":"cancelled",
        "elapsed_ms":started.elapsed().as_millis(),
    })
}

fn timeout_response(session_id: &str, condition: &WaitCondition, started: Instant) -> Value {
    json!({
        "type":"wait_completed",
        "success":false,
        "session_id":session_id,
        "condition":condition.kind,
        "error_code":"timeout",
        "elapsed_ms":started.elapsed().as_millis(),
    })
}
