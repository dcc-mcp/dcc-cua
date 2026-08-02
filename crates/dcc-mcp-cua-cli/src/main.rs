use std::env;
use std::fs;
use std::io::Read;

use dcc_mcp_cua_client::{
    HostClient, HostClientError, HostProcess, HostResponse, SnapshotTransport,
};
use dcc_mcp_cua_core::{
    ComputerUseAction, ComputerUseClipboardWriteRequest, ComputerUseDriver,
    ComputerUseLaunchRequest, ComputerUseTargetScope,
};
use dcc_mcp_cua_host::{HostTransport, run as run_host};
use dcc_mcp_cua_shm::{SharedImageDescriptor, SharedImageReader};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    let flags = args.collect::<Vec<_>>();
    if command == "host-call" {
        host_call(&flags).await?;
        return Ok(());
    }
    if command == "host-batch" {
        host_batch(&flags).await?;
        return Ok(());
    }
    if command == "host-jsonl" {
        host_jsonl(&flags).await?;
        return Ok(());
    }
    let driver = ComputerUseDriver::create()?;

    match command.as_str() {
        "list" => list_windows(&driver, &flags).await?,
        "apps" => list_apps(&driver).await?,
        "tools" => list_tools(&driver).await?,
        "call" => call_tool(&driver, &flags).await?,
        "desktop-snapshot" => desktop_snapshot(&driver, &flags).await?,
        "screen-size" => screen_size(&driver).await?,
        "cursor-position" => cursor_position(&driver).await?,
        "launch" => launch_app(&driver, &flags).await?,
        "terminate" => terminate_app(&driver, &flags).await?,
        "clipboard-read" => clipboard_read(&driver, &flags).await?,
        "clipboard-write" => clipboard_write(&driver, &flags).await?,
        "host" => {
            let transport = if has_flag(&flags, "--stdio") {
                HostTransport::Stdio
            } else {
                HostTransport::Endpoint(
                    flag_value(&flags, "--endpoint")
                        .unwrap_or_else(HostTransport::default_endpoint),
                )
            };
            run_host(driver, transport).await?;
        }
        "snapshot" => snapshot(&driver, &flags).await?,
        "verify" => verify_state(&driver, &flags).await?,
        "act" => act(&driver, &flags).await?,
        "desktop-act" => desktop_act(&driver, &flags).await?,
        "doctor" => doctor(&driver).await?,
        "help" | "--help" | "-h" => print_help(),
        other => return Err(format!("unknown command: {other}; use `help`").into()),
    }
    Ok(())
}

async fn list_windows(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let pid = flag_value(flags, "--pid")
        .map(|value| value.parse::<u32>())
        .transpose()?;
    let mut windows = driver
        .list_windows_filtered(pid, has_flag(flags, "--on-screen"))
        .await?;
    if let Some(app) = flag_value(flags, "--app") {
        windows.retain(|window| {
            window["app_name"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(&app))
        });
    }
    println!("{}", serde_json::to_string_pretty(&windows)?);
    Ok(())
}

async fn list_apps(driver: &ComputerUseDriver) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&driver.list_apps().await?)?
    );
    Ok(())
}

async fn list_tools(driver: &ComputerUseDriver) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&driver.list_tools().await?)?
    );
    Ok(())
}

async fn call_tool(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let name = flag_value(flags, "--tool").ok_or("call requires --tool NAME")?;
    let arguments = json_arguments(flags)?;
    let output = flag_value(flags, "--output");
    let has_target = ["--app", "--pid", "--window-id", "--title"]
        .into_iter()
        .any(|flag| flag_value(flags, flag).is_some());
    let result = if has_target {
        let scope = select_scope(driver, flags).await?;
        let app = flag_value(flags, "--app").unwrap_or_else(|| "DCC application".into());
        let session_id =
            flag_value(flags, "--session").unwrap_or_else(|| "dcc-mcp-call-cli".into());
        let mut session = driver.session(scope, app, session_id)?;
        session.start().await?;
        let result = session.call_tool(&name, arguments).await;
        let stop_result = session.stop().await;
        let result = result?;
        stop_result?;
        result
    } else {
        driver.call_tool(&name, arguments).await?
    };
    if let (Some(path), Some(image)) = (output.as_deref(), result.images.first()) {
        fs::write(path, &image.data)?;
    }
    let mut value = result.value;
    if let Some(path) = output {
        value["_dcc_mcp_image_output"] = json!(path);
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

async fn host_call(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let method = flag_value(flags, "--method").ok_or("host-call requires --method NAME")?;
    let params = json_arguments(flags)?;
    let snapshot_transport = snapshot_transport(flags)?;
    let mut connection = connect_host(flags, snapshot_transport).await?;
    let response = connection.client_mut().request(method, params).await?;
    let output = flag_value(flags, "--output");
    if let Some(path) = output.as_deref() {
        fs::write(path, response_image(&response, snapshot_transport)?)?;
    }
    let mut value = response.value;
    if let Some(path) = output {
        value["_dcc_mcp_binary_output"] = json!(path);
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    connection.shutdown().await?;
    Ok(())
}

async fn host_batch(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let requests = parse_host_batch(json_arguments(flags)?)?;
    let snapshot_transport = snapshot_transport(flags)?;
    let mut connection = connect_host(flags, snapshot_transport).await?;
    let responses = connection.client_mut().request_batch(requests).await?;
    let output_dir = flag_value(flags, "--output-dir");
    if let Some(path) = output_dir.as_deref() {
        fs::create_dir_all(path)?;
    }

    let mut values = Vec::with_capacity(responses.len());
    for (index, response) in responses.into_iter().enumerate() {
        let has_image = response.binary_attachment.is_some()
            || response.value.get("image").is_some()
            || response.value.get("attachments").is_some();
        let mut value = response.value;
        if has_image {
            let directory = output_dir.as_deref().ok_or(
                "host-batch received image data; pass --output-dir or use metadata-only requests",
            )?;
            let path = format!("{directory}/response-{index}.bin");
            let bytes = response_image(
                &HostResponse {
                    value: value.clone(),
                    binary_attachment: response.binary_attachment,
                },
                snapshot_transport,
            )?;
            fs::write(&path, bytes)?;
            value["_dcc_mcp_binary_output"] = json!(path);
        }
        values.push(value);
    }
    println!("{}", serde_json::to_string_pretty(&values)?);
    connection.shutdown().await?;
    Ok(())
}

async fn host_jsonl(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot_transport = snapshot_transport(flags)?;
    let mut connection = connect_host(flags, snapshot_transport).await?;
    let output_dir = flag_value(flags, "--output-dir");
    if let Some(path) = output_dir.as_deref() {
        fs::create_dir_all(path)?;
    }
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();
    let mut output = BufWriter::new(stdout);
    let mut index = 0_usize;

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let request = match parse_jsonl_request(line) {
            Ok(request) => request,
            Err(error) => {
                write_jsonl_response(
                    &mut output,
                    json!({
                        "type": "error",
                        "code": "invalid_request",
                        "message": error,
                    }),
                )
                .await?;
                index = index.saturating_add(1);
                continue;
            }
        };
        let response = match connection
            .client_mut()
            .request(&request.method, request.params)
            .await
        {
            Ok(response) => match jsonl_response_value(response, output_dir.as_deref(), index) {
                Ok(value) => value,
                Err(error) => {
                    write_jsonl_response(
                        &mut output,
                        json!({
                            "type": "error",
                            "code": "output_error",
                            "message": error.to_string(),
                        }),
                    )
                    .await?;
                    return Err(error);
                }
            },
            Err(error @ HostClientError::Remote { .. }) => host_error_value(&error),
            Err(error) => {
                write_jsonl_response(&mut output, host_error_value(&error)).await?;
                return Err(error.into());
            }
        };
        write_jsonl_response(&mut output, response).await?;
        index = index.saturating_add(1);
    }
    connection.shutdown().await?;
    Ok(())
}

struct JsonlRequest {
    method: String,
    params: serde_json::Value,
}

fn parse_jsonl_request(line: &str) -> Result<JsonlRequest, String> {
    let value: serde_json::Value = serde_json::from_str(line)
        .map_err(|error| format!("JSONL request is invalid JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "JSONL request must be an object".to_owned())?;
    let method = object
        .get("method")
        .and_then(serde_json::Value::as_str)
        .filter(|method| !method.is_empty())
        .ok_or_else(|| "JSONL request requires a non-empty method".to_owned())?;
    let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
    if !params.is_object() {
        return Err("JSONL request params must be an object".to_owned());
    }
    Ok(JsonlRequest {
        method: method.to_owned(),
        params,
    })
}

fn jsonl_response_value(
    response: HostResponse,
    output_dir: Option<&str>,
    index: usize,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut value = response.value;
    if let Some(bytes) = response.binary_attachment {
        let directory = output_dir.ok_or(
            "JSONL response contains image bytes; pass --output-dir or negotiate shared_memory",
        )?;
        let path = format!("{directory}/response-{index}.bin");
        fs::write(&path, bytes)?;
        value["_dcc_mcp_binary_output"] = json!(path);
    }
    Ok(value)
}

fn host_error_value(error: &HostClientError) -> serde_json::Value {
    match error {
        HostClientError::Io(error) => json!({
            "type": "error",
            "code": "transport_error",
            "message": error.to_string(),
        }),
        HostClientError::Protocol(message) => json!({
            "type": "error",
            "code": "protocol_error",
            "message": message,
        }),
        HostClientError::Remote {
            code,
            message,
            response,
        } => {
            let mut value = response.clone();
            value["type"] = json!("error");
            value["code"] = json!(code);
            value["message"] = json!(message);
            value
        }
    }
}

async fn write_jsonl_response<W: AsyncWrite + Unpin>(
    output: &mut W,
    value: serde_json::Value,
) -> Result<(), std::io::Error> {
    let body =
        serde_json::to_vec(&value).map_err(|error| std::io::Error::other(error.to_string()))?;
    output.write_all(&body).await?;
    output.write_all(b"\n").await?;
    output.flush().await
}

fn parse_host_batch(
    value: serde_json::Value,
) -> Result<Vec<(String, serde_json::Value)>, Box<dyn std::error::Error>> {
    let requests = value
        .as_array()
        .ok_or("host-batch JSON must be an array of {method, params} objects")?;
    requests
        .iter()
        .enumerate()
        .map(|(index, request)| {
            let object = request
                .as_object()
                .ok_or_else(|| format!("host-batch request {index} must be an object"))?;
            let method = object
                .get("method")
                .and_then(serde_json::Value::as_str)
                .filter(|method| !method.is_empty())
                .ok_or_else(|| format!("host-batch request {index} requires method"))?;
            let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
            if !params.is_object() {
                return Err(format!("host-batch request {index} params must be an object").into());
            }
            Ok((method.to_owned(), params))
        })
        .collect()
}

fn snapshot_transport(flags: &[String]) -> Result<SnapshotTransport, Box<dyn std::error::Error>> {
    match flag_value(flags, "--snapshot-transport").as_deref() {
        None | Some("binary_frame") => Ok(SnapshotTransport::BinaryFrame),
        Some("shared_memory") => Ok(SnapshotTransport::SharedMemory),
        Some(value) => Err(format!("unsupported snapshot transport: {value}").into()),
    }
}

enum HostConnection {
    Endpoint(HostClient),
    Spawned(HostProcess),
}

impl HostConnection {
    fn client_mut(&mut self) -> &mut HostClient {
        match self {
            Self::Endpoint(client) => client,
            Self::Spawned(process) => process.client_mut(),
        }
    }

    async fn shutdown(self) -> Result<(), Box<dyn std::error::Error>> {
        if let Self::Spawned(process) = self {
            let status = process.shutdown().await?;
            if !status.success() {
                return Err(format!("spawned Host exited with {status}").into());
            }
        }
        Ok(())
    }
}

async fn connect_host(
    flags: &[String],
    snapshot_transport: SnapshotTransport,
) -> Result<HostConnection, Box<dyn std::error::Error>> {
    if let Some(binary_path) = flag_value(flags, "--spawn") {
        return Ok(HostConnection::Spawned(
            HostProcess::spawn(binary_path, "dcc-mcp-cua-cli", snapshot_transport).await?,
        ));
    }
    Ok(match flag_value(flags, "--endpoint") {
        Some(endpoint) => HostConnection::Endpoint(
            HostClient::connect_with_transport(endpoint, "dcc-mcp-cua-cli", snapshot_transport)
                .await?,
        ),
        None => HostConnection::Endpoint(
            HostClient::connect_default_with_transport("dcc-mcp-cua-cli", snapshot_transport)
                .await?,
        ),
    })
}

fn response_image(
    response: &HostResponse,
    snapshot_transport: SnapshotTransport,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if let Some(image) = response.binary_attachment.as_deref() {
        return Ok(image.to_vec());
    }
    if snapshot_transport != SnapshotTransport::SharedMemory {
        return Err("--output requires a binary image attachment".into());
    }
    let descriptor: SharedImageDescriptor = serde_json::from_value(
        response
            .value
            .get("image")
            .cloned()
            .ok_or("--output requires an image response")?,
    )?;
    Ok(SharedImageReader::open(descriptor)?.read()?)
}

fn json_arguments(flags: &[String]) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let inline = flag_value(flags, "--json");
    let file = flag_value(flags, "--json-file");
    if inline.is_some() && file.is_some() {
        return Err("use --json or --json-file, not both".into());
    }
    let source = match file.as_deref() {
        Some("-") => {
            let mut source = String::new();
            std::io::stdin().read_to_string(&mut source)?;
            source
        }
        Some(path) => fs::read_to_string(path)?,
        None => inline.unwrap_or_else(|| "{}".into()),
    };
    Ok(serde_json::from_str(&source)?)
}

async fn desktop_snapshot(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let output = flag_value(flags, "--output").unwrap_or_else(|| "desktop.png".into());
    let snapshot = driver.desktop_snapshot().await?;
    fs::write(&output, &snapshot.data)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "success": true,
            "output": output,
            "state": snapshot.state,
            "backend": "cua-driver-sdk",
        }))?
    );
    Ok(())
}

async fn screen_size(driver: &ComputerUseDriver) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&driver.screen_size().await?)?
    );
    Ok(())
}

async fn cursor_position(driver: &ComputerUseDriver) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&driver.cursor_position().await?)?
    );
    Ok(())
}

async fn launch_app(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let request = ComputerUseLaunchRequest {
        name: flag_value(flags, "--name"),
        bundle_id: flag_value(flags, "--bundle-id"),
        aumid: flag_value(flags, "--aumid"),
        path: flag_value(flags, "--path"),
        launch_path: flag_value(flags, "--launch-path"),
        urls: flag_values(flags, "--url"),
        additional_arguments: flag_values(flags, "--arg"),
        creates_new_application_instance: has_flag(flags, "--new-instance"),
        start_minimized: has_flag(flags, "--start-minimized"),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&driver.launch_app(&request).await?)?
    );
    Ok(())
}

async fn terminate_app(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    if !has_flag(flags, "--confirm") {
        return Err(
            "terminate requires --confirm because it force-closes the exact target process".into(),
        );
    }
    let scope = select_scope(driver, flags).await?;
    let app = flag_value(flags, "--app").unwrap_or_else(|| "DCC application".into());
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-mcp-cli".into());
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let result = session.terminate_app().await;
    if result.is_err() {
        let _ = session.stop().await;
    }
    println!("{}", serde_json::to_string_pretty(&result?)?);
    Ok(())
}

async fn clipboard_read(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let scope = select_scope(driver, flags).await?;
    let app = flag_value(flags, "--app").unwrap_or_else(|| "DCC application".into());
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-mcp-cli".into());
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let result = session
        .clipboard_read(has_flag(flags, "--include-text"))
        .await;
    let stop_result = session.stop().await;
    let result = result?;
    stop_result?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn clipboard_write(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let request = ComputerUseClipboardWriteRequest {
        text: flag_value(flags, "--text"),
        image_path: flag_value(flags, "--image-path"),
        file_path: flag_value(flags, "--file-path"),
    };
    let scope = select_scope(driver, flags).await?;
    let app = flag_value(flags, "--app").unwrap_or_else(|| "DCC application".into());
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-mcp-cli".into());
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let result = session.clipboard_write(&request).await;
    let stop_result = session.stop().await;
    let result = result?;
    stop_result?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn snapshot(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let scope = select_scope(driver, flags).await?;
    let app = flag_value(flags, "--app").unwrap_or_else(|| "DCC application".into());
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-mcp-cli".into());
    let output = flag_value(flags, "--output").unwrap_or_else(|| "screenshot.png".into());
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let result = async {
        maybe_escalate(&mut session, flags).await?;
        session.screenshot().await
    }
    .await;
    let stop_result = session.stop().await;
    let screenshot = result?;
    stop_result?;
    fs::write(&output, &screenshot.data)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "success": true,
            "observation": screenshot.observation,
            "output": output,
            "backend": "cua-driver-sdk",
        }))?
    );
    Ok(())
}

async fn act(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let scope = select_scope(driver, flags).await?;
    let app = flag_value(flags, "--app").unwrap_or_else(|| "DCC application".into());
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-mcp-cli".into());
    let action_json = flag_value(flags, "--action-json")
        .ok_or("act requires --action-json with a ComputerUseAction JSON object")?;
    let mut action: ComputerUseAction = serde_json::from_str(&action_json)?;
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let result = async {
        maybe_escalate(&mut session, flags).await?;
        let screenshot = session.screenshot().await?;
        action.observation_id = Some(screenshot.observation.observation_id.clone());
        let action_result = session.perform_action(&action).await?;
        Ok::<_, dcc_mcp_cua_core::ComputerUseError>(json!({
            "success": true,
            "observation": screenshot.observation,
            "action": action_result,
        }))
    }
    .await;
    let stop_result = session.stop().await;
    let result = result?;
    stop_result?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn verify_state(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let scope = select_scope(driver, flags).await?;
    let app = flag_value(flags, "--app").unwrap_or_else(|| "DCC application".into());
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-mcp-verify-cli".into());
    let expect_json = flag_value(flags, "--expect-json")
        .ok_or("verify requires --expect-json with a JSON predicate array")?;
    let expect = serde_json::from_str(&expect_json)?;
    let timeout_ms = flag_value(flags, "--timeout-ms")
        .map(|value| value.parse::<u64>())
        .transpose()?;
    let stable_samples = flag_value(flags, "--stable-samples")
        .map(|value| value.parse::<u64>())
        .transpose()?;
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let verification = session
        .verify_state(expect, timeout_ms, stable_samples, false)
        .await;
    let stop_result = session.stop().await;
    let result = verification?.value;
    stop_result?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn desktop_act(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let action_json = flag_value(flags, "--action-json")
        .ok_or("desktop-act requires --action-json with a raw coordinate action")?;
    let mut action: ComputerUseAction = serde_json::from_str(&action_json)?;
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-mcp-desktop-cli".into());
    let mut session = driver.desktop_session(session_id.clone())?;
    session.start().await?;
    let result = async {
        let snapshot = session.screenshot().await?;
        action.observation_id = Some(snapshot.observation_id.clone());
        let action_result = session.perform_action(&action).await?;
        Ok::<_, dcc_mcp_cua_core::ComputerUseError>(json!({
            "success": true,
            "session_id": session_id,
            "observation_id": snapshot.observation_id,
            "state": snapshot.state,
            "action": action_result,
        }))
    }
    .await;
    let stop_result = session.stop().await;
    let result = result?;
    stop_result?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn doctor(driver: &ComputerUseDriver) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = driver.raw().metadata().await?;
    let windows = driver.list_windows().await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "backend": "cua-driver-sdk",
            "driver": metadata,
            "window_count": windows.len(),
            "host_endpoint": HostTransport::default_endpoint(),
        }))?
    );
    Ok(())
}

async fn maybe_escalate(
    session: &mut dcc_mcp_cua_core::ComputerUseSession,
    flags: &[String],
) -> dcc_mcp_cua_core::ComputerUseResult<Option<serde_json::Value>> {
    if !has_flag(flags, "--escalate") {
        return Ok(None);
    }
    let reason = flag_value(flags, "--escalation-reason").unwrap_or_else(|| "other".into());
    let detail = flag_value(flags, "--escalation-detail");
    session.escalate(&reason, detail.as_deref()).await.map(Some)
}

async fn select_scope(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<ComputerUseTargetScope, Box<dyn std::error::Error>> {
    let pid = flag_value(flags, "--pid")
        .map(|value| value.parse::<u32>())
        .transpose()?;
    let window_handle = flag_value(flags, "--window-id")
        .map(|value| value.parse::<u64>())
        .transpose()?;
    let title = flag_value(flags, "--title");
    if pid.is_some() || window_handle.is_some() || title.is_some() {
        return Ok(ComputerUseTargetScope {
            process_id: pid,
            window_handle,
            window_title: title,
        });
    }
    let app =
        flag_value(flags, "--app").ok_or("a target requires --app or --pid/--window-id/--title")?;
    let rows = driver.list_windows().await?;
    let matches = rows
        .iter()
        .filter(|row| {
            row["app_name"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(&app))
                && row["is_on_screen"] == true
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!(
            "expected one on-screen {app} window, found {}",
            matches.len()
        )
        .into());
    }
    let row = matches[0];
    Ok(ComputerUseTargetScope {
        process_id: Some(row["pid"].as_u64().ok_or("window is missing pid")? as u32),
        window_handle: Some(
            row["window_id"]
                .as_u64()
                .ok_or("window is missing window_id")?,
        ),
        window_title: row["title"].as_str().map(str::to_owned),
    })
}

fn flag_value(flags: &[String], name: &str) -> Option<String> {
    flags
        .iter()
        .position(|flag| flag == name)
        .and_then(|index| flags.get(index + 1))
        .cloned()
}

fn flag_values(flags: &[String], name: &str) -> Vec<String> {
    flags
        .windows(2)
        .filter(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .collect()
}

fn has_flag(flags: &[String], name: &str) -> bool {
    flags.iter().any(|flag| flag == name)
}

fn print_help() {
    println!(
        "dcc-mcp-cua\n\n  list [--app APP]\n  apps\n  tools\n  call --tool NAME [--json JSON|--json-file PATH] [--app APP|--pid PID --window-id ID] [--output FILE]\n  host-call --method NAME [--json JSON|--json-file PATH] [--endpoint PATH|--spawn BINARY] [--snapshot-transport binary_frame|shared_memory] [--output FILE]\n  host-batch --json JSON_ARRAY [--endpoint PATH|--spawn BINARY] [--snapshot-transport binary_frame|shared_memory] [--output-dir DIR]\n  host-jsonl [--endpoint PATH|--spawn BINARY] [--snapshot-transport binary_frame|shared_memory] [--output-dir DIR]\n  desktop-snapshot [--output FILE]\n  screen-size\n  cursor-position\n  launch --name NAME|--bundle-id ID|--aumid ID|--path PATH|--launch-path PATH [--url URL] [--arg ARG] [--new-instance] [--start-minimized]\n  terminate --app APP --confirm\n  snapshot --app APP [--output FILE]\n  act --app APP --action-json JSON\n  verify --app APP --expect-json JSON [--timeout-ms N] [--stable-samples N]\n  desktop-act --action-json JSON [--session ID]\n  clipboard-read --app APP [--include-text]\n  clipboard-write --app APP --text TEXT|--image-path FILE|--file-path FILE\n  doctor\n  host [--stdio|--endpoint PATH]\n\nHost uses versioned big-endian JSON frames. Hello version 1 negotiates binary-frame or shared-memory snapshots and supports request_id correlation."
    );
    println!(
        "Window snapshots/actions accept --escalate --escalation-reason REASON when an explicit desktop visual fallback approval is required."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_image_reads_host_owned_shared_memory() {
        let image = dcc_mcp_cua_shm::SharedImage::from_bytes(b"png", "image/png").unwrap();
        let response = HostResponse {
            value: json!({"image": image.descriptor()}),
            binary_attachment: None,
        };
        assert_eq!(
            response_image(&response, SnapshotTransport::SharedMemory).unwrap(),
            b"png"
        );
    }

    #[test]
    fn host_batch_parser_requires_read_only_request_shapes() {
        let requests = parse_host_batch(json!([
            {"method":"list_apps"},
            {"method":"screen_size","params":{}}
        ]))
        .unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].0, "list_apps");
        assert!(parse_host_batch(json!({"method":"list_apps"})).is_err());
        assert!(
            parse_host_batch(json!([
                {"method":"screen_size","params":[]}
            ]))
            .is_err()
        );
    }

    #[test]
    fn jsonl_parser_requires_method_and_object_params() {
        let request = parse_jsonl_request(r#"{"method":"list_apps"}"#).unwrap();
        assert_eq!(request.method, "list_apps");
        assert_eq!(request.params, json!({}));
        assert!(parse_jsonl_request("[]").is_err());
        assert!(parse_jsonl_request(r#"{"method":"list_apps","params":[]}"#).is_err());
    }
}
