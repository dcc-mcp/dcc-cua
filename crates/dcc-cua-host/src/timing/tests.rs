use std::collections::HashSet;
use std::future::{pending, poll_fn};
use std::hint::black_box;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use rstest::rstest;
use serde_json::{Value, json};
use tokio::sync::{Barrier, Notify};

use super::*;

#[derive(Default)]
struct RecordingObserver {
    samples: Mutex<Vec<HostTimingSample>>,
}

impl RecordingObserver {
    fn take(&self) -> Vec<HostTimingSample> {
        std::mem::take(&mut *self.samples.lock().unwrap())
    }
}

impl HostTimingObserver for RecordingObserver {
    fn record(&self, sample: HostTimingSample) {
        self.samples.lock().unwrap().push(sample);
    }
}

#[rstest]
#[tokio::test]
async fn disabled_request_and_unscoped_phase_preserve_results_without_a_collector() {
    let result = measure_request(None, HostTimingRequestKind::Other, async {
        assert!(ACTIVE_TIMING.with(Option::is_none));
        measure_phase(HostTimingPhase::NativeAction, async { Ok(17_u8) }).await
    })
    .await
    .unwrap();
    assert_eq!(result, 17);
    assert_eq!(
        measure_phase(HostTimingPhase::Snapshot, async { 23 }).await,
        23
    );
}

#[rstest]
#[tokio::test]
async fn absent_phases_are_omitted_and_returned_errors_are_counted() {
    let observer = Arc::new(RecordingObserver::default());
    let result = measure_request(
        Some(observer.clone()),
        HostTimingRequestKind::ExecuteAction,
        async {
            measure_phase(HostTimingPhase::NativeAction, async {
                Err::<(), _>(HostError::Protocol("synthetic error marker".into()))
            })
            .await
        },
    )
    .await;
    assert!(
        matches!(result, Err(HostError::Protocol(message)) if message == "synthetic error marker")
    );
    let samples = observer.take();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].schema, HOST_TIMING_SCHEMA);
    assert_eq!(samples[0].kind, HostTimingRequestKind::ExecuteAction);
    assert_eq!(samples[0].outcome, HostTimingOutcome::Error);
    assert_eq!(samples[0].stages.len(), 1);
    assert_eq!(samples[0].stages[0].phase, HostTimingPhase::NativeAction);
    assert_eq!(samples[0].stages[0].count, 1);
    assert_eq!(samples[0].stages[0].dropped_count, 0);
}

#[rstest]
#[tokio::test]
async fn returned_handler_preserves_unknown_effect_payload_and_fixed_schema() {
    let observer = Arc::new(RecordingObserver::default());
    let private = "synthetic-sensitive-payload-marker";
    let expected = json!({
        "request_id": private,
        "text": private,
        "path": private,
        "window_title": private,
        "details": {"completion": "unknown", "effect_unknown": true, "blind_retry": false}
    });
    let actual = measure_request(
        Some(observer.clone()),
        HostTimingRequestKind::ExecuteAction,
        async { Ok(expected.clone()) },
    )
    .await
    .unwrap();
    assert_eq!(actual, expected);
    let sample = observer.take().pop().unwrap();
    assert_eq!(sample.outcome, HostTimingOutcome::Returned);
    assert!(sample.stages.is_empty());
    let value = serde_json::to_value(sample).unwrap();
    let keys: HashSet<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        HashSet::from([
            "schema",
            "sequence",
            "kind",
            "outcome",
            "elapsed_ns",
            "stages"
        ])
    );
    assert!(!serde_json::to_string(&value).unwrap().contains(private));
}

struct LockCheckingObserver {
    lock: Arc<tokio::sync::Mutex<()>>,
    released: AtomicBool,
    samples: Mutex<Vec<HostTimingSample>>,
}

impl HostTimingObserver for LockCheckingObserver {
    fn record(&self, sample: HostTimingSample) {
        self.released
            .store(self.lock.try_lock().is_ok(), Ordering::SeqCst);
        self.samples.lock().unwrap().push(sample);
    }
}

struct RetainsGuardAfterReady {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl Future for RetainsGuardAfterReady {
    type Output = Result<u8, HostError>;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(Ok(9))
    }
}

#[rstest]
#[tokio::test]
async fn returned_inner_future_is_destroyed_before_observer_and_publishes_once() {
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    let observer = Arc::new(LockCheckingObserver {
        lock: lock.clone(),
        released: AtomicBool::new(false),
        samples: Mutex::new(Vec::new()),
    });
    let result = measure_request(
        Some(observer.clone()),
        HostTimingRequestKind::Other,
        RetainsGuardAfterReady {
            _guard: lock.lock_owned().await,
        },
    )
    .await;
    assert_eq!(result.unwrap(), 9);
    assert!(observer.released.load(Ordering::SeqCst));
    assert_eq!(observer.samples.lock().unwrap().len(), 1);
}

#[rstest]
#[tokio::test]
async fn aborted_request_destroys_guards_before_reporting_unfinished_phase() {
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    let observer = Arc::new(LockCheckingObserver {
        lock: lock.clone(),
        released: AtomicBool::new(false),
        samples: Mutex::new(Vec::new()),
    });
    let started = Arc::new(Notify::new());
    let task_observer = observer.clone();
    let task_started = started.clone();
    let task = tokio::spawn(async move {
        measure_request(
            Some(task_observer),
            HostTimingRequestKind::ExecuteAction,
            async move {
                let _guard = lock.lock_owned().await;
                measure_phase(HostTimingPhase::NativeAction, async {
                    task_started.notify_one();
                    pending::<Result<(), HostError>>().await
                })
                .await
            },
        )
        .await
    });
    started.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(observer.released.load(Ordering::SeqCst));
    let samples = observer.samples.lock().unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].outcome, HostTimingOutcome::Dropped);
    assert_eq!(samples[0].stages.len(), 1);
    assert_eq!(samples[0].stages[0].count, 0);
    assert_eq!(samples[0].stages[0].total_ns, 0);
    assert_eq!(samples[0].stages[0].dropped_count, 1);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn timeout_records_drop_without_reinterpreting_operation_effects() {
    let observer = Arc::new(RecordingObserver::default());
    let result = tokio::time::timeout(
        Duration::from_millis(5),
        measure_request(Some(observer.clone()), HostTimingRequestKind::Wait, async {
            measure_phase(
                HostTimingPhase::ConditionWait,
                pending::<Result<(), HostError>>(),
            )
            .await
        }),
    )
    .await;
    assert!(result.is_err());
    let sample = observer.take().pop().unwrap();
    assert_eq!(sample.outcome, HostTimingOutcome::Dropped);
    assert_eq!(sample.stages[0].count, 0);
    assert_eq!(sample.stages[0].dropped_count, 1);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_dropped_phase_does_not_turn_a_returned_request_into_an_error() {
    let observer = Arc::new(RecordingObserver::default());
    let result = measure_request(Some(observer.clone()), HostTimingRequestKind::Wait, async {
        let timeout = tokio::time::timeout(
            Duration::from_millis(5),
            measure_phase(HostTimingPhase::ConditionWait, pending::<()>()),
        )
        .await;
        assert!(timeout.is_err());
        Ok(31)
    })
    .await
    .unwrap();
    assert_eq!(result, 31);
    let sample = observer.take().pop().unwrap();
    assert_eq!(sample.outcome, HostTimingOutcome::Returned);
    assert_eq!(sample.stages[0].count, 0);
    assert_eq!(sample.stages[0].dropped_count, 1);
}

#[rstest]
#[tokio::test]
async fn dropping_an_unpolled_request_does_not_create_a_sample() {
    let observer = Arc::new(RecordingObserver::default());
    drop(measure_request(
        Some(observer.clone()),
        HostTimingRequestKind::Other,
        async { Ok(()) },
    ));
    assert!(observer.take().is_empty());
}

#[rstest]
#[tokio::test]
async fn concurrent_requests_have_isolated_fixed_size_collectors_and_sequences() {
    let observer = Arc::new(RecordingObserver::default());
    let barrier = Arc::new(Barrier::new(PHASES.len() + 1));
    let mut tasks = Vec::new();
    for (index, phase) in PHASES.into_iter().enumerate() {
        let observer = observer.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            measure_request(Some(observer), HostTimingRequestKind::Other, async move {
                barrier.wait().await;
                for _ in 0..=index {
                    measure_phase(phase, async { tokio::task::yield_now().await }).await;
                }
                Ok(index)
            })
            .await
        }));
    }
    barrier.wait().await;
    for (index, task) in tasks.into_iter().enumerate() {
        assert_eq!(task.await.unwrap().unwrap(), index);
    }
    let samples = observer.take();
    assert_eq!(samples.len(), PHASES.len());
    let sequences: HashSet<_> = samples.iter().map(|sample| sample.sequence).collect();
    assert_eq!(sequences.len(), PHASES.len());
    let phases: HashSet<_> = samples
        .iter()
        .map(|sample| {
            assert_eq!(sample.stages.len(), 1);
            let stage = &sample.stages[0];
            assert_eq!(stage.count, stage.phase as u64 + 1);
            assert_eq!(stage.dropped_count, 0);
            stage.phase
        })
        .collect();
    assert_eq!(phases.len(), PHASES.len());
}

#[rstest]
#[tokio::test]
async fn nested_enabled_requests_are_isolated_and_disabled_requests_mask_outer_context() {
    let outer = Arc::new(RecordingObserver::default());
    let inner = Arc::new(RecordingObserver::default());
    measure_request(Some(outer.clone()), HostTimingRequestKind::Other, async {
        measure_phase(HostTimingPhase::Snapshot, async {
            measure_request(None, HostTimingRequestKind::ExecuteAction, async {
                measure_phase(HostTimingPhase::NativeAction, async { Ok(()) }).await
            })
            .await
            .unwrap();
            measure_request(Some(inner.clone()), HostTimingRequestKind::Wait, async {
                measure_phase(HostTimingPhase::ConditionWait, async { Ok(()) }).await
            })
            .await
            .unwrap();
        })
        .await;
        Ok(())
    })
    .await
    .unwrap();
    let outer = outer.take().pop().unwrap();
    let inner = inner.take().pop().unwrap();
    assert_eq!(outer.stages.len(), 1);
    assert_eq!(outer.stages[0].phase, HostTimingPhase::Snapshot);
    assert_eq!(inner.stages.len(), 1);
    assert_eq!(inner.stages[0].phase, HostTimingPhase::ConditionWait);
    assert_ne!(outer.sequence, inner.sequence);
}

#[rstest]
#[tokio::test]
async fn collector_poison_does_not_replace_original_result() {
    let observer = Arc::new(RecordingObserver::default());
    let result = measure_request(
        Some(observer.clone()),
        HostTimingRequestKind::Other,
        async {
            let collector = ACTIVE_TIMING.with(|collector| collector.clone().unwrap());
            let _ = catch_unwind(AssertUnwindSafe(|| {
                let _guard = collector.lock().unwrap();
                panic!("synthetic collector poison");
            }));
            measure_phase(HostTimingPhase::Verification, async { Ok(41) }).await
        },
    )
    .await
    .unwrap();
    assert_eq!(result, 41);
    assert_eq!(observer.take()[0].stages[0].count, 1);
}

struct PanickingObserver(AtomicU64);

impl HostTimingObserver for PanickingObserver {
    fn record(&self, _: HostTimingSample) {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("synthetic observer panic");
    }
}

#[rstest]
#[tokio::test]
async fn observer_panic_does_not_replace_success_error_or_drop() {
    let observer = Arc::new(PanickingObserver(AtomicU64::new(0)));
    assert_eq!(
        measure_request(
            Some(observer.clone()),
            HostTimingRequestKind::Other,
            async { Ok(53) }
        )
        .await
        .unwrap(),
        53
    );
    let result = measure_request(
        Some(observer.clone()),
        HostTimingRequestKind::Other,
        async { Err::<(), _>(HostError::Protocol("original failure".into())) },
    )
    .await;
    assert!(matches!(result, Err(HostError::Protocol(message)) if message == "original failure"));
    let mut operation = Box::pin(measure_request(
        Some(observer.clone()),
        HostTimingRequestKind::Other,
        pending::<Result<(), HostError>>(),
    ));
    poll_fn(|context| {
        assert!(operation.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(operation);
    assert_eq!(observer.0.load(Ordering::SeqCst), 3);
}

#[rstest]
fn stage_summaries_are_bounded_saturating_and_distinguish_zero_from_absent() {
    let mut collector = Collector::default();
    assert!(collector.summaries().is_empty());
    collector.record(HostTimingPhase::Snapshot, Some(0));
    assert_eq!(collector.summaries()[0].count, 1);
    assert_eq!(collector.summaries()[0].total_ns, 0);
    for phase in PHASES {
        for _ in 0..32 {
            collector.record(phase, Some(7));
            collector.record(phase, None);
        }
    }
    assert_eq!(collector.summaries().len(), PHASES.len());
    collector.phases[HostTimingPhase::NativeAction as usize] = PhaseTotals {
        count: u64::MAX,
        total_ns: u64::MAX,
        dropped_count: u64::MAX,
    };
    collector.record(HostTimingPhase::NativeAction, Some(1));
    collector.record(HostTimingPhase::NativeAction, None);
    let stages = collector.summaries();
    let saturated = stages
        .iter()
        .find(|stage| stage.phase == HostTimingPhase::NativeAction)
        .unwrap();
    assert_eq!(saturated.count, u64::MAX);
    assert_eq!(saturated.total_ns, u64::MAX);
    assert_eq!(saturated.dropped_count, u64::MAX);
    let encoded = serde_json::to_value(stages).unwrap();
    for stage in encoded.as_array().unwrap() {
        let keys: HashSet<_> = stage
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            HashSet::from(["phase", "count", "total_ns", "dropped_count"])
        );
    }
}

struct NoopObserver;

impl HostTimingObserver for NoopObserver {
    fn record(&self, sample: HostTimingSample) {
        black_box(sample);
    }
}

async fn overhead_batch(observer: Option<Arc<dyn HostTimingObserver>>, iterations: u64) -> u64 {
    let started = Instant::now();
    for _ in 0..iterations {
        measure_request(observer.clone(), HostTimingRequestKind::Other, async {
            measure_phase(HostTimingPhase::NativeAction, async {
                let mut payload: Value = json!({"ok": true, "step": 1, "items": [1, 2, 3]});
                payload["step"] = json!(2);
                black_box(payload);
                Ok(())
            })
            .await
        })
        .await
        .unwrap();
    }
    started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
}

/// Same-binary ABBA paired batches, not independent per-iteration samples.
/// This measures instrumentation cost on fixed synthetic JSON work only. It
/// does not measure desktop latency or establish an end-to-end speedup.
/// A is this version's disabled wrapper, not the earlier unwrapped code, so
/// the probe does not establish zero regression for the disabled path.
#[rstest]
#[ignore = "manual release-only instrumentation overhead probe; no latency threshold"]
#[allow(
    clippy::assertions_on_constants,
    reason = "the ignored probe rejects debug builds when manually invoked"
)]
fn release_instrumentation_overhead_probe() {
    assert!(
        !cfg!(debug_assertions),
        "run this ignored probe in release mode"
    );
    const ITERATIONS: u64 = 128;
    const BLOCKS: usize = 24;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let observer: Arc<dyn HostTimingObserver> = Arc::new(NoopObserver);
    runtime.block_on(async {
        for _ in 0..4 {
            overhead_batch(None, ITERATIONS).await;
            overhead_batch(Some(observer.clone()), ITERATIONS).await;
        }
        let mut raw: Vec<[u64; 4]> = Vec::with_capacity(BLOCKS);
        let mut disabled_total = 0_u128;
        let mut enabled_total = 0_u128;
        let mut paired_total = 0_f64;
        for _ in 0..BLOCKS {
            let a1 = overhead_batch(None, ITERATIONS).await;
            let b1 = overhead_batch(Some(observer.clone()), ITERATIONS).await;
            let b2 = overhead_batch(Some(observer.clone()), ITERATIONS).await;
            let a2 = overhead_batch(None, ITERATIONS).await;
            raw.push([a1, b1, b2, a2]);
            disabled_total += u128::from(a1) + u128::from(a2);
            enabled_total += u128::from(b1) + u128::from(b2);
            paired_total += ((b1 as f64 + b2 as f64) - (a1 as f64 + a2 as f64)) / 2.0;
        }
        // Fixed field names and numeric values only; no paths or runtime data.
        println!(
            "{}",
            json!({
                "batch_iterations": ITERATIONS,
                "abba_blocks": BLOCKS,
                "disabled_batch_mean_ns": disabled_total as f64 / (2 * BLOCKS) as f64,
                "enabled_batch_mean_ns": enabled_total as f64 / (2 * BLOCKS) as f64,
                "paired_batch_delta_mean_ns": paired_total / BLOCKS as f64,
                "raw_batch_ns_abba": raw
            })
        );
    });
}
