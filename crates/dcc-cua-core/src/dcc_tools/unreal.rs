//! `unreal_remote_call`: call the local Unreal Remote Control HTTP API
//! (default port 30010). Loopback-only by policy; the HTTP exchange is a
//! minimal HTTP/1.1 client so no new TLS/HTTP dependency enters the core.

use std::time::Duration;

use async_trait::async_trait;
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::{Tool, ToolDef};
use serde_json::{Value, json};
use tokio::net::TcpStream;

use super::{
    DCC_LOOPBACK_HOST, MAX_DCC_REPLY_BYTES, effective_timeout_ms, require_object, require_port,
};

mod http;

pub(crate) const MAX_UNREAL_BODY_BYTES: usize = 256 * 1024;

const ALLOWED_METHODS: [&str; 3] = ["GET", "PUT", "POST"];

pub(crate) struct UnrealRemoteCallTool {
    def: ToolDef,
}

impl UnrealRemoteCallTool {
    pub(crate) fn new() -> Self {
        Self {
            def: ToolDef {
                name: "unreal_remote_call".into(),
                description: "Call the local Unreal Remote Control HTTP API (loopback only). \
                              The path must start with /remote/ and the body is forwarded as \
                              JSON."
                    .into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "port": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 65535,
                            "description": "Local Remote Control HTTP port (default 30010)."
                        },
                        "method": {
                            "type": "string",
                            "enum": ALLOWED_METHODS,
                            "description": "HTTP method for the Remote Control endpoint."
                        },
                        "path": {
                            "type": "string",
                            "description": "Endpoint path; must start with /remote/."
                        },
                        "body": {
                            "type": "object",
                            "description": "Optional JSON request body."
                        },
                        "timeout_ms": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": super::MAX_DCC_CALL_TIMEOUT_MS,
                            "description": "Optional per-call timeout in milliseconds."
                        }
                    },
                    "required": ["port", "method", "path"],
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
impl Tool for UnrealRemoteCallTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        match invoke_unreal_remote_call(&args).await {
            Ok(result) => result,
            Err(error) => *error,
        }
    }
}

async fn invoke_unreal_remote_call(args: &Value) -> Result<ToolResult, Box<ToolResult>> {
    require_object(args)?;
    let port = require_port(args)?;
    let timeout_ms = effective_timeout_ms(args)?;
    let method = args
        .get("method")
        .and_then(Value::as_str)
        .filter(|method| ALLOWED_METHODS.contains(method))
        .ok_or_else(|| Box::new(ToolResult::error("method must be one of GET, PUT, POST")))?;
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| is_safe_remote_path(path))
        .ok_or_else(|| {
            Box::new(ToolResult::error(
                "path must start with /remote/ and contain only URL-safe characters",
            ))
        })?;
    let body = match args.get("body") {
        None | Some(Value::Null) => Vec::new(),
        Some(body @ Value::Object(_)) => serde_json::to_vec(body).map_err(|error| {
            Box::new(ToolResult::error(format!(
                "body serialization failed: {error}"
            )))
        })?,
        Some(_) => return Err(Box::new(ToolResult::error("body must be a JSON object"))),
    };
    if body.len() > MAX_UNREAL_BODY_BYTES {
        return Err(Box::new(ToolResult::error(format!(
            "body exceeds {MAX_UNREAL_BODY_BYTES} bytes"
        ))));
    }
    let response = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        exchange(port, method, path, &body),
    )
    .await
    .map_err(|_| {
        Box::new(ToolResult::error(format!(
            "unreal_remote_call timed out after {timeout_ms} ms (dcc_typed_timeout)"
        )))
    })?
    .map_err(|error| {
        Box::new(ToolResult::error(format!(
            "unreal remote control on {DCC_LOOPBACK_HOST}:{port} failed: {error} \
             (dcc_typed_unreachable)"
        )))
    })?;
    let parsed_body: Value = serde_json::from_slice(&response.body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&response.body).into_owned()));
    let structured = json!({
        "port": port,
        "status": response.status,
        "body": parsed_body,
    });
    let result = ToolResult::text(structured.to_string()).with_structured(structured);
    if response.status >= 400 {
        return Err(Box::new(ToolResult {
            is_error: Some(true),
            ..result
        }));
    }
    Ok(result)
}

/// Restrict paths to the Remote Control namespace with URL-safe characters.
fn is_safe_remote_path(path: &str) -> bool {
    path.starts_with("/remote/")
        && path.len() <= 512
        && path
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "/-_.~%?&=".contains(character))
}

async fn exchange(
    port: u16,
    method: &str,
    path: &str,
    body: &[u8],
) -> std::io::Result<http::HttpResponse> {
    let stream = TcpStream::connect((DCC_LOOPBACK_HOST, port)).await?;
    http::roundtrip(stream, method, path, body, MAX_DCC_REPLY_BYTES).await
}
