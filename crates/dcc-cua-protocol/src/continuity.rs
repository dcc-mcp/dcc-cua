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
    if next.action_evidence_epoch <= receipt.post_action_evidence_epoch {
        return Err(ChainError::EpochNotAdvanced);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> TargetBinding {
        TargetBinding {
            process_id: 42,
            window_handle: 7,
        }
    }

    fn receipt() -> ActionReceipt {
        ActionReceipt {
            receipt_id: "r1".into(),
            action_id: "harvest:plot-1".into(),
            target: target(),
            completed: true,
            post_observation_id: "obs-1".into(),
            post_action_evidence_epoch: 10,
            transition_fence: "fence-1".into(),
        }
    }

    fn next() -> ObservationFrame {
        ObservationFrame {
            frame_id: "frame-2".into(),
            observation_id: "obs-2".into(),
            action_evidence_epoch: 11,
            target: target(),
            parent_frame_id: Some("obs-1".into()),
            image_ref: Some("qq-classic-farm://frame-2".into()),
            semantic_state_id: Some("farm-grid-v2".into()),
        }
    }

    #[test]
    fn accepts_chained_farm_observation() {
        assert!(validate_next_observation(&receipt(), &next()).is_ok());
    }

    #[test]
    fn rejects_stale_or_unrelated_observation() {
        let mut frame = next();
        frame.parent_frame_id = Some("obs-0".into());
        assert_eq!(
            validate_next_observation(&receipt(), &frame),
            Err(ChainError::ObservationNotChained)
        );
        frame.parent_frame_id = Some("obs-1".into());
        frame.action_evidence_epoch = 10;
        assert_eq!(
            validate_next_observation(&receipt(), &frame),
            Err(ChainError::EpochNotAdvanced)
        );
    }

    #[test]
    fn rejects_window_switch() {
        let mut frame = next();
        frame.target.window_handle = 8;
        assert_eq!(
            validate_next_observation(&receipt(), &frame),
            Err(ChainError::TargetChanged)
        );
    }
}
