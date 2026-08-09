//! Hidden SDK worker used by the packaged macOS Host.
//!
//! The parent launches the same `dcc-cua` executable with `__private-worker`;
//! no standalone `cua-driver` executable or public endpoint is involved.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::Arc;

use cua_driver_sdk::worker::{
    ActionCompletion, ChannelRequest, ChannelResponse, PRIVATE_WORKER_PROTOCOL_VERSION,
    WorkerInitialization,
};
use cua_driver_sdk::{CuaDriver, CuaDriverSession, DriverHostOptions};
use serde_json::Value;

use crate::contracts::MOUSE_CURSOR_THEME;

/// Serve one parent-owned private-worker generation over inherited stdio.
pub async fn run_private_worker(generation: String) -> Result<(), String> {
    if generation.is_empty()
        || generation.len() > 128
        || !generation
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("private worker requires one valid --generation value".into());
    }

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    let Some(first_line) = lines.next() else {
        return Ok(());
    };
    let initialization_request: ChannelRequest =
        serde_json::from_str(&first_line.map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if initialization_request.protocol_version != PRIVATE_WORKER_PROTOCOL_VERSION
        || initialization_request.request_id != 0
        || initialization_request.generation != generation
        || initialization_request.operation != "initialize"
    {
        write_response(
            &mut writer,
            &ChannelResponse::error(
                initialization_request.request_id,
                &generation,
                "invalid_initialization",
                "private worker initialization identity mismatch",
                ActionCompletion::NotStarted,
            ),
        )?;
        return Ok(());
    }
    let initialization: WorkerInitialization = serde_json::from_value(
        initialization_request
            .arguments
            .ok_or("private worker initialization omitted options")?,
    )
    .map_err(|error| error.to_string())?;
    if initialization.host_bundle_id.trim().is_empty() {
        write_response(
            &mut writer,
            &ChannelResponse::error(
                0,
                &generation,
                "invalid_initialization",
                "private worker host identity is empty",
                ActionCompletion::NotStarted,
            ),
        )?;
        return Ok(());
    }

    crate::driver_factory::ensure_bundled_cursor_theme().map_err(|error| error.to_string())?;

    let driver = match CuaDriver::try_create_configured_for_host(
        initialization.configured_driver,
        DriverHostOptions {
            cursor: cursor_overlay::CursorConfig {
                theme_id: MOUSE_CURSOR_THEME.into(),
                ..cursor_overlay::CursorConfig::default()
            },
            host_owns_permission_ux: true,
            host_bundle_id: Some(initialization.host_bundle_id.clone()),
            claude_code_compatibility: false,
            prepare_desktop_environment: true,
            register_host_tools: None,
            authorization_host: None,
            activity_observer: None,
        },
    ) {
        Ok(driver) => driver,
        Err(error) => {
            write_response(
                &mut writer,
                &ChannelResponse::error(
                    0,
                    &generation,
                    "runtime_initialization_failed",
                    error.to_string(),
                    ActionCompletion::NotStarted,
                ),
            )?;
            return Ok(());
        }
    };
    let metadata = driver.metadata().await.map_err(|error| error.to_string())?;
    write_response(
        &mut writer,
        &ChannelResponse::ok(
            0,
            &generation,
            serde_json::json!({
                "ready": true,
                "pid": std::process::id(),
                "host_bundle_id": initialization.host_bundle_id,
                "metadata": metadata,
            }),
        ),
    )?;

    let mut sessions: HashMap<String, Arc<CuaDriverSession>> = HashMap::new();
    for line in lines {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        let request: ChannelRequest = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                write_response(
                    &mut writer,
                    &ChannelResponse::error(
                        0,
                        &generation,
                        "invalid_request",
                        format!("parse private worker request: {error}"),
                        ActionCompletion::NotStarted,
                    ),
                )?;
                continue;
            }
        };
        if request.protocol_version != PRIVATE_WORKER_PROTOCOL_VERSION
            || request.generation != generation
        {
            write_response(
                &mut writer,
                &ChannelResponse::error(
                    request.request_id,
                    &generation,
                    "generation_mismatch",
                    "private worker request belongs to another runtime generation",
                    ActionCompletion::NotStarted,
                ),
            )?;
            continue;
        }

        let response = handle_request(&driver, &mut sessions, &generation, request).await;
        let shutdown = response
            .result
            .as_ref()
            .and_then(|value| value.get("shutdown"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        write_response(&mut writer, &response)?;
        if shutdown {
            break;
        }
    }

    sessions.clear();
    driver.shutdown().await.map_err(|error| error.to_string())
}

async fn handle_request(
    driver: &Arc<CuaDriver>,
    sessions: &mut HashMap<String, Arc<CuaDriverSession>>,
    generation: &str,
    request: ChannelRequest,
) -> ChannelResponse {
    let request_id = request.request_id;
    let result: Result<Value, String> = match request.operation.as_str() {
        "metadata" => driver
            .metadata()
            .await
            .map_err(|error| error.to_string())
            .and_then(|metadata| serde_json::to_value(metadata).map_err(|error| error.to_string())),
        "list" => driver
            .list_tools_json()
            .await
            .map_err(|error| error.to_string())
            .and_then(|json| serde_json::from_str(&json).map_err(|error| error.to_string())),
        "call" => {
            let name = request.name.as_deref().unwrap_or("");
            let arguments = request
                .arguments
                .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
            let invocation = if let Some(handle) = request.session_handle.as_deref() {
                match sessions.get(handle) {
                    Some(session) => {
                        session
                            .call_tool(name.to_owned(), arguments.to_string())
                            .await
                    }
                    None => {
                        return ChannelResponse::error(
                            request_id,
                            generation,
                            "session_not_bound",
                            "private worker session handle is not live on this channel",
                            ActionCompletion::NotStarted,
                        );
                    }
                }
            } else {
                driver.call_tool_from_trusted_adapter(name, arguments).await
            };
            invocation
                .map_err(|error| error.to_string())
                .and_then(|result| {
                    serde_json::from_str(&result.raw_json).map_err(|error| error.to_string())
                })
        }
        "bind_session" => {
            let options = request
                .arguments
                .ok_or_else(|| "bind_session omitted options".to_owned())
                .and_then(|value| serde_json::from_value(value).map_err(|error| error.to_string()));
            match options {
                Ok(options) => match driver.create_trusted_session(options) {
                    Ok(session) => {
                        let handle = uuid::Uuid::new_v4().to_string();
                        sessions.insert(handle.clone(), session);
                        Ok(serde_json::json!({"session_handle": handle}))
                    }
                    Err(error) => Err(error.to_string()),
                },
                Err(error) => Err(error),
            }
        }
        "close_session" => {
            let Some(handle) = request.session_handle.as_deref() else {
                return ChannelResponse::error(
                    request_id,
                    generation,
                    "invalid_request",
                    "close_session omitted session_handle",
                    ActionCompletion::NotStarted,
                );
            };
            if let Some(session) = sessions.remove(handle) {
                session.close();
            }
            Ok(serde_json::json!({"closed": true}))
        }
        "shutdown" => {
            sessions.clear();
            driver
                .shutdown()
                .await
                .map(|()| serde_json::json!({"shutdown": true}))
                .map_err(|error| error.to_string())
        }
        other => Err(format!("unknown private worker operation: {other}")),
    };

    match result {
        Ok(value) => ChannelResponse::ok(request_id, generation, value),
        Err(error) => ChannelResponse::error(
            request_id,
            generation,
            "worker_request_failed",
            error,
            ActionCompletion::Completed,
        ),
    }
}

fn write_response(writer: &mut impl Write, response: &ChannelResponse) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, response).map_err(|error| error.to_string())?;
    writer.write_all(b"\n").map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())
}
