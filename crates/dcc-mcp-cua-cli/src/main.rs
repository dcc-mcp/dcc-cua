use std::env;
use std::fs;

use dcc_mcp_cua_core::{
    ComputerUseAction, ComputerUseClipboardWriteRequest, ComputerUseDriver,
    ComputerUseLaunchRequest, ComputerUseTargetScope,
};
use dcc_mcp_cua_host::{HostTransport, run as run_host};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    let flags = args.collect::<Vec<_>>();
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
    let mut windows = driver.list_windows().await?;
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
    let arguments = serde_json::from_str::<serde_json::Value>(
        &flag_value(flags, "--json").unwrap_or_else(|| "{}".into()),
    )?;
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
    let result = session.screenshot().await;
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
        "dcc-mcp-cua\n\n  list [--app APP]\n  apps\n  tools\n  call --tool NAME [--json JSON] [--app APP|--pid PID --window-id ID] [--output FILE]\n  desktop-snapshot [--output FILE]\n  screen-size\n  cursor-position\n  launch --name NAME|--bundle-id ID|--aumid ID|--path PATH|--launch-path PATH [--url URL] [--arg ARG] [--new-instance] [--start-minimized]\n  terminate --app APP --confirm\n  snapshot --app APP [--output FILE]\n  act --app APP --action-json JSON\n  verify --app APP --expect-json JSON [--timeout-ms N] [--stable-samples N]\n  desktop-act --action-json JSON [--session ID]\n  clipboard-read --app APP [--include-text]\n  clipboard-write --app APP --text TEXT|--image-path FILE|--file-path FILE\n  doctor\n  host [--stdio|--endpoint PATH]\n\nHost uses versioned big-endian JSON frames. Hello version 1 negotiates binary-frame or shared-memory snapshots and supports request_id correlation."
    );
}
