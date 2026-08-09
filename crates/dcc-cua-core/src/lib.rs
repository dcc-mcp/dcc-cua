//! CUA-backed, scoped Computer Use domain and execution boundary.
//!
//! CUA owns native capture/input and its color-coded cursor. This crate keeps
//! the dcc-cua safety shell: exact target scope, fresh observations, bounded
//! actions, stop semantics, and auditable provenance.

mod contracts;
mod driver_factory;
mod interactive_desktop;
mod live_observation;
mod observation;
mod platform_process;
mod policy;
mod private_worker;
mod runtime;
mod showcase;
mod window_target;
mod windows_uia_fallback;

pub use contracts::*;
pub use private_worker::run_private_worker;
#[cfg(target_os = "macos")]
pub use private_worker::run_private_worker_with_appkit;
pub use runtime::*;

#[cfg(test)]
mod tests;
