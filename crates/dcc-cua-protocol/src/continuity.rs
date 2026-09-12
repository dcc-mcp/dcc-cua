//! Chained observations and receipts for long-running visual tasks.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetBinding {
    pub process_id: u32,
    pub window_handle: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationFrame {
    pub frame_id: String,
    pub observation_id: String,
    pub action_evidence_epoch: u64,
    pub target: TargetBinding,
    pub parent_frame_id: Option<String>,
    pub image_ref: Option<String>,
    pub semantic_state_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionReceipt {
    pub receipt_id: String,
    pub action_id: String,
    pub target: TargetBinding,
    pub completed: bool,
    pub post_observation_id: String,
    pub post_action_evidence_epoch: u64,
    pub transition_fence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchPolicy {
    pub max_actions: u32,
    pub abort_on_failure: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationDelta {
    pub added_candidates: Vec<String>,
    pub removed_candidates: Vec<String>,
    pub changed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuityState {
    pub latest_frame_id: Option<String>,
    pub latest_observation_id: Option<String>,
    pub latest_action_evidence_epoch: Option<u64>,
    pub pending_candidates: Vec<String>,
    pub completed_targets: Vec<String>,
}

impl ContinuityState {
    pub fn apply_observation(&mut self, frame: &ObservationFrame, delta: ObservationDelta) {
        self.latest_frame_id = Some(frame.frame_id.clone());
        self.latest_observation_id = Some(frame.observation_id.clone());
        self.latest_action_evidence_epoch = Some(frame.action_evidence_epoch);
        self.pending_candidates
            .retain(|id| !delta.removed_candidates.contains(id));
        for id in delta.added_candidates {
            if !self.pending_candidates.contains(&id) && !self.completed_targets.contains(&id) {
                self.pending_candidates.push(id);
            }
        }
    }

    pub fn mark_completed(&mut self, target_id: String) {
        self.pending_candidates.retain(|id| id != &target_id);
        if !self.completed_targets.contains(&target_id) {
            self.completed_targets.push(target_id);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuityMetrics {
    pub actions: u64,
    pub observations: u64,
    pub model_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub stale_rejections: u64,
    pub recovery_attempts: u64,
}

impl ContinuityMetrics {
    pub fn total_tokens(self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
    pub fn record_model_call(&mut self, input_tokens: u64, output_tokens: u64) {
        self.model_calls = self.model_calls.saturating_add(1);
        self.input_tokens = self.input_tokens.saturating_add(input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(output_tokens);
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ChainError {
    #[error("batch exceeds the configured action limit")]
    BatchLimitExceeded,
    #[error("action receipt did not complete")]
    IncompleteReceipt,
    #[error("target binding changed during continuity chain")]
    TargetChanged,
    #[error("next observation must consume the preceding receipt")]
    ObservationNotChained,
    #[error("action evidence epoch must advance")]
    EpochNotAdvanced,
}

/// Validate the observation that follows an action before allowing the next action.
pub fn validate_next_observation(
    receipt: &ActionReceipt,
    next: &ObservationFrame,
) -> Result<(), ChainError> {
    if !receipt.completed {
        return Err(ChainError::IncompleteReceipt);
    }
    if receipt.target != next.target {
        return Err(ChainError::TargetChanged);
    }
    if next.parent_frame_id.as_deref() != Some(receipt.post_observation_id.as_str()) {
        return Err(ChainError::ObservationNotChained);
    }
    let Some(expected_epoch) = receipt.post_action_evidence_epoch.checked_add(1) else {
        return Err(ChainError::EpochNotAdvanced);
    };
    if next.action_evidence_epoch != expected_epoch {
        return Err(ChainError::EpochNotAdvanced);
    }
    Ok(())
}

/// Validate a bounded batch while preserving the receipt-to-observation chain.
pub fn validate_batch(
    policy: BatchPolicy,
    initial: &ObservationFrame,
    steps: &[(ActionReceipt, ObservationFrame)],
) -> Result<(), ChainError> {
    if steps.len() > policy.max_actions as usize {
        return Err(ChainError::BatchLimitExceeded);
    }
    let mut previous = initial.clone();
    for (receipt, next) in steps {
        if receipt.post_observation_id != previous.observation_id {
            return Err(ChainError::ObservationNotChained);
        }
        validate_next_observation(receipt, next)?;
        previous = next.clone();
    }
    Ok(())
}
