//! Opt-in, content-free measurements of work inside one Host request.
//!
//! These measurements are diagnostic only. A returned request or phase does not
//! establish that input was sent or that the requested application effect
//! occurred. Client transport, work before this wrapper is polled, and model
//! round trips are outside this clock.

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Instant;

use serde::Serialize;

use crate::HostError;

/// The versioned, content-free Host request timing schema.
pub const HOST_TIMING_SCHEMA: &str = "dcc-cua.host-request-timing.v1";

/// Fixed measurement boundaries; no request-supplied labels are accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostTimingPhase {
    SessionPreparation,
    RawInputQueue,
    Snapshot,
    NativeAction,
    PostActionWait,
    PostObservation,
    ConditionWait,
    Verification,
    SessionCleanup,
}

const PHASES: [HostTimingPhase; 9] = [
    HostTimingPhase::SessionPreparation,
    HostTimingPhase::RawInputQueue,
    HostTimingPhase::Snapshot,
    HostTimingPhase::NativeAction,
    HostTimingPhase::PostActionWait,
    HostTimingPhase::PostObservation,
    HostTimingPhase::ConditionWait,
    HostTimingPhase::Verification,
    HostTimingPhase::SessionCleanup,
];

/// A closed classification, independent of arbitrary method or action strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostTimingRequestKind {
    Handshake,
    OpenSession,
    Snapshot,
    ExecuteAction,
    Wait,
    Verification,
    StopSession,
    Other,
}

/// How the measured handler future ended, rather than an application outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostTimingOutcome {
    Returned,
    Error,
    Dropped,
}

/// Bounded aggregation for one phase.
///
/// Count and total_ns include only futures that returned, including Err.
/// A future destroyed before returning increments only dropped_count; its
/// unfinished duration is not mixed into the returned-phase mean. An absent
/// phase is omitted entirely. Nested or overlapping phases are not additive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HostTimingStageSummary {
    pub phase: HostTimingPhase,
    pub count: u64,
    pub total_ns: u64,
    pub dropped_count: u64,
}

/// One request's fixed-schema diagnostic sample.
///
/// Sequence is process-local diagnostic correlation, not an identity or
/// authorization token. Elapsed_ns starts when the enabled wrapper is first
/// polled and ends after its inner future has been destroyed, before delivery
/// to the observer. At most nine stage summaries can be emitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HostTimingSample {
    pub schema: &'static str,
    pub sequence: u64,
    pub kind: HostTimingRequestKind,
    pub outcome: HostTimingOutcome,
    pub elapsed_ns: u64,
    pub stages: Vec<HostTimingStageSummary>,
}

/// Constructor-owned, optional diagnostic delivery.
///
/// Implementations must be nonblocking and must not panic. A bounded sink
/// should discard excess samples rather than delay a request. The Host calls
/// this after destroying the inner request future and its guards, without
/// holding the timing collector's mutex. An observer panic is contained in
/// unwind builds and does not replace the original handler result; the global
/// panic hook is not changed. This observer never grants authority and must
/// not trigger retries, additional observations, or application actions.
pub trait HostTimingObserver: Send + Sync {
    fn record(&self, sample: HostTimingSample);
}

#[derive(Clone, Copy, Default)]
struct PhaseTotals {
    count: u64,
    total_ns: u64,
    dropped_count: u64,
}

#[derive(Default)]
struct Collector {
    phases: [PhaseTotals; PHASES.len()],
}

impl Collector {
    fn record(&mut self, phase: HostTimingPhase, returned_ns: Option<u64>) {
        let totals = &mut self.phases[phase as usize];
        if let Some(elapsed_ns) = returned_ns {
            totals.count = totals.count.saturating_add(1);
            totals.total_ns = totals.total_ns.saturating_add(elapsed_ns);
        } else {
            totals.dropped_count = totals.dropped_count.saturating_add(1);
        }
    }

    fn summaries(&self) -> Vec<HostTimingStageSummary> {
        PHASES
            .iter()
            .zip(self.phases)
            .filter(|(_, totals)| totals.count != 0 || totals.dropped_count != 0)
            .map(|(&phase, totals)| HostTimingStageSummary {
                phase,
                count: totals.count,
                total_ns: totals.total_ns,
                dropped_count: totals.dropped_count,
            })
            .collect()
    }
}

type SharedCollector = Arc<Mutex<Collector>>;

tokio::task_local! {
    // An explicit None also masks an outer enabled request in a nested
    // disabled wrapper. Tokio spawned tasks do not inherit this context.
    static ACTIVE_TIMING: Option<SharedCollector>;
}

static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn now() -> Instant {
    Instant::now()
}

fn elapsed_ns(started: Instant) -> u64 {
    now()
        .saturating_duration_since(started)
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64
}

struct PhaseGuard {
    collector: SharedCollector,
    phase: HostTimingPhase,
    started: Instant,
    returned: bool,
}

impl Drop for PhaseGuard {
    fn drop(&mut self) {
        let returned_ns = self.returned.then(|| elapsed_ns(self.started));
        self.collector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record(self.phase, returned_ns);
    }
}

/// Measure a phase only when this task is inside an enabled request.
pub(crate) async fn measure_phase<F: Future>(phase: HostTimingPhase, operation: F) -> F::Output {
    let collector = ACTIVE_TIMING.try_with(Clone::clone).ok().flatten();
    let Some(collector) = collector else {
        return operation.await;
    };
    let mut guard = PhaseGuard {
        collector,
        phase,
        started: now(),
        returned: false,
    };
    let result = operation.await;
    guard.returned = true;
    result
}

// The explicit wrapper owns the scoped future so completion and cancellation
// both destroy that future before snapshotting or invoking an observer.
struct MeasuredRequest<F> {
    operation: Option<Pin<Box<F>>>,
    collector: SharedCollector,
    observer: Option<Arc<dyn HostTimingObserver>>,
    sequence: u64,
    kind: HostTimingRequestKind,
    started: Instant,
    outcome: HostTimingOutcome,
}

impl<F> MeasuredRequest<F> {
    fn publish(&mut self) {
        let Some(observer) = self.observer.take() else {
            return;
        };
        let elapsed_ns = elapsed_ns(self.started);
        let stages = self
            .collector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .summaries();
        let sample = HostTimingSample {
            schema: HOST_TIMING_SCHEMA,
            sequence: self.sequence,
            kind: self.kind,
            outcome: self.outcome,
            elapsed_ns,
            stages,
        };
        let _ = catch_unwind(AssertUnwindSafe(|| observer.record(sample)));
    }
}

impl<F, T> Future for MeasuredRequest<F>
where
    F: Future<Output = Result<T, HostError>>,
{
    type Output = Result<T, HostError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let result = match this
            .operation
            .as_mut()
            .expect("a completed measured request cannot be polled again")
            .as_mut()
            .poll(context)
        {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(result) => result,
        };
        this.outcome = if result.is_ok() {
            HostTimingOutcome::Returned
        } else {
            HostTimingOutcome::Error
        };
        drop(this.operation.take());
        this.publish();
        Poll::Ready(result)
    }
}

impl<F> Drop for MeasuredRequest<F> {
    fn drop(&mut self) {
        drop(self.operation.take());
        self.publish();
    }
}

/// Measure one handler without changing its result or completion semantics.
///
/// This wrapper's disabled branch adds no collector or boxed future and reads
/// no timing clock. Callers may separately box their operation. Once polled,
/// enabled requests publish once, including when their
/// future is dropped. An unpolled request has no sample. The caller must
/// establish a separate scope for each spawned
/// request; context is not inherited by tokio::spawn.
pub(crate) async fn measure_request<F, T>(
    observer: Option<Arc<dyn HostTimingObserver>>,
    kind: HostTimingRequestKind,
    operation: F,
) -> Result<T, HostError>
where
    F: Future<Output = Result<T, HostError>>,
{
    let Some(observer) = observer else {
        return ACTIVE_TIMING.scope(None, operation).await;
    };
    let started = now();
    let collector = Arc::new(Mutex::new(Collector::default()));
    let operation = ACTIVE_TIMING.scope(Some(collector.clone()), operation);
    MeasuredRequest {
        operation: Some(Box::pin(operation)),
        collector,
        observer: Some(observer),
        sequence: NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        kind,
        started,
        outcome: HostTimingOutcome::Dropped,
    }
    .await
}

#[cfg(test)]
mod tests;
