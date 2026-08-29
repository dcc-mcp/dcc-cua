use dcc_cua_core::ComputerUseError;
use thiserror::Error;

/// Local transport selected by the CLI or embedding host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostTransport {
    Stdio,
    Endpoint(String),
}

impl HostTransport {
    #[must_use]
    pub fn default_endpoint() -> String {
        dcc_cua_protocol::default_endpoint()
    }
}

#[derive(Debug, Error)]
pub enum HostError {
    #[error("host transport failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "host endpoint security failed: {endpoint} was pre-created before the first Host instance"
    )]
    EndpointHijacked {
        endpoint: String,
        #[source]
        source: std::io::Error,
    },
    #[error("host protocol failed: {0}")]
    Protocol(String),
    #[error("host protocol failed: {message}")]
    CodedProtocol {
        code: HostProtocolErrorCode,
        message: String,
    },
    #[error("{0}")]
    ComputerUse(#[from] ComputerUseError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostProtocolErrorCode {
    ProtocolMismatch,
    RawInputNotGranted,
    AppLaunchNotGranted,
    AppTerminateNotGranted,
    BrowserInputNotGranted,
    BrowserPrepareNotGranted,
    BrowserDownloadNotGranted,
    ClipboardReadNotGranted,
    ClipboardWriteNotGranted,
    SecretVaultUnavailable,
    SecretNotFound,
    SecretCaptureFailed,
    TaskAuthorizationRequired,
    TaskAuthorizationDenied,
    TaskAuthorizationExpired,
    TaskAuthorizationRevoked,
    RecordingNotGranted,
    NativeToolNotGranted,
    MenuInvokeNotGranted,
    SessionEscalationNotGranted,
    SessionLimitReached,
    RequestInProgress,
    RequestNotFound,
    Forbidden,
    Unsupported,
}

impl HostProtocolErrorCode {
    pub(crate) const fn as_wire_code(self) -> &'static str {
        match self {
            Self::ProtocolMismatch => "protocol_mismatch",
            Self::RawInputNotGranted => "raw_input_not_granted",
            Self::AppLaunchNotGranted => "app_launch_not_granted",
            Self::AppTerminateNotGranted => "app_terminate_not_granted",
            Self::BrowserInputNotGranted => "browser_input_not_granted",
            Self::BrowserPrepareNotGranted => "browser_prepare_not_granted",
            Self::BrowserDownloadNotGranted => "browser_download_not_granted",
            Self::ClipboardReadNotGranted => "clipboard_read_not_granted",
            Self::ClipboardWriteNotGranted => "clipboard_write_not_granted",
            Self::SecretVaultUnavailable => "secret_vault_unavailable",
            Self::SecretNotFound => "secret_not_found",
            Self::SecretCaptureFailed => "secret_capture_failed",
            Self::TaskAuthorizationRequired => "task_authorization_required",
            Self::TaskAuthorizationDenied => "task_authorization_denied",
            Self::TaskAuthorizationExpired => "task_authorization_expired",
            Self::TaskAuthorizationRevoked => "task_authorization_revoked",
            Self::RecordingNotGranted => "recording_not_granted",
            Self::NativeToolNotGranted => "native_tool_not_granted",
            Self::MenuInvokeNotGranted => "menu_invoke_not_granted",
            Self::SessionEscalationNotGranted => "session_escalation_not_granted",
            Self::SessionLimitReached => "session_limit_reached",
            Self::RequestInProgress => "request_in_progress",
            Self::RequestNotFound => "request_not_found",
            Self::Forbidden => "forbidden",
            Self::Unsupported => "unsupported",
        }
    }
}

impl HostError {
    pub(crate) fn coded_protocol(code: HostProtocolErrorCode, message: impl Into<String>) -> Self {
        Self::CodedProtocol {
            code,
            message: message.into(),
        }
    }
}
