use std::collections::BTreeSet;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

use super::{
    ComputerUseDriver, ComputerUseSession, RECORDING_KEEPALIVE_INTERVAL, call_driver_tool,
    ensure_tool_ok,
};
use crate::{ComputerUseError, ComputerUseErrorCode, ComputerUseResult};

pub(crate) async fn probe_recording_state(
    driver: &ComputerUseDriver,
    session_id: &str,
) -> ComputerUseResult<Value> {
    call_recording_tool_without_refresh(
        driver,
        session_id,
        "get_recording_state",
        "probe CUA recording state",
    )
    .await
}

pub(crate) async fn call_recording_tool_without_refresh(
    driver: &ComputerUseDriver,
    session_id: &str,
    tool: &str,
    operation: &str,
) -> ComputerUseResult<Value> {
    let result = call_driver_tool(
        &driver.driver,
        tool,
        json!({"session": session_id}).to_string(),
        operation,
    )
    .await?;
    ensure_tool_ok(operation, &result)?;
    serde_json::from_str(&result.raw_json).map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            format!("CUA {tool} returned invalid JSON: {error}"),
        )
    })
}

#[derive(Clone, Debug)]
pub(crate) struct RecordingVideoTerminalEvidence {
    state: Value,
}

impl RecordingVideoTerminalEvidence {
    pub(crate) fn try_from_finalized(state: Value) -> ComputerUseResult<Self> {
        if state.get("active").and_then(Value::as_bool) != Some(false)
            || state.get("finalized").and_then(Value::as_bool) != Some(true)
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "showcase stop did not return finalized terminal video evidence",
            ));
        }
        Ok(Self { state })
    }

    pub(crate) const fn state(&self) -> &Value {
        &self.state
    }
}

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

impl ComputerUseSession {
    pub(super) fn start_recording_keepalive(&mut self, expected_video: bool, trajectory: &Value) {
        debug_assert!(self.recording_keepalive.is_none());
        let health = RecordingHealth::new(self.session_id.as_str());
        let lease_is_healthy = health.observe_trajectory(trajectory);
        let driver = self.driver.clone();
        let keepalive = lease_is_healthy.then(|| {
            RecordingKeepalive::spawn(
                self.session_id.clone(),
                RECORDING_KEEPALIVE_INTERVAL,
                health.clone(),
                move |session_id| {
                    let driver = driver.clone();
                    async move { probe_recording_state(&driver, &session_id).await }
                },
            )
        });
        self.recording_expected_video = expected_video;
        self.recording_health = Some(health);
        self.recording_keepalive = keepalive;
    }

    pub(super) async fn stop_recording_keepalive(&mut self) {
        if let Some(mut keepalive) = self.recording_keepalive.take() {
            keepalive.stop().await;
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
    let video_paused = recording_active
        && expected_video
        && video
            .and_then(|state| state.get("active"))
            .and_then(Value::as_bool)
            == Some(true)
        && video
            .and_then(|state| state.get("paused"))
            .and_then(Value::as_bool)
            == Some(true);
    let mut projected_issues = issues.iter().copied().collect::<BTreeSet<_>>();
    if video_paused {
        projected_issues.insert("video_paused");
    }
    let projected_issues = projected_issues.into_iter().collect::<Vec<_>>();
    let status = if !recording_active {
        "stopped"
    } else if !issues.is_empty() {
        "degraded"
    } else if video_paused {
        "paused"
    } else {
        "active"
    };
    let expected_components = if expected_video {
        json!(["trajectory", "video"])
    } else {
        json!(["trajectory"])
    };

    json!({
        "status": status,
        "healthy": projected_issues.is_empty(),
        "expected_components": expected_components,
        "issues": projected_issues,
        "trajectory": trajectory,
        "video": video,
    })
}
