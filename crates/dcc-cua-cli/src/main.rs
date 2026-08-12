use std::env;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::{collections::BTreeMap, time::Instant};

mod actions;
mod authorization;
mod cli_args;
mod host_lifecycle;
mod manifest;
mod profile_package;
mod profile_state;
mod semantic_profile;
mod update;

use actions::{
    act, action_result_value, activate_window, desktop_act, friendly_action, invoke_menu,
    is_friendly_action, semantic_post_snapshot_value, set_window_frame, verify_state,
    window_post_snapshot_value, zoom,
};
use cli_args::*;

use dcc_cua_client::{
    HostClient, HostClientError, HostProcess, HostResponse, MAX_REQUEST_ID_CHARS,
    SnapshotTransport, is_parallel_discovery_method,
};
use dcc_cua_core::{
    ComputerUseAction, ComputerUseClipboardWriteRequest, ComputerUseDriver,
    ComputerUseLaunchRequest, ComputerUseMenuRequest, ComputerUseTargetScope,
    ComputerUseToolResult, ComputerUseWindowFrameRequest, ComputerUseWindowQuery,
    ComputerUseWindowWaitRequest, ComputerUseZoomRequest,
};
use dcc_cua_host::{HostTransport, MAX_PARALLEL_DISCOVERY_REQUESTS, run as run_host};
use dcc_cua_semantic_profiles::{
    SemanticProfile, builtin_profile, builtin_profiles, parse_profile,
};
use dcc_cua_shm::{SharedImageDescriptor, SharedImageReader};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};

const PARALLEL_DISCOVERY_WINDOW_MS: u64 = 5;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    {
        let arguments = env::args().skip(1).collect::<Vec<_>>();
        if let Some(generation) = private_worker_generation_from(&arguments)? {
            dcc_cua_core::run_private_worker_with_appkit(generation)
                .map_err(std::io::Error::other)?;
            return Ok(());
        }
    }
    async_main()
}

#[tokio::main]
async fn async_main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    let flags = args.collect::<Vec<_>>();
    if is_help_request(&command, &flags) {
        print_help();
        return Ok(());
    }
    if command == "__private-worker" {
        let generation =
            flag_value(&flags, "--generation").ok_or("private worker requires --generation")?;
        dcc_cua_core::run_private_worker(generation)
            .await
            .map_err(std::io::Error::other)?;
        return Ok(());
    }
    if command == "host-call" {
        host_call(&flags).await?;
        return Ok(());
    }
    if command == "ping" {
        host_ping(&flags).await?;
        return Ok(());
    }
    if command == "interrupt-all" {
        host_interrupt_all(&flags).await?;
        return Ok(());
    }
    if command == "doctor"
        && flags
            .iter()
            .any(|flag| flag == "--spawn" || flag == "--endpoint")
    {
        host_doctor(&flags).await?;
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
    if command == "host-ensure" {
        let endpoint =
            flag_value(&flags, "--endpoint").unwrap_or_else(HostTransport::default_endpoint);
        let response = host_lifecycle::ensure(endpoint, &flags).await?;
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }
    if command == "manifest" {
        println!("{}", serde_json::to_string_pretty(&manifest::document())?);
        return Ok(());
    }
    if command == "profiles" {
        list_semantic_profiles()?;
        return Ok(());
    }
    if command == "profile" && profile_package::is_management_command(&flags) {
        profile_package::execute(&flags)?;
        return Ok(());
    }
    if command == "profile" && flags.first().is_some_and(|value| value == "match") {
        match_semantic_profile(&flags)?;
        return Ok(());
    }
    if command == "profile" && flag_value(&flags, "--app").is_none() {
        inspect_semantic_profile(&flags)?;
        return Ok(());
    }
    if command == "profile-state" {
        profile_state::execute(&flags).await?;
        return Ok(());
    }
    if command == "update" {
        tokio::task::spawn_blocking(move || update::run(&flags))
            .await
            .map_err(|error| std::io::Error::other(format!("update worker failed: {error}")))?
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        return Ok(());
    }
    let driver = if command == "host" {
        host_driver(&flags)?
    } else {
        ComputerUseDriver::create()?
    };

    match command.as_str() {
        "list" => list_windows(&driver, &flags).await?,
        "wait-window" => wait_window(&driver, &flags).await?,
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
        "accessibility" => accessibility_snapshot(&driver, &flags).await?,
        "window-state" => window_state(&driver, &flags).await?,
        "activate" => activate_window(&driver, &flags).await?,
        "set-window-frame" => set_window_frame(&driver, &flags).await?,
        "invoke-menu" => invoke_menu(&driver, &flags).await?,
        "zoom" => zoom(&driver, &flags).await?,
        "verify" => verify_state(&driver, &flags).await?,
        "act" => act(&driver, &flags).await?,
        "desktop-act" => desktop_act(&driver, &flags).await?,
        "doctor" => doctor(&driver, &flags).await?,
        "profile" => semantic_profile::execute(&driver, &flags).await?,
        friendly if is_friendly_action(friendly) => {
            friendly_action(&driver, &flags, friendly).await?
        }
        other => return Err(format!("unknown command: {other}; use `help`").into()),
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn private_worker_generation_from(arguments: &[String]) -> Result<Option<String>, String> {
    if arguments.first().map(String::as_str) != Some("__private-worker") {
        return Ok(None);
    }
    let flags = &arguments[1..];
    flag_value(flags, "--generation")
        .or_else(|| {
            flags
                .iter()
                .find_map(|flag| flag.strip_prefix("--generation=").map(str::to_owned))
        })
        .ok_or_else(|| "private worker requires --generation".to_owned())
        .map(Some)
}

fn host_driver(flags: &[String]) -> Result<ComputerUseDriver, Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    return authorization::driver_for_private_worker(flags, &macos_private_worker_binary()?);
    #[cfg(not(target_os = "macos"))]
    authorization::driver_for_host(flags)
}

#[cfg(target_os = "macos")]
fn macos_private_worker_binary() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(fs::canonicalize(env::current_exe()?)?)
}

async fn list_windows(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let pid = flag_value(flags, "--pid")
        .map(|value| value.parse::<u32>())
        .transpose()?;
    let app = flag_value(flags, "--app");
    let window_id = flag_value(flags, "--window-id")
        .map(|value| value.parse::<u64>())
        .transpose()?;
    let title = flag_value(flags, "--title");
    let query = ComputerUseWindowQuery {
        app,
        process_id: pid,
        window_handle: window_id,
        window_title: title,
        on_screen_only: has_flag(flags, "--on-screen"),
    };
    query.validate_selectors()?;
    let mut windows = driver
        .list_windows_filtered(query.process_id, query.on_screen_only)
        .await?;
    windows.retain(|window| query.matches_window(window));
    println!("{}", serde_json::to_string_pretty(&windows)?);
    Ok(())
}

fn list_semantic_profiles() -> Result<(), Box<dyn std::error::Error>> {
    let mut profiles = builtin_profiles()
        .iter()
        .map(|profile| {
            json!({
                "id": profile.id,
                "display_name": profile.display_name,
                "source": "builtin",
                "version": profile.profile_version,
                "application": profile.application,
                "extends": profile.extends,
                "status": "ready",
                "preferred_route": profile.settings.preferred_route,
                "dialog_style": profile.settings.dialog_style,
                "supported_locales": profile.supported_locales(),
                "surface_count": profile.surfaces.len(),
                "state_source_count": profile.state_sources.len(),
            })
        })
        .collect::<Vec<_>>();
    profiles.extend(profile_package::installed_profile_summaries(None)?);
    profiles.sort_by(|left, right| {
        left.get("id")
            .and_then(serde_json::Value::as_str)
            .cmp(&right.get("id").and_then(serde_json::Value::as_str))
            .then_with(|| {
                right
                    .get("source")
                    .and_then(serde_json::Value::as_str)
                    .cmp(&left.get("source").and_then(serde_json::Value::as_str))
            })
    });
    println!("{}", serde_json::to_string_pretty(&profiles)?);
    Ok(())
}

fn inspect_semantic_profile(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let profile = load_semantic_profile(flags)?;
    println!("{}", serde_json::to_string_pretty(&profile)?);
    Ok(())
}

fn match_semantic_profile(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let application_name =
        flag_value(flags, "--app").ok_or("profile match requires --app APPLICATION_NAME")?;
    let window_title = flag_value(flags, "--title").unwrap_or_default();

    let mut catalog = builtin_profiles()
        .iter()
        .cloned()
        .map(|profile| (profile.id.clone(), ("builtin", profile)))
        .collect::<BTreeMap<_, _>>();
    for profile in profile_package::installed_ready_profiles(None)? {
        // This mirrors explicit loading: an installed Profile with the same ID
        // is the user's intentional override of a built-in.
        catalog.insert(profile.id.clone(), ("user", profile));
    }

    let candidates = catalog
        .into_values()
        .map(|(source, profile)| (source.to_owned(), profile))
        .collect::<Vec<_>>();
    let result = profile_match_result(candidates, &application_name, &window_title);
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn profile_match_result(
    catalog: Vec<(String, SemanticProfile)>,
    application_name: &str,
    window_title: &str,
) -> serde_json::Value {
    let mut candidates = catalog
        .into_iter()
        .filter(|(_, profile)| profile.matches_window(application_name, window_title))
        .map(|(source, profile)| {
            let version_specific = !profile.application.versions.is_empty();
            (source, profile, version_specific)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .2
            .cmp(&left.2)
            .then_with(|| {
                left.1
                    .application
                    .versions
                    .len()
                    .cmp(&right.1.application.versions.len())
            })
            .then_with(|| right.0.cmp(&left.0))
            .then_with(|| left.1.id.cmp(&right.1.id))
    });

    let top_specificity = candidates
        .first()
        .map(|(_, profile, specific)| (*specific, profile.application.versions.len()));
    let top_count = top_specificity.map_or(0, |specificity| {
        candidates
            .iter()
            .take_while(|(_, profile, specific)| {
                (*specific, profile.application.versions.len()) == specificity
            })
            .count()
    });
    let selected = (top_count == 1).then(|| candidates[0].1.id.clone());
    let candidates = candidates
        .into_iter()
        .map(|(source, profile, version_specific)| {
            json!({
                "id": profile.id,
                "profile_version": profile.profile_version,
                "application": profile.application,
                "extends": profile.extends,
                "source": source,
                "version_specific": version_specific,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "application_name": application_name,
        "window_title": window_title,
        "selected": selected,
        "ambiguous": top_count > 1,
        "candidates": candidates,
    })
}

fn load_semantic_profile(flags: &[String]) -> Result<SemanticProfile, Box<dyn std::error::Error>> {
    if let Some(path) = flag_value(flags, "--profile-file") {
        let input = fs::read_to_string(&path)?;
        let profile = parse_profile(&input)?;
        return Ok(profile_package::resolve_profile_with_store(profile, None)?);
    }
    let id =
        flag_value(flags, "--id").ok_or("profile requires --id PROFILE or --profile-file PATH")?;
    if let Some(profile) = profile_package::load_installed_profile(&id, None)? {
        return Ok(profile);
    }
    builtin_profile(&id)
        .cloned()
        .ok_or_else(|| format!("unknown semantic profile: {id}").into())
}

async fn wait_window(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let request = window_wait_request(flags)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&driver.wait_for_window(&request).await?)?
    );
    Ok(())
}

fn window_wait_request(
    flags: &[String],
) -> Result<ComputerUseWindowWaitRequest, Box<dyn std::error::Error>> {
    let request = ComputerUseWindowWaitRequest {
        query: ComputerUseWindowQuery {
            app: flag_value(flags, "--app"),
            process_id: flag_value(flags, "--pid")
                .map(|value| value.parse::<u32>())
                .transpose()?,
            window_handle: flag_value(flags, "--window-id")
                .map(|value| value.parse::<u64>())
                .transpose()?,
            window_title: flag_value(flags, "--title"),
            on_screen_only: has_flag(flags, "--on-screen"),
        },
        timeout_ms: flag_value(flags, "--timeout-ms")
            .map(|value| value.parse::<u64>())
            .transpose()?,
        interval_ms: flag_value(flags, "--poll-ms")
            .map(|value| value.parse::<u64>())
            .transpose()?,
    };
    request.query.validate()?;
    Ok(request)
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
        let app = application_label(flags);
        let session_id =
            flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-call-cli".into());
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
        value["_dcc_cua_image_output"] = json!(path);
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
        value["_dcc_cua_binary_output"] = json!(path);
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    connection.shutdown().await?;
    Ok(())
}

async fn host_ping(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot_transport = snapshot_transport(flags)?;
    let mut connection = connect_host(flags, snapshot_transport).await?;
    let response = connection.client_mut().ping().await?;
    println!("{}", serde_json::to_string_pretty(&response.value)?);
    connection.shutdown().await?;
    Ok(())
}

async fn host_interrupt_all(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if flag_value(flags, "--spawn").is_some() {
        return Err("interrupt-all must target an existing Host endpoint".into());
    }
    let mut connection = connect_host(flags, SnapshotTransport::BinaryFrame).await?;
    let response = connection.client_mut().interrupt_all().await?;
    println!("{}", serde_json::to_string_pretty(&response.value)?);
    connection.shutdown().await
}

async fn host_doctor(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot_transport = snapshot_transport(flags)?;
    let mut connection = connect_host(flags, snapshot_transport).await?;
    let response = connection.client_mut().doctor().await?;
    println!("{}", serde_json::to_string_pretty(&response.value)?);
    connection.shutdown().await?;
    let route = doctor_route(flags)?;
    if !diagnostic_route_ready(&response.value, route) {
        return Err(format!("CUA Host {route} diagnostics are not ready").into());
    }
    Ok(())
}

async fn host_batch(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let requests = parse_host_batch(json_arguments(flags)?)?;
    let snapshot_transport = snapshot_transport(flags)?;
    let mut connection = connect_host(flags, snapshot_transport).await?;
    let requests = requests.into_iter().enumerate().map(|(index, request)| {
        (
            request
                .request_id
                .unwrap_or_else(|| format!("host-batch-{index}")),
            request.method,
            request.params,
        )
    });
    let responses = connection
        .client_mut()
        .request_batch_with_ids(requests)
        .await?;
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
            value["_dcc_cua_binary_output"] = json!(path);
        }
        values.push(value);
    }
    println!("{}", serde_json::to_string_pretty(&values)?);
    connection.shutdown().await?;
    Ok(())
}

async fn host_jsonl(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let mut metrics = HostJsonlMetrics::with_output(
        flag_value(flags, "--metrics-output").map(PathBuf::from),
        started,
    );
    let run_result = match metrics.checkpoint() {
        Ok(()) => host_jsonl_inner(flags, &mut metrics).await,
        Err(error) => Err(error),
    };
    let report_result = metrics.finish(run_result.is_ok());
    match (run_result, report_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(run_error), Err(report_error)) => Err(format!(
            "host-jsonl failed: {run_error}; metrics report failed: {report_error}"
        )
        .into()),
    }
}

async fn host_jsonl_inner(
    flags: &[String],
    metrics: &mut HostJsonlMetrics,
) -> Result<(), Box<dyn std::error::Error>> {
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
    let parallel_discovery = has_flag(flags, "--parallel-discovery");
    let showcase_dir = showcase_directory(flags)?;
    let mut pending_line = None;

    loop {
        let (line_index, line) = match pending_line.take() {
            Some(value) => value,
            None => {
                let Some(line) = lines.next_line().await? else {
                    break;
                };
                if line.trim().is_empty() {
                    continue;
                }
                let line_index = index;
                index = index.saturating_add(1);
                (line_index, line)
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        metrics.record_input(line);
        let mut request = match parse_jsonl_request(line) {
            Ok(request) => request,
            Err(error) => {
                write_jsonl_response(
                    &mut output,
                    json!({
                        "type": "error",
                        "code": "invalid_request",
                        "message": error,
                    }),
                    metrics,
                )
                .await?;
                continue;
            }
        };
        metrics.record_request(&request);
        if let Some(directory) = showcase_dir.as_deref() {
            enable_showcase(&mut request, directory, line_index)?;
        }

        if parallel_discovery && is_parallel_discovery_method(&request.method) {
            let mut batch = vec![(line_index, request)];
            while batch.len() < MAX_PARALLEL_DISCOVERY_REQUESTS {
                let next_line = match tokio::time::timeout(
                    std::time::Duration::from_millis(PARALLEL_DISCOVERY_WINDOW_MS),
                    lines.next_line(),
                )
                .await
                {
                    Ok(line) => line?,
                    Err(_) => break,
                };
                let Some(next_line) = next_line else {
                    break;
                };
                if next_line.trim().is_empty() {
                    continue;
                }
                let next_index = index;
                index = index.saturating_add(1);
                match parse_jsonl_request(next_line.trim()) {
                    Ok(next_request) if is_parallel_discovery_method(&next_request.method) => {
                        metrics.record_input(next_line.trim());
                        metrics.record_request(&next_request);
                        batch.push((next_index, next_request));
                    }
                    Ok(_) => {
                        pending_line = Some((next_index, next_line));
                        break;
                    }
                    Err(error) => {
                        metrics.record_input(next_line.trim());
                        write_jsonl_response(
                            &mut output,
                            json!({
                                "type": "error",
                                "code": "invalid_request",
                                "message": error,
                            }),
                            metrics,
                        )
                        .await?;
                    }
                }
            }

            let mut metadata = Vec::with_capacity(batch.len());
            let mut requests = Vec::with_capacity(batch.len());
            for (request_index, request) in batch {
                let request_id = request
                    .request_id
                    .unwrap_or_else(|| format!("host-jsonl-{request_index}"));
                metadata.push((request_index, request.method.clone()));
                requests.push((request_id, request.method, request.params));
            }
            let host_results = connection
                .client_mut()
                .request_batch_with_ids_all(requests)
                .await?;
            for ((request_index, request_method), host_result) in
                metadata.into_iter().zip(host_results)
            {
                let response = match host_result {
                    Ok(response) => {
                        match measured_jsonl_response_value(
                            response,
                            output_dir.as_deref(),
                            request_index,
                            metrics,
                            &request_method,
                        ) {
                            Ok(value) => value,
                            Err(error) => {
                                write_jsonl_response(
                                    &mut output,
                                    json!({
                                        "type": "error",
                                        "code": "output_error",
                                        "message": error.to_string(),
                                    }),
                                    metrics,
                                )
                                .await?;
                                return Err(error);
                            }
                        }
                    }
                    Err(error @ HostClientError::Remote { .. }) => {
                        let value = host_error_value(&error);
                        metrics.record_response(&request_method, &value);
                        value
                    }
                    Err(error) => {
                        let value = host_error_value(&error);
                        metrics.record_response(&request_method, &value);
                        write_jsonl_response(&mut output, value, metrics).await?;
                        return Err(error.into());
                    }
                };
                write_jsonl_response(&mut output, response, metrics).await?;
            }
            continue;
        }

        let request_method = request.method.clone();
        let host_result = match request.request_id {
            Some(request_id) => {
                connection
                    .client_mut()
                    .request_with_id(request_id, request.method, request.params)
                    .await
            }
            None => {
                connection
                    .client_mut()
                    .request(request.method, request.params)
                    .await
            }
        };
        let response = match host_result {
            Ok(response) => match measured_jsonl_response_value(
                response,
                output_dir.as_deref(),
                line_index,
                metrics,
                &request_method,
            ) {
                Ok(value) => value,
                Err(error) => {
                    write_jsonl_response(
                        &mut output,
                        json!({
                            "type": "error",
                            "code": "output_error",
                            "message": error.to_string(),
                        }),
                        metrics,
                    )
                    .await?;
                    return Err(error);
                }
            },
            Err(error @ HostClientError::Remote { .. }) => {
                let value = host_error_value(&error);
                metrics.record_response(&request_method, &value);
                value
            }
            Err(error) => {
                let value = host_error_value(&error);
                metrics.record_response(&request_method, &value);
                write_jsonl_response(&mut output, value, metrics).await?;
                return Err(error.into());
            }
        };
        write_jsonl_response(&mut output, response, metrics).await?;
    }
    connection.shutdown().await?;
    Ok(())
}

struct JsonlRequest {
    request_id: Option<String>,
    method: String,
    params: serde_json::Value,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct HostJsonlImageMetrics {
    images_total: u64,
    pixels_total: u64,
    encoded_bytes_total: u64,
    unknown_dimensions_total: u64,
}

struct JsonlResponseOutput {
    value: serde_json::Value,
    image_metrics: HostJsonlImageMetrics,
}

#[derive(Debug, Default)]
struct HostJsonlMetrics {
    input_lines_total: u64,
    requests_total: u64,
    action_requests_total: u64,
    action_attempts_total: u64,
    action_succeeded_total: u64,
    action_rejected_total: u64,
    post_action_snapshot_requests_total: u64,
    visual_observation_requests_total: u64,
    semantic_observation_requests_total: u64,
    post_action_observation_requests_total: u64,
    standalone_snapshot_requests_total: u64,
    errors_total: u64,
    json_input_bytes: u64,
    json_output_bytes: u64,
    image_outputs_total: u64,
    image_pixels_total: u64,
    image_encoded_bytes_total: u64,
    image_unknown_dimensions_total: u64,
    live_observation_start_requests_total: u64,
    live_observation_stop_requests_total: u64,
    live_observation_final_states_total: u64,
    methods: BTreeMap<String, u64>,
    action_kinds: BTreeMap<String, u64>,
    error_codes: BTreeMap<String, u64>,
    checkpoint: Option<HostJsonlMetricsCheckpoint>,
}

#[derive(Debug)]
struct HostJsonlMetricsReport<'a> {
    schema: &'static str,
    run_status: HostJsonlRunStatus,
    transport_success: Option<bool>,
    elapsed_ms: u64,
    input_lines_total: u64,
    requests_total: u64,
    action_requests_total: u64,
    action_attempts_total: u64,
    action_succeeded_total: u64,
    action_rejected_total: u64,
    post_action_snapshot_requests_total: u64,
    visual_observation_requests_total: u64,
    semantic_observation_requests_total: u64,
    post_action_observation_requests_total: u64,
    standalone_snapshot_requests_total: u64,
    errors_total: u64,
    json_input_bytes: u64,
    json_output_bytes: u64,
    image_outputs_total: u64,
    image_pixels_total: u64,
    image_encoded_bytes_total: u64,
    image_unknown_dimensions_total: u64,
    live_observation_start_requests_total: u64,
    live_observation_stop_requests_total: u64,
    live_observation_final_states_total: u64,
    methods: &'a BTreeMap<String, u64>,
    action_kinds: &'a BTreeMap<String, u64>,
    error_codes: &'a BTreeMap<String, u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostJsonlRunStatus {
    Running,
    Succeeded,
    Failed,
}

impl HostJsonlRunStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    const fn transport_success(self) -> Option<bool> {
        match self {
            Self::Running => None,
            Self::Succeeded => Some(true),
            Self::Failed => Some(false),
        }
    }
}

#[derive(Debug)]
struct HostJsonlMetricsCheckpoint {
    path: PathBuf,
    started: Instant,
}

impl HostJsonlMetrics {
    fn with_output(path: Option<PathBuf>, started: Instant) -> Self {
        Self {
            checkpoint: path.map(|path| HostJsonlMetricsCheckpoint { path, started }),
            ..Self::default()
        }
    }

    fn record_input(&mut self, line: &str) {
        self.input_lines_total = self.input_lines_total.saturating_add(1);
        self.json_input_bytes = self
            .json_input_bytes
            .saturating_add(line.len().try_into().unwrap_or(u64::MAX));
    }

    fn record_request(&mut self, request: &JsonlRequest) {
        self.requests_total = self.requests_total.saturating_add(1);
        *self.methods.entry(request.method.clone()).or_default() += 1;
        if is_action_request(&request.method) {
            self.action_requests_total = self.action_requests_total.saturating_add(1);
            self.action_attempts_total = self.action_attempts_total.saturating_add(1);
            if let Some(kind) = request
                .params
                .get("action")
                .and_then(|action| action.get("action"))
                .and_then(serde_json::Value::as_str)
            {
                *self.action_kinds.entry(kind.to_owned()).or_default() += 1;
            }
        }
        if matches!(
            request.method.as_str(),
            "execute_action" | "execute_desktop_action"
        ) && request
            .params
            .get("capture_after")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            self.post_action_snapshot_requests_total =
                self.post_action_snapshot_requests_total.saturating_add(1);
            self.post_action_observation_requests_total = self
                .post_action_observation_requests_total
                .saturating_add(1);
            self.visual_observation_requests_total =
                self.visual_observation_requests_total.saturating_add(1);
        }
        if is_standalone_snapshot_request(&request.method) {
            self.standalone_snapshot_requests_total =
                self.standalone_snapshot_requests_total.saturating_add(1);
        }
        if is_visual_observation_request(request) {
            self.visual_observation_requests_total =
                self.visual_observation_requests_total.saturating_add(1);
        }
        if is_semantic_observation_request(&request.method) {
            self.semantic_observation_requests_total =
                self.semantic_observation_requests_total.saturating_add(1);
        }
        if request.method == "live_observation_start" {
            self.live_observation_start_requests_total =
                self.live_observation_start_requests_total.saturating_add(1);
        }
        if request.method == "live_observation_stop" {
            self.live_observation_stop_requests_total =
                self.live_observation_stop_requests_total.saturating_add(1);
        }
    }

    fn record_output(&mut self, value: &serde_json::Value, body_bytes: usize) {
        self.json_output_bytes = self
            .json_output_bytes
            .saturating_add(body_bytes.try_into().unwrap_or(u64::MAX));
        if value.get("type").and_then(serde_json::Value::as_str) == Some("error") {
            self.errors_total = self.errors_total.saturating_add(1);
            if let Some(code) = value.get("code").and_then(serde_json::Value::as_str) {
                *self.error_codes.entry(code.to_owned()).or_default() += 1;
            }
        }
    }

    fn record_response(&mut self, method: &str, value: &serde_json::Value) {
        if is_action_request(method) {
            if is_rejected_action_response(value) {
                self.action_rejected_total = self.action_rejected_total.saturating_add(1);
            } else {
                self.action_succeeded_total = self.action_succeeded_total.saturating_add(1);
            }
        }
        if method == "live_observation_stop"
            && value.get("type").and_then(serde_json::Value::as_str)
                == Some("live_observation_stopped")
            && value
                .get("result")
                .and_then(|result| result.get("active"))
                .and_then(serde_json::Value::as_bool)
                == Some(false)
        {
            self.live_observation_final_states_total =
                self.live_observation_final_states_total.saturating_add(1);
        }
    }

    fn record_images(&mut self, images: HostJsonlImageMetrics) {
        self.image_outputs_total = self.image_outputs_total.saturating_add(images.images_total);
        self.image_pixels_total = self.image_pixels_total.saturating_add(images.pixels_total);
        self.image_encoded_bytes_total = self
            .image_encoded_bytes_total
            .saturating_add(images.encoded_bytes_total);
        self.image_unknown_dimensions_total = self
            .image_unknown_dimensions_total
            .saturating_add(images.unknown_dimensions_total);
    }

    fn report(
        &self,
        run_status: HostJsonlRunStatus,
        elapsed: std::time::Duration,
    ) -> HostJsonlMetricsReport<'_> {
        HostJsonlMetricsReport {
            schema: "dcc-cua.host-jsonl.metrics.v3",
            run_status,
            transport_success: run_status.transport_success(),
            elapsed_ms: elapsed.as_millis().try_into().unwrap_or(u64::MAX),
            input_lines_total: self.input_lines_total,
            requests_total: self.requests_total,
            action_requests_total: self.action_requests_total,
            action_attempts_total: self.action_attempts_total,
            action_succeeded_total: self.action_succeeded_total,
            action_rejected_total: self.action_rejected_total,
            post_action_snapshot_requests_total: self.post_action_snapshot_requests_total,
            visual_observation_requests_total: self.visual_observation_requests_total,
            semantic_observation_requests_total: self.semantic_observation_requests_total,
            post_action_observation_requests_total: self.post_action_observation_requests_total,
            standalone_snapshot_requests_total: self.standalone_snapshot_requests_total,
            errors_total: self.errors_total,
            json_input_bytes: self.json_input_bytes,
            json_output_bytes: self.json_output_bytes,
            image_outputs_total: self.image_outputs_total,
            image_pixels_total: self.image_pixels_total,
            image_encoded_bytes_total: self.image_encoded_bytes_total,
            image_unknown_dimensions_total: self.image_unknown_dimensions_total,
            live_observation_start_requests_total: self.live_observation_start_requests_total,
            live_observation_stop_requests_total: self.live_observation_stop_requests_total,
            live_observation_final_states_total: self.live_observation_final_states_total,
            methods: &self.methods,
            action_kinds: &self.action_kinds,
            error_codes: &self.error_codes,
        }
    }

    fn checkpoint(&self) -> Result<(), Box<dyn std::error::Error>> {
        let Some(checkpoint) = &self.checkpoint else {
            return Ok(());
        };
        write_host_jsonl_metrics(
            Some(&checkpoint.path),
            &self.report(HostJsonlRunStatus::Running, checkpoint.started.elapsed()),
        )
    }

    fn finish(&self, succeeded: bool) -> Result<(), Box<dyn std::error::Error>> {
        let Some(checkpoint) = &self.checkpoint else {
            return Ok(());
        };
        let status = if succeeded {
            HostJsonlRunStatus::Succeeded
        } else {
            HostJsonlRunStatus::Failed
        };
        write_host_jsonl_metrics(
            Some(&checkpoint.path),
            &self.report(status, checkpoint.started.elapsed()),
        )
    }
}

fn is_action_request(method: &str) -> bool {
    matches!(
        method,
        "execute_action"
            | "execute_desktop_action"
            | "browser_click"
            | "browser_type"
            | "browser_pointer"
            | "browser_navigate"
            | "browser_set_input_files"
            | "browser_dialog"
    )
}

fn is_rejected_action_response(value: &serde_json::Value) -> bool {
    value.get("type").and_then(serde_json::Value::as_str) == Some("error")
        || value.get("success").and_then(serde_json::Value::as_bool) == Some(false)
}

fn is_standalone_snapshot_request(method: &str) -> bool {
    matches!(
        method,
        "snapshot" | "desktop_snapshot" | "desktop_session_snapshot" | "browser_snapshot"
    )
}

fn is_visual_observation_request(request: &JsonlRequest) -> bool {
    is_standalone_snapshot_request(&request.method)
        || request.method == "zoom"
        || (request.method == "verify_state"
            && request
                .params
                .get("include_screenshot")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false))
}

fn is_semantic_observation_request(method: &str) -> bool {
    matches!(
        method,
        "accessibility_snapshot"
            | "find"
            | "get_session_state"
            | "get_input_state"
            | "session_health"
            | "poll_session_events"
            | "get_window_state"
            | "verify_state"
            | "wait_for"
    )
}

fn write_host_jsonl_metrics(
    path: Option<&std::path::Path>,
    report: &HostJsonlMetricsReport<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = path else {
        return Ok(());
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let value = json!({
        "schema": report.schema,
        "run_status": report.run_status.as_str(),
        "transport_success": report.transport_success,
        "elapsed_ms": report.elapsed_ms,
        "input_lines_total": report.input_lines_total,
        "requests_total": report.requests_total,
        "action_requests_total": report.action_requests_total,
        "action_attempts_total": report.action_attempts_total,
        "action_succeeded_total": report.action_succeeded_total,
        "action_rejected_total": report.action_rejected_total,
        "post_action_snapshot_requests_total": report.post_action_snapshot_requests_total,
        "visual_observation_requests_total": report.visual_observation_requests_total,
        "semantic_observation_requests_total": report.semantic_observation_requests_total,
        "post_action_observation_requests_total": report.post_action_observation_requests_total,
        "standalone_snapshot_requests_total": report.standalone_snapshot_requests_total,
        "errors_total": report.errors_total,
        "json_input_bytes": report.json_input_bytes,
        "json_output_bytes": report.json_output_bytes,
        "image_outputs_total": report.image_outputs_total,
        "image_pixels_total": report.image_pixels_total,
        "image_encoded_bytes_total": report.image_encoded_bytes_total,
        "image_unknown_dimensions_total": report.image_unknown_dimensions_total,
        "live_observation_start_requests_total": report.live_observation_start_requests_total,
        "live_observation_stop_requests_total": report.live_observation_stop_requests_total,
        "live_observation_final_states_total": report.live_observation_final_states_total,
        "methods": report.methods,
        "action_kinds": report.action_kinds,
        "error_codes": report.error_codes,
    });
    write_json_atomically(path, &serde_json::to_vec_pretty(&value)?)?;
    Ok(())
}

fn write_json_atomically(path: &std::path::Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("metrics output requires a file name"))?
        .to_string_lossy();
    let temporary = path.with_file_name(format!(".{file_name}.dcc-cua-{}.tmp", std::process::id()));
    fs::write(&temporary, bytes)?;
    replace_file(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

#[cfg(unix)]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: Both paths are valid, NUL-terminated UTF-16 buffers that remain alive for the call.
    let succeeded = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if succeeded == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn parse_jsonl_request(line: &str) -> Result<JsonlRequest, String> {
    let line = line.strip_prefix('\u{feff}').unwrap_or(line);
    let value: serde_json::Value = serde_json::from_str(line)
        .map_err(|error| format!("JSONL request is invalid JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "JSONL request must be an object".to_owned())?;
    let request_id = match object.get("request_id") {
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| "JSONL request_id must be a string".to_owned())?
                .to_owned(),
        ),
        None => None,
    };
    if request_id
        .as_deref()
        .is_some_and(|id| id.is_empty() || id.chars().count() > MAX_REQUEST_ID_CHARS)
    {
        return Err(format!(
            "JSONL request_id must contain 1..{MAX_REQUEST_ID_CHARS} characters"
        ));
    }
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
        request_id,
        method: method.to_owned(),
        params,
    })
}

fn measured_jsonl_response_value(
    response: HostResponse,
    output_dir: Option<&str>,
    index: usize,
    metrics: &mut HostJsonlMetrics,
    request_method: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    metrics.record_response(request_method, &response.value);
    let output = jsonl_response_value_with_metrics(response, output_dir, index)?;
    metrics.record_images(output.image_metrics);
    Ok(output.value)
}

fn jsonl_response_value_with_metrics(
    response: HostResponse,
    output_dir: Option<&str>,
    index: usize,
) -> Result<JsonlResponseOutput, Box<dyn std::error::Error>> {
    let mut value = response.value;
    let shared_image = value
        .get("image")
        .is_some_and(|image| image["encoding"] == "shared_memory");
    let bytes = match response.binary_attachment {
        Some(bytes) => Some(bytes),
        None if shared_image => {
            let descriptor = serde_json::from_value(value["image"].clone())?;
            Some(SharedImageReader::open(descriptor)?.read()?)
        }
        None => None,
    };
    let image_metrics = response_image_metrics(&value, bytes.as_deref());
    if let Some(bytes) = bytes {
        let directory = output_dir.ok_or(
            "JSONL response contains image bytes; pass --output-dir or negotiate shared_memory",
        )?;
        let path = format!("{directory}/response-{index}.bin");
        fs::write(&path, bytes)?;
        value["_dcc_cua_binary_output"] = json!(path);
    }
    Ok(JsonlResponseOutput {
        value,
        image_metrics,
    })
}

fn response_image_metrics(
    value: &serde_json::Value,
    bytes: Option<&[u8]>,
) -> HostJsonlImageMetrics {
    let Some(bytes) = bytes else {
        return HostJsonlImageMetrics::default();
    };
    let mut metrics = HostJsonlImageMetrics::default();
    let attachments = value
        .get("attachments")
        .and_then(serde_json::Value::as_array);
    if let Some(attachments) = attachments.filter(|attachments| !attachments.is_empty()) {
        for attachment in attachments {
            let Some(offset) = attachment
                .get("offset")
                .and_then(serde_json::Value::as_u64)
                .and_then(|offset| usize::try_from(offset).ok())
            else {
                continue;
            };
            let Some(length) = attachment
                .get("length")
                .and_then(serde_json::Value::as_u64)
                .and_then(|length| usize::try_from(length).ok())
            else {
                continue;
            };
            let Some(end) = offset.checked_add(length) else {
                continue;
            };
            let Some(image) = bytes.get(offset..end) else {
                continue;
            };
            record_encoded_image(&mut metrics, image);
        }
    } else if value.get("image").is_some() {
        let length = value["image"]
            .get("length")
            .and_then(serde_json::Value::as_u64)
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(bytes.len());
        if let Some(image) = bytes.get(..length) {
            record_encoded_image(&mut metrics, image);
        }
    }
    metrics
}

fn record_encoded_image(metrics: &mut HostJsonlImageMetrics, image: &[u8]) {
    metrics.images_total = metrics.images_total.saturating_add(1);
    metrics.encoded_bytes_total = metrics
        .encoded_bytes_total
        .saturating_add(image.len().try_into().unwrap_or(u64::MAX));
    if let Some(pixels) = png_pixel_count(image) {
        metrics.pixels_total = metrics.pixels_total.saturating_add(pixels);
    } else {
        metrics.unknown_dimensions_total = metrics.unknown_dimensions_total.saturating_add(1);
    }
}

fn png_pixel_count(image: &[u8]) -> Option<u64> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if image.len() < 24 || image.get(..8)? != PNG_SIGNATURE || image.get(12..16)? != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(image.get(16..20)?.try_into().ok()?);
    let height = u32::from_be_bytes(image.get(20..24)?.try_into().ok()?);
    u64::from(width).checked_mul(u64::from(height))
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
    metrics: &mut HostJsonlMetrics,
) -> Result<(), std::io::Error> {
    let body =
        serde_json::to_vec(&value).map_err(|error| std::io::Error::other(error.to_string()))?;
    output.write_all(&body).await?;
    output.write_all(b"\n").await?;
    output.flush().await?;
    metrics.record_output(&value, body.len());
    metrics
        .checkpoint()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(())
}

fn parse_host_batch(
    value: serde_json::Value,
) -> Result<Vec<HostBatchRequest>, Box<dyn std::error::Error>> {
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
            let request_id = match object.get("request_id") {
                Some(value) => Some(
                    value
                        .as_str()
                        .ok_or_else(|| {
                            format!("host-batch request {index} request_id must be a string")
                        })?
                        .to_owned(),
                ),
                None => None,
            };
            if request_id.as_deref().is_some_and(|id| {
                id.is_empty() || id.chars().count() > MAX_REQUEST_ID_CHARS
            }) {
                return Err(format!(
                    "host-batch request {index} request_id must contain 1..{MAX_REQUEST_ID_CHARS} characters"
                )
                .into());
            }
            let method = object
                .get("method")
                .and_then(serde_json::Value::as_str)
                .filter(|method| !method.is_empty())
                .ok_or_else(|| format!("host-batch request {index} requires method"))?;
            let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
            if !params.is_object() {
                return Err(format!("host-batch request {index} params must be an object").into());
            }
            Ok(HostBatchRequest {
                request_id,
                method: method.to_owned(),
                params,
            })
        })
        .collect()
}

struct HostBatchRequest {
    request_id: Option<String>,
    method: String,
    params: serde_json::Value,
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
    Spawned(Box<HostProcess>),
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
            let status = (*process).shutdown().await?;
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
    let agent_name = flag_value(flags, "--agent-name").unwrap_or_else(|| "dcc-cua-cli".into());
    if let Some(binary_path) = flag_value(flags, "--spawn") {
        return Ok(HostConnection::Spawned(Box::new(
            HostProcess::spawn(binary_path, agent_name, snapshot_transport).await?,
        )));
    }
    Ok(match flag_value(flags, "--endpoint") {
        Some(endpoint) => HostConnection::Endpoint(
            HostClient::connect_with_transport(endpoint, agent_name, snapshot_transport).await?,
        ),
        None => HostConnection::Endpoint(
            HostClient::connect_default_with_transport(agent_name, snapshot_transport).await?,
        ),
    })
}

fn showcase_directory(flags: &[String]) -> Result<Option<String>, Box<dyn std::error::Error>> {
    if !has_flag(flags, "--showcase") {
        return Ok(None);
    }
    let directory = flag_value(flags, "--showcase-dir")
        .map(std::path::PathBuf::from)
        .unwrap_or(env::current_dir()?.join("artifacts").join("showcase"));
    fs::create_dir_all(&directory)?;
    Ok(Some(
        fs::canonicalize(directory)?.to_string_lossy().into_owned(),
    ))
}

fn enable_showcase(
    request: &mut JsonlRequest,
    directory: &str,
    request_index: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    if request.method != "open_session" {
        return Ok(());
    }
    let grant = request
        .params
        .get_mut("grant")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or("showcase open_session requires params.grant")?;
    let session_directory = reserve_showcase_session_directory(directory, request_index)?;
    grant.insert("allow_recording".into(), json!(true));
    grant.insert("showcase_output_dir".into(), json!(session_directory));
    Ok(())
}

fn reserve_showcase_session_directory(
    directory: &str,
    request_index: usize,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let root = std::path::Path::new(directory);
    fs::create_dir_all(root)?;
    for attempt in 0_u32.. {
        let name = if attempt == 0 {
            format!("session-{request_index}")
        } else {
            format!("session-{request_index}-{attempt}")
        };
        let candidate = root.join(name);
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(fs::canonicalize(candidate)?),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    unreachable!("the showcase session suffix space is unbounded for this process")
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
    let app = application_label(flags);
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-cli".into());
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
    let app = application_label(flags);
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-cli".into());
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
    let app = application_label(flags);
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-cli".into());
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
    let app = application_label(flags);
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-cli".into());
    let output = flag_value(flags, "--output").unwrap_or_else(|| "screenshot.png".into());
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let result = async {
        maybe_escalate(&mut session, flags).await?;
        let activation = if has_flag(flags, "--activate") {
            Some(session.activate().await?)
        } else {
            None
        };
        Ok::<_, dcc_cua_core::ComputerUseError>((activation, session.screenshot().await?))
    }
    .await;
    let stop_result = session.stop().await;
    let (activation, screenshot) = result?;
    stop_result?;
    let node_count = screenshot.accessibility["elements"]
        .as_array()
        .map_or(0, Vec::len);
    let accessibility = screenshot.accessibility;
    fs::write(&output, &screenshot.data)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "success": true,
            "observation": screenshot.observation,
            "accessibility": accessibility,
            "node_count": node_count,
            "output": output,
            "activation": activation,
            "backend": "cua-driver-sdk",
        }))?
    );
    Ok(())
}

async fn accessibility_snapshot(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let scope = select_scope(driver, flags).await?;
    let app = application_label(flags);
    let session_id =
        flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-accessibility-cli".into());
    let max_elements = bounded_u32(flags, "--max-elements", 5_000, 5_000)?;
    let max_depth = bounded_u32(flags, "--max-depth", 64, 64)?;
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let result = session
        .accessibility_snapshot(max_elements, max_depth)
        .await;
    let stop_result = session.stop().await;
    let root = result?;
    stop_result?;
    let node_count = root["elements"].as_array().map_or(0, Vec::len);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "accessibility": root,
            "node_count": node_count,
            "max_elements": max_elements,
            "max_depth": max_depth,
        }))?
    );
    Ok(())
}

async fn window_state(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let scope = select_scope(driver, flags).await?;
    let app = application_label(flags);
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-window-cli".into());
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let result = session.window_state().await;
    let stop_result = session.stop().await;
    let state = result?;
    stop_result?;
    println!("{}", serde_json::to_string_pretty(&state)?);
    Ok(())
}

async fn doctor(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let report = driver.diagnostics().await;
    println!("{}", serde_json::to_string_pretty(&report)?);
    let route = doctor_route(flags)?;
    if !diagnostic_route_ready(&report, route) {
        return Err(format!("CUA {route} diagnostics are not ready").into());
    }
    Ok(())
}

fn doctor_route(flags: &[String]) -> Result<&str, Box<dyn std::error::Error>> {
    let route = flag_value(flags, "--route").unwrap_or_else(|| "full".into());
    match route.as_str() {
        "full" => Ok("full"),
        "visual" => Ok("visual"),
        "semantic" => Ok("semantic"),
        _ => Err("--route must be full, visual, or semantic".into()),
    }
}

fn diagnostic_route_ready(report: &serde_json::Value, route: &str) -> bool {
    if route == "full" {
        report["ready"] == true
    } else {
        report["routes"][route]["ready"] == true
    }
}

async fn maybe_escalate(
    session: &mut dcc_cua_core::ComputerUseSession,
    flags: &[String],
) -> dcc_cua_core::ComputerUseResult<Option<serde_json::Value>> {
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

#[cfg(test)]
mod tests;
