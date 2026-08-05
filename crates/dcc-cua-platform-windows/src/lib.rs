//! Exact-window Windows UI Automation fallback.
//!
//! CUA remains the primary cross-platform backend. This crate owns the
//! Windows-only semantic fallback used when an application's UIA provider is
//! usable but CUA's combined window snapshot path is not.

mod contracts;
#[cfg(any(windows, test))]
mod snapshot;

#[cfg(windows)]
mod windows;

pub use contracts::{UiaAction, UiaError, UiaTarget};

#[cfg(windows)]
pub use windows::UiaSession;

#[cfg(not(windows))]
pub struct UiaSession;

#[cfg(not(windows))]
impl UiaSession {
    pub fn new(_target: UiaTarget) -> Self {
        Self
    }

    pub fn snapshot(
        &mut self,
        _max_nodes: u32,
        _max_depth: u32,
        _allow_owned_standard_menu_popup: bool,
    ) -> Result<serde_json::Value, UiaError> {
        Err(UiaError::Unsupported)
    }

    pub fn perform(&mut self, _action: &UiaAction) -> Result<serde_json::Value, UiaError> {
        Err(UiaError::Unsupported)
    }
}

#[cfg(test)]
mod tests;
