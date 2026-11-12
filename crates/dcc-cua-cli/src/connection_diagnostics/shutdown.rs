//! Transport closure and owned-resource cleanup are separate observations.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupStatus {
    #[default]
    Unknown,
    Pending,
    Completed,
    Failed,
}

/// No process-exit or child-reap claim is inferred from this receipt.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ShutdownDiagnostics {
    pub cleanup_status: CleanupStatus,
    pub failed_tasks: Option<usize>,
    pub cleanup_finished_at_unix_ms: Option<u64>,
}
