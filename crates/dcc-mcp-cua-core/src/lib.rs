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
pub(crate) use contracts::{
    DEFAULT_SNAPSHOT_MAX_DEPTH, DEFAULT_SNAPSHOT_MAX_ELEMENTS, MAX_SNAPSHOT_DEPTH,
    MAX_SNAPSHOT_ELEMENTS, MAX_TEXT_UTF16_UNITS,
};
#[cfg(test)]
pub(crate) use policy::*;
#[cfg(test)]
pub(crate) use runtime::{
    WindowTarget, bounded_snapshot_depth, bounded_snapshot_elements, tool_schema_from_inventory,
    validate_launch_request,
};

#[cfg(test)]
mod tests;
