use std::collections::BTreeSet;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RecordingIssue {
    TrajectoryLeaseLost,
    OwnerMismatch,
    VideoStopped,
}

impl RecordingIssue {
    fn as_str(self) -> &'static str {
        match self {
            Self::TrajectoryLeaseLost => "trajectory_lease_lost",
            Self::OwnerMismatch => "owner_mismatch",
            Self::VideoStopped => "video_stopped",
        }
    }
}

#[derive(Clone)]
pub(crate) struct RecordingHealth {
    expected_owner: Arc<str>,
    issues: Arc<Mutex<BTreeSet<RecordingIssue>>>,
}

impl RecordingHealth {
    pub(crate) fn new(expected_owner: impl Into<Arc<str>>) -> Self {
        Self {
            expected_owner: expected_owner.into(),
            issues: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    pub(crate) fn observe_trajectory(&self, trajectory: &Value) -> bool {
        let trajectory = trajectory.get("structuredContent").unwrap_or(trajectory);
        let mut issues = self.lock_issues();
        let enabled = trajectory.get("enabled").and_then(Value::as_bool) == Some(true);
        let owned = trajectory.get("owner").and_then(Value::as_str) == Some(&self.expected_owner);
        if !enabled {
            issues.insert(RecordingIssue::TrajectoryLeaseLost);
        }
        if !owned {
            issues.insert(RecordingIssue::OwnerMismatch);
        }
        enabled && owned
    }

    pub(crate) fn observe_video(&self, video: Option<&Value>, expected: bool) {
        if expected && video.and_then(|state| state.get("active")) != Some(&Value::Bool(true)) {
            self.lock_issues().insert(RecordingIssue::VideoStopped);
        }
    }

    pub(crate) fn issue_names(&self) -> Vec<&'static str> {
        self.lock_issues()
            .iter()
            .copied()
            .map(RecordingIssue::as_str)
            .collect()
    }

    fn lock_issues(&self) -> MutexGuard<'_, BTreeSet<RecordingIssue>> {
        self.issues
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub(crate) struct RecordingKeepalive {
    cancel: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl RecordingKeepalive {
    pub(crate) fn spawn<Probe, ProbeFuture, ProbeError>(
        session_id: String,
        period: Duration,
        health: RecordingHealth,
        mut probe: Probe,
    ) -> Self
    where
        Probe: FnMut(String) -> ProbeFuture + Send + 'static,
        ProbeFuture: Future<Output = Result<Value, ProbeError>> + Send + 'static,
        ProbeError: Send + 'static,
    {
        let (cancel, cancellation) = oneshot::channel();
        let worker = tokio::spawn(async move {
            let mut cancellation = Box::pin(cancellation);
            let mut interval = tokio::time::interval_at(Instant::now() + period, period);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            'keepalive: loop {
                tokio::select! {
                    biased;
                    _ = &mut cancellation => break,
                    _ = interval.tick() => {
                        let future = probe(session_id.clone());
                        tokio::pin!(future);
                        let result = tokio::select! {
                            biased;
                            _ = &mut cancellation => break 'keepalive,
                            result = &mut future => result,
                        };
                        if let Ok(trajectory) = result
                            && !health.observe_trajectory(&trajectory)
                        {
                            break;
                        }
                    }
                }
            }
        });
        Self {
            cancel: Some(cancel),
            worker: Some(worker),
        }
    }

    pub(crate) async fn stop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.await;
        }
    }

    fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
}

impl Drop for RecordingKeepalive {
    fn drop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}

pub(crate) fn aggregate_recording_state(
    recording_active: bool,
    expected_video: bool,
    trajectory: &Value,
    video: Option<&Value>,
    issues: &[&str],
) -> Value {
    let status = if !recording_active {
        "stopped"
    } else if issues.is_empty() {
        "active"
    } else {
        "degraded"
    };
    let expected_components = if expected_video {
        json!(["trajectory", "video"])
    } else {
        json!(["trajectory"])
    };

    json!({
        "status": status,
        "healthy": issues.is_empty(),
        "expected_components": expected_components,
        "issues": issues,
        "trajectory": trajectory,
        "video": video,
    })
}
