//! Constructor-owned permission for one normal application login window.
use std::sync::Mutex;

use crate::{ComputerUseError, ComputerUseErrorCode, ComputerUseResult, RecordingAuthorization};

/// This capability is never accepted from tool arguments or serialized. It
/// relaxes only normal application identity classification; OS protection,
/// action scopes, fresh observations and credential handling remain separate.
pub struct NativeTaskAuthorization {
    process_id: u32,
    window_handle: u64,
    runtime_session: Mutex<Option<String>>,
    lifetime: RecordingAuthorization,
}

impl NativeTaskAuthorization {
    #[must_use]
    pub fn new(process_id: u32, window_handle: u64, expires_at_unix_ms: u64) -> Self {
        Self {
            process_id,
            window_handle,
            runtime_session: Mutex::new(None),
            lifetime: RecordingAuthorization::new(expires_at_unix_ms),
        }
    }

    pub fn revoke(&self) {
        self.lifetime.revoke();
    }

    pub(crate) fn bind_session(
        &self,
        session_id: &str,
        process_id: u32,
        window_handle: u64,
    ) -> ComputerUseResult<()> {
        self.validate_target(process_id, window_handle)?;
        self.lifetime.with_active(|| ()).map_err(|_| refused())?;
        let mut bound = self.runtime_session.lock().map_err(|_| refused())?;
        match bound.as_ref() {
            Some(expected) if expected != session_id => Err(refused()),
            Some(_) => Ok(()),
            None if !session_id.is_empty() => {
                *bound = Some(session_id.to_owned());
                Ok(())
            }
            None => Err(refused()),
        }
    }

    pub(crate) fn validate(
        &self,
        session_id: &str,
        process_id: u32,
        window_handle: u64,
    ) -> ComputerUseResult<()> {
        self.validate_target(process_id, window_handle)?;
        self.lifetime.with_active(|| ()).map_err(|_| refused())?;
        let bound = self.runtime_session.lock().map_err(|_| refused())?;
        if bound.as_deref() == Some(session_id) {
            Ok(())
        } else {
            Err(refused())
        }
    }

    fn validate_target(&self, process_id: u32, window_handle: u64) -> ComputerUseResult<()> {
        if process_id != 0
            && window_handle != 0
            && self.process_id == process_id
            && self.window_handle == window_handle
        {
            Ok(())
        } else {
            Err(refused())
        }
    }
}

fn refused() -> ComputerUseError {
    ComputerUseError::new(
        ComputerUseErrorCode::InvalidTarget,
        "normal application authorization is expired, revoked or outside this exact session target",
    )
}

#[cfg(test)]
mod tests;
