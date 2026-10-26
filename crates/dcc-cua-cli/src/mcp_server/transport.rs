//! Bounded JSON-RPC byte transport; task cleanup remains owned by the connection.
use std::error::Error;

use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::{CloseReason, TaskAuthorizationServer, rpc_error};

pub(super) async fn run_transport<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    server: &mut TaskAuthorizationServer,
    input: &mut R,
    output: &mut W,
    reason: &mut CloseReason,
) -> Result<(), Box<dyn Error>> {
    loop {
        *reason = CloseReason::InputError;
        let mut line = Vec::new();
        let count = (&mut *input)
            .take(dcc_cua_protocol::MAX_JSON_FRAME_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .await?;
        if count == 0 {
            *reason = CloseReason::StdinEof;
            break;
        }
        if let Some(diagnostics) = server.diagnostics.as_mut() {
            diagnostics.activity();
        }
        if count > dcc_cua_protocol::MAX_JSON_FRAME_BYTES {
            *reason = CloseReason::FrameLimit;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "MCP request exceeds frame limit",
            )
            .into());
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let decoded: Value = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(_) => {
                *reason = CloseReason::OutputError;
                output
                    .write_all(
                        format!("{}\n", rpc_error(Value::Null, -32700, "Parse error")).as_bytes(),
                    )
                    .await?;
                output.flush().await?;
                continue;
            }
        };
        if let Some(diagnostics) = server.diagnostics.as_mut() {
            diagnostics.set_request_in_flight(true);
        }
        let mut responses = Vec::new();
        if let Some(batch) = decoded.as_array() {
            for message in batch {
                if let Some(response) = server.handle_rpc(message.clone()).await {
                    responses.push(response);
                }
            }
        } else if let Some(response) = server.handle_rpc(decoded).await {
            responses.push(response);
        }
        server.update_diagnostics();
        if let Some(diagnostics) = server.diagnostics.as_mut() {
            diagnostics.set_request_in_flight(false);
        }
        if responses.is_empty() {
            continue;
        }
        let response = if responses.len() == 1 {
            responses.remove(0)
        } else {
            Value::Array(responses)
        };
        *reason = CloseReason::OutputError;
        output.write_all(format!("{response}\n").as_bytes()).await?;
        output.flush().await?;
    }
    Ok(())
}
