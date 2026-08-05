//! CUA-backed, scoped Computer Use domain and execution boundary.
//!
//! CUA owns native capture/input and its color-coded cursor. This crate keeps
//! the dcc-cua safety shell: exact target scope, fresh observations, bounded
//! actions, stop semantics, and auditable provenance.

mod contracts;
mod dcc_tools;
mod driver_factory;
mod interactive_desktop;
mod observation;
mod platform_process;
mod policy;
mod runtime;
mod window_target;
mod windows_uia_fallback;

pub use contracts::*;
pub use runtime::*;

#[cfg(test)]
mod tests;
