//! Process-local cooperative interruption shared by Host runtime components.
//!
//! The generation counter is deliberately independent from any UI indicator:
//! producers and consumers can participate without depending on presentation.

use std::sync::atomic::{AtomicU64, Ordering};

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

/// Return whether a session has observed a newer cooperative stop generation.
#[must_use]
pub const fn interrupt_generation_changed(started: u64, current: u64) -> bool {
    started != current
}

#[cfg(test)]
mod tests;
