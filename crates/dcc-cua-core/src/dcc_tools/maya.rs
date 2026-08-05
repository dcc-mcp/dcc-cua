//! `maya_command`: execute a MEL/Python snippet against a local Maya
//! commandPort (`commandPort -name ":<port>"`). Loopback-only by policy.

use std::time::Duration;

use async_trait::async_trait;
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::{Tool, ToolDef};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::{
    DCC_LOOPBACK_HOST, MAX_DCC_REPLY_BYTES, effective_timeout_ms, require_object, require_port,
};

pub(crate) const MAX_MAYA_COMMAND_BYTES: usize = 64 * 1024;

pub(crate) struct MayaCommandTool {
    def: ToolDef,
}

impl MayaCommandTool {
    pub(crate) fn new() -> Self {
        Self {
            def: ToolDef {
                name: "maya_command".into(),
                description: "Execute a MEL or Python snippet through a local Maya commandPort \
                              (loopback only). The snippet language must match the commandPort \
                              sourceType configured inside Maya."
                    .into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "port": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 65535,
                            "description": "Local Maya commandPort TCP port."
                        },
                        "command": {
                            "type": "string",
                            "description": "MEL or Python source sent verbatim to the commandPort."
                        },
                        "timeout_ms": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": super::MAX_DCC_CALL_TIMEOUT_MS,
                            "description": "Optional per-call timeout in milliseconds."
                        }
                    },
                    "required": ["port", "command"],
                    "additionalProperties": false
                }),
                read_only: false,
                destructive: true,
                idempotent: false,
                open_world: true,
            },
        }
    }
}

#[async_trait]
impl Tool for MayaCommandTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        match invoke_maya_command(&args).await {
            Ok(result) => result,
            Err(error) => *error,
        }
    }
}

async fn invoke_maya_command(args: &Value) -> Result<ToolResult, Box<ToolResult>> {
    require_object(args)?;
    let port = require_port(args)?;
    let timeout_ms = effective_timeout_ms(args)?;
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .filter(|command| !command.is_empty())
        .ok_or_else(|| Box::new(ToolResult::error("command must be a non-empty string")))?;
    if command.len() > MAX_MAYA_COMMAND_BYTES {
        return Err(Box::new(ToolResult::error(format!(
            "command exceeds {MAX_MAYA_COMMAND_BYTES} bytes"
        ))));
    }
    let reply = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        exchange(port, command.as_bytes()),
    )
    .await
    .map_err(|_| {
        Box::new(ToolResult::error(format!(
            "maya_command timed out after {timeout_ms} ms (dcc_typed_timeout)"
        )))
    })?
    .map_err(|error| {
        Box::new(ToolResult::error(format!(
            "maya commandPort on {DCC_LOOPBACK_HOST}:{port} failed: {error} (dcc_typed_unreachable)"
        )))
    })?;
    Ok(ToolResult::text(reply.clone()).with_structured(json!({
        "port": port,
        "reply": reply,
    })))
}

/// Send the command and read the NUL-terminated commandPort reply.
async fn exchange(port: u16, command: &[u8]) -> std::io::Result<String> {
    let mut stream = TcpStream::connect((DCC_LOOPBACK_HOST, port)).await?;
    stream.write_all(command).await?;
    stream.write_all(b"\n").await?;
    let mut reply = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        if let Some(nul) = chunk[..read].iter().position(|byte| *byte == 0) {
            reply.extend_from_slice(&chunk[..nul]);
            break;
        }
        reply.extend_from_slice(&chunk[..read]);
        if reply.len() > MAX_DCC_REPLY_BYTES {
            return Err(std::io::Error::other(format!(
                "reply exceeds {MAX_DCC_REPLY_BYTES} bytes"
            )));
        }
    }
    Ok(String::from_utf8_lossy(&reply).trim_end().to_owned())
}
