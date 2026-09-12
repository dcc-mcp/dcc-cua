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

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ChainError {
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
