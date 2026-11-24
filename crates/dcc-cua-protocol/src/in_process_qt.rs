//! Typed boundary for adapter-owned in-process Qt capture.
//!
//! The protocol describes the identity and lifecycle facts an adapter must
//! prove. It does not create Qt objects, discover widgets, or grant capture
//! authority by itself.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::frame_quality::{CaptureMode, FrameAssessment};

pub const MAX_CONNECTION_ID_CHARS: usize = 128;
pub const MAX_TASK_ID_CHARS: usize = 128;
pub const MAX_WIDGET_PATH_CHARS: usize = 1_024;
pub const MAX_WIDGET_CLASS_CHARS: usize = 256;
pub const MAX_OBJECT_NAME_CHARS: usize = 256;
pub const MAX_QT_QUEUE_DEPTH: u32 = 32;
pub const MAX_QT_COMMAND_LIFETIME_MS: u64 = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InProcessQtReadiness {
    Unavailable,
    WaitingForDocument,
    Ready,
    Disconnected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InProcessQtCommandStatus {
    Queued,
    Captured,
    BlankCapture,
    Refused,
    Expired,
    Disconnected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InProcessQtFailure {
    UnsupportedCaptureMode,
    InvalidBinding,
    WidgetUnavailable,
    AmbiguousWidget,
    NotReady,
    QueueFull,
    Expired,
    Disconnected,
    BlankCapture,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InProcessQtQueuePolicy {
    pub max_depth: u32,
    pub max_command_lifetime_ms: u64,
}

impl InProcessQtQueuePolicy {
    pub fn validate(&self) -> Result<(), InProcessQtContractError> {
        if self.max_depth == 0 || self.max_depth > MAX_QT_QUEUE_DEPTH {
            return Err(InProcessQtContractError::InvalidQueuePolicy);
        }
        if self.max_command_lifetime_ms == 0
            || self.max_command_lifetime_ms > MAX_QT_COMMAND_LIFETIME_MS
        {
            return Err(InProcessQtContractError::InvalidQueuePolicy);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InProcessQtTargetBinding {
    pub connection_id: String,
    pub task_id: String,
    pub process_id: u32,
    pub window_handle: u64,
    pub document_generation: u64,
}

impl InProcessQtTargetBinding {
    pub fn validate(&self) -> Result<(), InProcessQtContractError> {
        validate_identifier(&self.connection_id, MAX_CONNECTION_ID_CHARS)?;
        validate_identifier(&self.task_id, MAX_TASK_ID_CHARS)?;
        if self.process_id == 0 || self.window_handle == 0 || self.document_generation == 0 {
            return Err(InProcessQtContractError::InvalidBinding);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QtWidgetIdentity {
    /// Adapter-issued path that identifies one widget within one document.
    pub widget_path: String,
    pub class_name: String,
    pub object_name: Option<String>,
    pub widget_generation: u64,
}

impl QtWidgetIdentity {
    pub fn validate(&self) -> Result<(), InProcessQtContractError> {
        validate_identifier(&self.widget_path, MAX_WIDGET_PATH_CHARS)?;
        validate_identifier(&self.class_name, MAX_WIDGET_CLASS_CHARS)?;
        if let Some(object_name) = &self.object_name {
            validate_identifier(object_name, MAX_OBJECT_NAME_CHARS)?;
        }
        if self.widget_generation == 0 {
            return Err(InProcessQtContractError::InvalidWidget);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InProcessQtCapability {
    pub mode: CaptureMode,
    pub readiness: InProcessQtReadiness,
    pub target: InProcessQtTargetBinding,
    pub queue: InProcessQtQueuePolicy,
}

impl InProcessQtCapability {
    pub fn validate(&self) -> Result<(), InProcessQtContractError> {
        if self.mode != CaptureMode::InProcessQt {
            return Err(InProcessQtContractError::UnsupportedMode);
        }
        self.target.validate()?;
        self.queue.validate()?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InProcessQtCaptureRequest {
    pub target: InProcessQtTargetBinding,
    pub widget: QtWidgetIdentity,
    pub deadline_ms: u64,
}

impl InProcessQtCaptureRequest {
    pub fn validate(
        &self,
        capability: &InProcessQtCapability,
    ) -> Result<(), InProcessQtContractError> {
        capability.validate()?;
        self.target.validate()?;
        self.widget.validate()?;
        if self.target != capability.target {
            return Err(InProcessQtContractError::BindingChanged);
        }
        if self.deadline_ms == 0 || self.deadline_ms > capability.queue.max_command_lifetime_ms {
            return Err(InProcessQtContractError::InvalidDeadline);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InProcessQtCaptureReceipt {
    pub status: InProcessQtCommandStatus,
    pub target: InProcessQtTargetBinding,
    pub widget: QtWidgetIdentity,
    pub assessment: Option<FrameAssessment>,
    pub failure: Option<InProcessQtFailure>,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum InProcessQtContractError {
    #[error("in-process Qt capture mode is unsupported")]
    UnsupportedMode,
    #[error("in-process Qt binding is invalid")]
    InvalidBinding,
    #[error("in-process Qt widget identity is invalid")]
    InvalidWidget,
    #[error("in-process Qt queue policy is invalid")]
    InvalidQueuePolicy,
    #[error("in-process Qt target binding changed")]
    BindingChanged,
    #[error("in-process Qt capture deadline is invalid")]
    InvalidDeadline,
    #[error("in-process Qt identifier is empty or too long")]
    InvalidIdentifier,
}

fn validate_identifier(value: &str, max_chars: usize) -> Result<(), InProcessQtContractError> {
    if value.is_empty() || value.chars().count() > max_chars || value.contains('\0') {
        return Err(InProcessQtContractError::InvalidIdentifier);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
