#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiaTarget {
    pub process_id: u32,
    pub window_handle: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiaAction {
    pub action: String,
    pub element_index: Option<u32>,
    pub element_token: Option<String>,
    pub text: Option<String>,
    pub checked: Option<bool>,
}

#[derive(Debug, thiserror::Error)]
pub enum UiaError {
    #[error("Windows UI Automation fallback is unavailable on this platform")]
    Unsupported,
    #[error("Windows UI Automation target is invalid: {0}")]
    InvalidTarget(String),
    #[error("Windows UI Automation snapshot is stale: {0}")]
    StaleSnapshot(String),
    #[error("Windows UI Automation denied the request: {0}")]
    PermissionDenied(String),
    #[error("Windows UI Automation action is invalid: {0}")]
    InvalidAction(String),
    #[error("Windows UI Automation backend failed: {0}")]
    BackendUnavailable(String),
}
