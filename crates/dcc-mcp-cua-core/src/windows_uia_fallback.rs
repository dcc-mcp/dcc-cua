#[cfg(windows)]
use std::sync::{Arc, Mutex};

#[cfg(windows)]
use dcc_mcp_cua_platform_windows::{UiaAction, UiaError, UiaSession, UiaTarget};
use serde_json::{Value, json};

#[cfg(windows)]
use crate::policy::{bounded_snapshot_depth, bounded_snapshot_elements};
use crate::runtime::ComputerUseSession;
use crate::window_target::WindowTarget;
#[cfg(windows)]
use crate::{ComputerUseAction, ComputerUseErrorCode, ComputerUseToolResult};
#[cfg(windows)]
use crate::{ComputerUseError, ComputerUseResult};

#[cfg(windows)]
#[derive(Clone)]
pub(crate) struct WindowsUiaFallback {
    target: UiaTarget,
    session: Arc<Mutex<UiaSession>>,
}

#[cfg(windows)]
impl WindowsUiaFallback {
    pub(crate) fn new(process_id: u32, window_handle: u64) -> Self {
        let target = UiaTarget {
            process_id,
            window_handle,
        };
        Self {
            target,
            session: Arc::new(Mutex::new(UiaSession::new(target))),
        }
    }

    pub(crate) fn matches(&self, process_id: u32, window_handle: u64) -> bool {
        self.target.process_id == process_id && self.target.window_handle == window_handle
    }

    pub(crate) async fn snapshot(
        &self,
        max_nodes: u32,
        max_depth: u32,
    ) -> ComputerUseResult<Value> {
        let session = Arc::clone(&self.session);
        tokio::task::spawn_blocking(move || {
            session
                .lock()
                .map_err(|_| backend_error("Windows UIA session lock was poisoned"))?
                .snapshot(max_nodes, max_depth, true)
                .map_err(map_error)
        })
        .await
        .map_err(|error| backend_error(format!("Windows UIA task failed: {error}")))?
    }

    pub(crate) async fn perform(
        &self,
        action: &ComputerUseAction,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        let session = Arc::clone(&self.session);
        let action = UiaAction {
            action: action.action.clone(),
            element_index: action.element_index,
            element_token: action.element_token.clone(),
            text: action.text.clone(),
            checked: None,
            delivery_mode: action.delivery_mode.clone(),
        };
        let value = tokio::task::spawn_blocking(move || {
            session
                .lock()
                .map_err(|_| backend_error("Windows UIA session lock was poisoned"))?
                .perform(&action)
                .map_err(map_error)
        })
        .await
        .map_err(|error| backend_error(format!("Windows UIA task failed: {error}")))??;
        Ok(ComputerUseToolResult {
            value,
            text: "Windows UIA semantic action completed".into(),
            images: Vec::new(),
            degraded: false,
        })
    }
}

impl ComputerUseSession {
    #[cfg(windows)]
    pub(crate) fn activate_windows_uia_fallback(&mut self, target: &WindowTarget) {
        if self
            .windows_uia
            .as_ref()
            .is_none_or(|fallback| !fallback.matches(target.pid, target.window_id))
        {
            self.windows_uia = Some(WindowsUiaFallback::new(target.pid, target.window_id));
        }
    }

    pub(crate) async fn visual_fallback_accessibility(
        &mut self,
        target: &WindowTarget,
        max_elements: u32,
        max_depth: u32,
        visual_fallback: &str,
    ) -> Value {
        #[cfg(not(windows))]
        let _ = (max_elements, max_depth);
        #[cfg(windows)]
        if let Ok(value) = self
            .windows_accessibility_snapshot(target, max_elements, max_depth)
            .await
        {
            return value;
        }
        json!({
            "degraded": true,
            "accessibility_available": false,
            "fallback": visual_fallback,
            "window_id": target.window_id,
            "pid": target.pid,
        })
    }

    #[cfg(windows)]
    pub(crate) async fn windows_accessibility_snapshot(
        &mut self,
        target: &WindowTarget,
        max_elements: u32,
        max_depth: u32,
    ) -> ComputerUseResult<Value> {
        self.activate_windows_uia_fallback(target);
        self.windows_uia
            .as_ref()
            .expect("Windows UIA fallback was initialized")
            .snapshot(
                bounded_snapshot_elements(max_elements),
                bounded_snapshot_depth(max_depth),
            )
            .await
    }
}

#[cfg(windows)]
fn map_error(error: UiaError) -> ComputerUseError {
    let code = match error {
        UiaError::InvalidTarget(_) => ComputerUseErrorCode::InvalidTarget,
        UiaError::StaleSnapshot(_) => ComputerUseErrorCode::StaleObservation,
        UiaError::PermissionDenied(_) | UiaError::InvalidAction(_) => {
            ComputerUseErrorCode::InvalidAction
        }
        UiaError::Unsupported | UiaError::BackendUnavailable(_) => {
            ComputerUseErrorCode::BackendUnavailable
        }
    };
    ComputerUseError::new(code, error.to_string())
}

#[cfg(windows)]
fn backend_error(message: impl Into<String>) -> ComputerUseError {
    ComputerUseError::new(ComputerUseErrorCode::BackendUnavailable, message)
}
