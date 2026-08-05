//! DCC typed tools registered into the upstream CUA `ToolRegistry`.
//!
//! ADR 0002 (hybrid strategy): DCC typed APIs (Maya commandPort, Unreal
//! Remote Control) are first-class CUA tools registered through
//! `DriverHostOptions.register_host_tools`. They reuse upstream dispatch,
//! authorization, and audit instead of a parallel host-side registry.
//!
//! The registration hook is a plain `fn` pointer, so tools stay stateless:
//! every invocation opens its own loopback connection. Agents cannot point
//! these tools at remote hosts — the endpoint is always `127.0.0.1` and only
//! the port is caller-selected.

use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::ToolRegistry;
use serde_json::Value;

pub(crate) mod maya;
pub(crate) mod unreal;

/// Loopback-only endpoint policy for every DCC typed tool.
pub(crate) const DCC_LOOPBACK_HOST: &str = "127.0.0.1";

pub(crate) const DEFAULT_DCC_CALL_TIMEOUT_MS: u64 = 15_000;
pub(crate) const MAX_DCC_CALL_TIMEOUT_MS: u64 = 60_000;
pub(crate) const MAX_DCC_REPLY_BYTES: usize = 1024 * 1024;

/// Registered on every embedded/authorized driver via
/// `DriverHostOptions.register_host_tools` (see `driver_factory`).
pub(crate) fn register_dcc_host_tools(registry: &mut ToolRegistry) {
    registry.register(Box::new(maya::MayaCommandTool::new()));
    registry.register(Box::new(unreal::UnrealRemoteCallTool::new()));
}

/// Extract and validate the mandatory loopback port argument.
pub(crate) fn require_port(args: &Value) -> Result<u16, Box<ToolResult>> {
    let port = args
        .get("port")
        .and_then(Value::as_u64)
        .ok_or_else(|| Box::new(ToolResult::error("port must be an integer in 1..65535")))?;
    u16::try_from(port)
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| Box::new(ToolResult::error("port must be an integer in 1..65535")))
}

/// Resolve the per-call timeout, clamped to the tool ceiling.
pub(crate) fn effective_timeout_ms(args: &Value) -> Result<u64, Box<ToolResult>> {
    match args.get("timeout_ms") {
        None | Some(Value::Null) => Ok(DEFAULT_DCC_CALL_TIMEOUT_MS),
        Some(value) => value
            .as_u64()
            .filter(|timeout| (1..=MAX_DCC_CALL_TIMEOUT_MS).contains(timeout))
            .ok_or_else(|| {
                Box::new(ToolResult::error(format!(
                    "timeout_ms must be an integer in 1..{MAX_DCC_CALL_TIMEOUT_MS}"
                )))
            }),
    }
}

/// Reject non-object argument envelopes before field extraction.
pub(crate) fn require_object(args: &Value) -> Result<(), Box<ToolResult>> {
    if args.is_object() {
        Ok(())
    } else {
        Err(Box::new(ToolResult::error(
            "arguments must be a JSON object",
        )))
    }
}
