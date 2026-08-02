//! CUA-backed, scoped Computer Use domain and execution boundary.
//!
//! CUA owns native capture/input and its color-coded cursor. This crate keeps
//! the DCC-MCP safety shell: exact target scope, fresh observations, bounded
//! actions, stop semantics, and auditable provenance.

mod contracts;
mod policy;
mod runtime;

pub use contracts::*;
pub use runtime::*;

#[cfg(test)]
mod tests;
