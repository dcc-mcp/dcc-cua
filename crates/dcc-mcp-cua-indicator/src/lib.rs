//! User-visible control-session indicator owned by the Host.
//!
//! CUA owns native input and its semantic mouse cursor. This crate owns the
//! separate, non-activating safety banner that tells the operator which app is
//! under agent control and provides the Escape stop boundary.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use thiserror::Error;

const MAX_LABEL_CHARS: usize = 512;
static INTERRUPT_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Broadcast a cooperative stop to every control session in this Host process.
pub fn broadcast_interrupt() -> u64 {
    INTERRUPT_GENERATION
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1)
}

/// Return the current Host-process stop generation.
#[must_use]
pub fn interrupt_generation() -> u64 {
    INTERRUPT_GENERATION.load(Ordering::Acquire)
}

#[must_use]
pub fn interrupt_generation_changed(started: u64, current: u64) -> bool {
    started != current
}

#[derive(Debug, Clone)]
pub struct BannerTarget {
    pub process_id: u32,
    pub window_handle: u64,
    pub label: String,
}

impl BannerTarget {
    fn validate(&self) -> Result<(), IndicatorError> {
        if self.process_id == 0 || self.window_handle == 0 {
            return Err(IndicatorError::InvalidTarget(
                "control banner requires an exact process and window".into(),
            ));
        }
        let label_len = self.label.chars().count();
        if label_len == 0 || label_len > MAX_LABEL_CHARS {
            return Err(IndicatorError::InvalidTarget(format!(
                "control banner label must contain 1..{MAX_LABEL_CHARS} characters"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BannerStatus {
    pub backend: &'static str,
    pub visible: bool,
    pub target_frame_visible: bool,
    pub interrupted: bool,
    pub stop_key: &'static str,
}

#[derive(Debug, Error)]
pub enum IndicatorError {
    #[error("invalid banner target: {0}")]
    InvalidTarget(String),
    #[error("control banner backend failed: {0}")]
    Backend(String),
}

pub struct ControlBanner {
    generation: u64,
    platform: platform::PlatformBanner,
}

impl ControlBanner {
    pub fn start(target: BannerTarget) -> Result<Self, IndicatorError> {
        target.validate()?;
        Ok(Self {
            generation: interrupt_generation(),
            platform: platform::PlatformBanner::start(target)?,
        })
    }

    #[must_use]
    pub fn status(&self) -> BannerStatus {
        let mut status = self.platform.status();
        status.interrupted = self.interrupted();
        status
    }

    #[must_use]
    pub fn interrupted(&self) -> bool {
        interrupt_generation_changed(self.generation, interrupt_generation())
            || self.platform.interrupted()
    }

    pub fn set_cursor_position(&self, x: f64, y: f64) {
        self.platform.set_cursor_position(x, y);
    }
}

#[cfg(windows)]
mod platform;

#[cfg(not(windows))]
mod platform {
    use super::{BannerStatus, BannerTarget, IndicatorError};

    pub(super) struct PlatformBanner;

    impl PlatformBanner {
        pub(super) fn start(_target: BannerTarget) -> Result<Self, IndicatorError> {
            Ok(Self)
        }

        pub(super) fn status(&self) -> BannerStatus {
            BannerStatus {
                backend: "unavailable",
                visible: false,
                target_frame_visible: false,
                interrupted: false,
                stop_key: "Escape",
            }
        }

        pub(super) fn interrupted(&self) -> bool {
            false
        }

        pub(super) fn set_cursor_position(&self, _x: f64, _y: f64) {}
    }
}

#[cfg(test)]
mod tests;
