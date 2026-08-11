use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ActionEvidenceEpoch, ComputerUseInputStatus, ComputerUseSessionInputState,
    ComputerUseSessionTargetState, ComputerUseTargetStatus,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ComputerUseSessionHealthPolicy {
    pub require_recording_healthy: bool,
    pub require_recording_progress: bool,
    pub previous_recording_progress: Option<ComputerUseRecordingProgressFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseRecordingProgressFingerprint {
    pub lane: ComputerUseRecordingProgressLane,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trajectory_turn: Option<u64>,
    pub finalized_segments: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_partial_size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_partial_modified_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerUseRecordingProgressLane {
    Trajectory,
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerUseRecordingStatus {
    NotGranted,
    Unavailable,
    Stopped,
    Active,
    Paused,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerUseRecordingProgressStatus {
    NotRequested,
    BaselineRequired,
    Advanced,
    Stalled,
    Regressed,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseRecordingHealth {
    pub granted: bool,
    pub status: ComputerUseRecordingStatus,
    pub healthy: Option<bool>,
    pub issues: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<ComputerUseRecordingProgressFingerprint>,
    pub progress_status: ComputerUseRecordingProgressStatus,
}

impl ComputerUseRecordingHealth {
    #[must_use]
    pub fn from_state(
        granted: bool,
        state: &Value,
        policy: &ComputerUseSessionHealthPolicy,
    ) -> Self {
        if !granted {
            return Self {
                granted: false,
                status: ComputerUseRecordingStatus::NotGranted,
                healthy: None,
                issues: Vec::new(),
                progress: None,
                progress_status: if policy.require_recording_progress {
                    ComputerUseRecordingProgressStatus::Unavailable
                } else {
                    ComputerUseRecordingProgressStatus::NotRequested
                },
            };
        }

        let status = match state.get("status").and_then(Value::as_str) {
            Some("stopped") => ComputerUseRecordingStatus::Stopped,
            Some("active") => ComputerUseRecordingStatus::Active,
            Some("paused") => ComputerUseRecordingStatus::Paused,
            Some("degraded") => ComputerUseRecordingStatus::Degraded,
            _ => ComputerUseRecordingStatus::Unavailable,
        };
        let progress = recording_progress_fingerprint(state);
        let progress_status = recording_progress_status(policy, progress.as_ref());
        Self {
            granted: true,
            status,
            healthy: state.get("healthy").and_then(Value::as_bool),
            issues: state
                .get("issues")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            progress,
            progress_status,
        }
    }
}

fn recording_progress_fingerprint(
    state: &Value,
) -> Option<ComputerUseRecordingProgressFingerprint> {
    let trajectory = state
        .get("trajectory")
        .map(|value| value.get("structuredContent").unwrap_or(value));
    let trajectory_turn = trajectory
        .and_then(|value| value.get("next_turn"))
        .and_then(Value::as_u64);
    let video = state.get("video").filter(|value| !value.is_null());
    let segments = video
        .and_then(|value| value.get("segments"))
        .and_then(Value::as_array);
    let finalized_segments = segments.map_or(0, |segments| {
        segments
            .iter()
            .filter(|segment| segment.get("finalized").and_then(Value::as_bool) == Some(true))
            .count()
            .try_into()
            .unwrap_or(u64::MAX)
    });
    let current_partial = video
        .and_then(|value| value.get("current_partial"))
        .and_then(Value::as_str);
    let metadata = current_partial.and_then(|path| std::fs::metadata(path).ok());
    let current_partial_size_bytes = metadata.as_ref().map(std::fs::Metadata::len);
    let current_partial_modified_at_unix_ms = metadata
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX));

    let lane = if video.is_some() {
        ComputerUseRecordingProgressLane::Video
    } else {
        ComputerUseRecordingProgressLane::Trajectory
    };
    (video.is_some() || trajectory_turn.is_some()).then_some(
        ComputerUseRecordingProgressFingerprint {
            lane,
            trajectory_turn,
            finalized_segments,
            current_partial_size_bytes,
            current_partial_modified_at_unix_ms,
        },
    )
}

fn recording_progress_status(
    policy: &ComputerUseSessionHealthPolicy,
    current: Option<&ComputerUseRecordingProgressFingerprint>,
) -> ComputerUseRecordingProgressStatus {
    if !policy.require_recording_progress {
        return ComputerUseRecordingProgressStatus::NotRequested;
    }
    let Some(current) = current else {
        return ComputerUseRecordingProgressStatus::Unavailable;
    };
    let Some(previous) = policy.previous_recording_progress.as_ref() else {
        return ComputerUseRecordingProgressStatus::BaselineRequired;
    };
    compare_recording_progress(previous, current)
}

fn compare_recording_progress(
    previous: &ComputerUseRecordingProgressFingerprint,
    current: &ComputerUseRecordingProgressFingerprint,
) -> ComputerUseRecordingProgressStatus {
    if current.lane != previous.lane {
        return ComputerUseRecordingProgressStatus::BaselineRequired;
    }
    match current.lane {
        ComputerUseRecordingProgressLane::Trajectory => {
            if option_regressed(previous.trajectory_turn, current.trajectory_turn) {
                ComputerUseRecordingProgressStatus::Regressed
            } else if option_advanced(previous.trajectory_turn, current.trajectory_turn) {
                ComputerUseRecordingProgressStatus::Advanced
            } else {
                ComputerUseRecordingProgressStatus::Stalled
            }
        }
        ComputerUseRecordingProgressLane::Video => {
            if current.finalized_segments < previous.finalized_segments
                || (current.finalized_segments == previous.finalized_segments
                    && (option_regressed(
                        previous.current_partial_size_bytes,
                        current.current_partial_size_bytes,
                    ) || option_regressed(
                        previous.current_partial_modified_at_unix_ms,
                        current.current_partial_modified_at_unix_ms,
                    )))
            {
                ComputerUseRecordingProgressStatus::Regressed
            } else if current.finalized_segments > previous.finalized_segments
                || option_advanced(
                    previous.current_partial_size_bytes,
                    current.current_partial_size_bytes,
                )
                || option_advanced(
                    previous.current_partial_modified_at_unix_ms,
                    current.current_partial_modified_at_unix_ms,
                )
            {
                ComputerUseRecordingProgressStatus::Advanced
            } else {
                ComputerUseRecordingProgressStatus::Stalled
            }
        }
    }
}

fn option_advanced(previous: Option<u64>, current: Option<u64>) -> bool {
    matches!((previous, current), (Some(previous), Some(current)) if current > previous)
        || matches!((previous, current), (None, Some(_)))
}

fn option_regressed(previous: Option<u64>, current: Option<u64>) -> bool {
    matches!((previous, current), (Some(previous), Some(current)) if current < previous)
        || matches!((previous, current), (Some(_), None))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerUseSessionHealthBlocker {
    StateChangedDuringProbe,
    InputSuspended,
    TargetUnavailable,
    TargetMinimized,
    TargetNotForeground,
    TargetProbeFailed,
    SessionInterrupted,
    RecordingNotGranted,
    RecordingInactive,
    RecordingPaused,
    RecordingDegraded,
    RecordingProbeFailed,
    RecordingProgressBaselineRequired,
    RecordingProgressStalled,
    RecordingProgressRegressed,
    RecordingProgressUnavailable,
}

pub struct ComputerUseSessionHealthEvaluation {
    pub policy: ComputerUseSessionHealthPolicy,
    pub input_state: ComputerUseSessionInputState,
    pub target_state: ComputerUseSessionTargetState,
    pub recording: ComputerUseRecordingHealth,
    pub action_evidence_epoch: ActionEvidenceEpoch,
    pub transition_sequence: u64,
    pub state_changed_during_probe: bool,
    pub interrupted: bool,
    pub target_probe_failed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseSessionHealth {
    pub schema_version: u8,
    pub safe_to_input: bool,
    pub authority: String,
    pub automatic_activation: bool,
    pub automatic_input: bool,
    pub fresh_observation_required: bool,
    pub policy: ComputerUseSessionHealthPolicy,
    pub input_state: ComputerUseSessionInputState,
    pub target_state: ComputerUseSessionTargetState,
    pub recording: ComputerUseRecordingHealth,
    pub action_evidence_epoch: String,
    pub transition_sequence: u64,
    pub blockers: Vec<ComputerUseSessionHealthBlocker>,
}

impl ComputerUseSessionHealth {
    #[must_use]
    pub fn evaluate(evaluation: ComputerUseSessionHealthEvaluation) -> Self {
        let mut blockers = Vec::new();
        if evaluation.state_changed_during_probe {
            blockers.push(ComputerUseSessionHealthBlocker::StateChangedDuringProbe);
        }
        if evaluation.input_state.status == ComputerUseInputStatus::Suspended {
            blockers.push(ComputerUseSessionHealthBlocker::InputSuspended);
        }
        if evaluation.target_state.status == ComputerUseTargetStatus::Minimized
            || evaluation.target_state.minimized
        {
            blockers.push(ComputerUseSessionHealthBlocker::TargetMinimized);
        } else if evaluation.target_state.status == ComputerUseTargetStatus::Unavailable
            || !evaluation.target_state.visible
        {
            blockers.push(ComputerUseSessionHealthBlocker::TargetUnavailable);
        } else if !evaluation.target_state.foreground {
            blockers.push(ComputerUseSessionHealthBlocker::TargetNotForeground);
        }
        if evaluation.target_probe_failed {
            blockers.push(ComputerUseSessionHealthBlocker::TargetProbeFailed);
        }
        if evaluation.interrupted {
            blockers.push(ComputerUseSessionHealthBlocker::SessionInterrupted);
        }
        if evaluation.policy.require_recording_healthy
            || evaluation.policy.require_recording_progress
        {
            let recording_blocker = if !evaluation.recording.granted
                || evaluation.recording.status == ComputerUseRecordingStatus::NotGranted
            {
                Some(ComputerUseSessionHealthBlocker::RecordingNotGranted)
            } else {
                match evaluation.recording.status {
                    ComputerUseRecordingStatus::Unavailable => {
                        Some(ComputerUseSessionHealthBlocker::RecordingProbeFailed)
                    }
                    ComputerUseRecordingStatus::Stopped => {
                        Some(ComputerUseSessionHealthBlocker::RecordingInactive)
                    }
                    ComputerUseRecordingStatus::Paused => {
                        Some(ComputerUseSessionHealthBlocker::RecordingPaused)
                    }
                    ComputerUseRecordingStatus::Degraded => {
                        Some(ComputerUseSessionHealthBlocker::RecordingDegraded)
                    }
                    ComputerUseRecordingStatus::Active
                        if evaluation.recording.healthy != Some(true) =>
                    {
                        Some(ComputerUseSessionHealthBlocker::RecordingDegraded)
                    }
                    ComputerUseRecordingStatus::Active
                        if evaluation.policy.require_recording_progress =>
                    {
                        match evaluation.recording.progress_status {
                            ComputerUseRecordingProgressStatus::BaselineRequired => Some(
                                ComputerUseSessionHealthBlocker::RecordingProgressBaselineRequired,
                            ),
                            ComputerUseRecordingProgressStatus::Stalled => {
                                Some(ComputerUseSessionHealthBlocker::RecordingProgressStalled)
                            }
                            ComputerUseRecordingProgressStatus::Regressed => {
                                Some(ComputerUseSessionHealthBlocker::RecordingProgressRegressed)
                            }
                            ComputerUseRecordingProgressStatus::Unavailable
                            | ComputerUseRecordingProgressStatus::NotRequested => {
                                Some(ComputerUseSessionHealthBlocker::RecordingProgressUnavailable)
                            }
                            ComputerUseRecordingProgressStatus::Advanced => None,
                        }
                    }
                    ComputerUseRecordingStatus::Active => None,
                    ComputerUseRecordingStatus::NotGranted => unreachable!(),
                }
            };
            blockers.extend(recording_blocker);
        }
        let safe_to_input = blockers.is_empty();
        Self {
            schema_version: 1,
            safe_to_input,
            authority: "preflight_only".into(),
            automatic_activation: false,
            automatic_input: false,
            fresh_observation_required: true,
            policy: evaluation.policy,
            input_state: evaluation.input_state,
            target_state: evaluation.target_state,
            recording: evaluation.recording,
            action_evidence_epoch: evaluation.action_evidence_epoch.opaque_token(),
            transition_sequence: evaluation.transition_sequence,
            blockers,
        }
    }
}

#[cfg(test)]
mod tests;
