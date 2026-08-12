use super::*;

pub(super) async fn activate_window(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let scope = select_scope(driver, flags).await?;
    let app = application_label(flags);
    let session_id =
        flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-activate-cli".into());
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let result = session.activate().await;
    let stop_result = session.stop().await;
    let activation = result?;
    stop_result?;
    println!("{}", serde_json::to_string_pretty(&activation)?);
    Ok(())
}

pub(super) async fn set_window_frame(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let scope = select_scope(driver, flags).await?;
    let app = application_label(flags);
    let session_id =
        flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-window-frame-cli".into());
    let frame = window_frame_request(flags)?;
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let result = session.set_window_frame(&frame).await;
    let stop_result = session.stop().await;
    let result = result?;
    stop_result?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub(super) fn window_frame_request(
    flags: &[String],
) -> Result<ComputerUseWindowFrameRequest, Box<dyn std::error::Error>> {
    let required = |name: &str| -> Result<f64, Box<dyn std::error::Error>> {
        Ok(flag_value(flags, name)
            .ok_or_else(|| format!("set-window-frame requires {name}"))?
            .parse()?)
    };
    let request = ComputerUseWindowFrameRequest {
        x: required("--x")?,
        y: required("--y")?,
        width: required("--width")?,
        height: required("--height")?,
    };
    request.validate()?;
    Ok(request)
}

pub(super) async fn invoke_menu(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let scope = select_scope(driver, flags).await?;
    let app = application_label(flags);
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-menu-cli".into());
    let request = menu_request(flags)?;
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let result = session.invoke_menu(&request).await;
    let stop_result = session.stop().await;
    let result = result?;
    stop_result?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub(super) fn menu_request(
    flags: &[String],
) -> Result<ComputerUseMenuRequest, Box<dyn std::error::Error>> {
    let request = ComputerUseMenuRequest {
        path: flag_values(flags, "--menu"),
    };
    request.validate()?;
    Ok(request)
}

pub(super) async fn zoom(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let scope = select_scope(driver, flags).await?;
    let app = application_label(flags);
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-zoom-cli".into());
    let coordinate = |name: &str| -> Result<f64, Box<dyn std::error::Error>> {
        flag_value(flags, name)
            .ok_or_else(|| format!("zoom requires {name}").into())
            .and_then(|value| Ok(value.parse()?))
    };
    let output = flag_value(flags, "--output");
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let result = async {
        maybe_escalate(&mut session, flags).await?;
        let screenshot = session.screenshot().await?;
        let zoom = session
            .zoom(&ComputerUseZoomRequest {
                observation_id: screenshot.observation.observation_id.clone(),
                x1: coordinate("--x1")?,
                y1: coordinate("--y1")?,
                x2: coordinate("--x2")?,
                y2: coordinate("--y2")?,
            })
            .await?;
        Ok::<_, Box<dyn std::error::Error>>((screenshot.observation, zoom))
    }
    .await;
    let stop_result = session.stop().await;
    let (observation, result) = result?;
    stop_result?;
    let mut value = result.value;
    if let Some(path) = output {
        let image = result.images.first().ok_or("zoom returned no image")?;
        fs::write(&path, &image.data)?;
        value["_dcc_cua_image_output"] = json!(path);
    }
    value["observation"] = serde_json::to_value(observation)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub(super) async fn act(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let action_json = flag_value(flags, "--action-json")
        .ok_or("act requires --action-json with a ComputerUseAction JSON object")?;
    execute_action(driver, flags, serde_json::from_str(&action_json)?).await
}

pub(super) async fn friendly_action(
    driver: &ComputerUseDriver,
    flags: &[String],
    command: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    execute_action(driver, flags, action_from_command(command, flags)?).await
}

async fn execute_action(
    driver: &ComputerUseDriver,
    flags: &[String],
    mut action: ComputerUseAction,
) -> Result<(), Box<dyn std::error::Error>> {
    default_activated_action_to_foreground(flags, &mut action);
    let scope = select_scope(driver, flags).await?;
    let app = application_label(flags);
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-cli".into());
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let semantic_action = action.element_index.is_some() || action.element_token.is_some();
    let visible_dimensions = visible_snapshot_dimensions(flags)?;
    let max_elements = bounded_u32(flags, "--max-elements", 5_000, 5_000)?;
    let max_depth = bounded_u32(flags, "--max-depth", 64, 64)?;
    let result = async {
        maybe_escalate(&mut session, flags).await?;
        let activation = if has_flag(flags, "--activate") {
            Some(session.activate().await?)
        } else {
            None
        };
        let (observation, action_result, post_snapshot) = if semantic_action {
            let accessibility = session
                .accessibility_snapshot(max_elements, max_depth)
                .await?;
            bind_fresh_element_token(&mut action, &accessibility);
            let observation = session.latest_observation().cloned().ok_or_else(|| {
                dcc_cua_core::ComputerUseError::new(
                    dcc_cua_core::ComputerUseErrorCode::CaptureFailed,
                    "semantic snapshot returned no observation metadata",
                )
            })?;
            action.observation_id = Some(observation.observation_id.clone());
            let action_result = session.perform_action(&action).await?;
            let post_snapshot = semantic_post_snapshot_value(
                session
                    .accessibility_snapshot(max_elements, max_depth)
                    .await,
                flag_value(flags, "--output"),
            );
            (observation, action_result, post_snapshot)
        } else {
            let screenshot = session.screenshot().await?;
            map_visible_snapshot_coordinates(
                &mut action,
                visible_dimensions,
                &screenshot.observation,
            )
            .map_err(|message| {
                ComputerUseError::new(ComputerUseErrorCode::InvalidAction, message)
            })?;
            action.observation_id = Some(screenshot.observation.observation_id.clone());
            let action_result = session.perform_action(&action).await?;
            let post_snapshot = window_post_snapshot_value(
                session.screenshot().await,
                flag_value(flags, "--output"),
            );
            (screenshot.observation, action_result, post_snapshot)
        };
        Ok::<_, dcc_cua_core::ComputerUseError>((
            activation,
            observation,
            action_result,
            post_snapshot,
        ))
    }
    .await;
    let stop_result = session.stop().await;
    let (activation, observation, action_result, post_snapshot) = result?;
    stop_result?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "success": true,
            "activation": activation,
            "observation": observation,
            "action": action_result_value(action_result),
            "post_snapshot": post_snapshot,
        }))?
    );
    Ok(())
}

pub(super) fn visible_snapshot_dimensions(
    flags: &[String],
) -> Result<Option<(u32, u32)>, Box<dyn std::error::Error>> {
    let width = flag_value(flags, "--observation-width");
    let height = flag_value(flags, "--observation-height");
    match (width, height) {
        (None, None) => Ok(None),
        (Some(width), Some(height)) => {
            let width = width.parse::<u32>()?;
            let height = height.parse::<u32>()?;
            if width == 0 || height == 0 {
                return Err("observation dimensions must be greater than zero".into());
            }
            Ok(Some((width, height)))
        }
        _ => Err("--observation-width and --observation-height must be provided together".into()),
    }
}

pub(super) fn map_visible_snapshot_coordinates(
    action: &mut ComputerUseAction,
    visible_dimensions: Option<(u32, u32)>,
    fresh_observation: &ComputerUseObservation,
) -> Result<(), String> {
    let Some((visible_width, visible_height)) = visible_dimensions else {
        return Ok(());
    };
    let map = |x: f64, y: f64| -> Result<(f64, f64), String> {
        if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
            return Err("visible snapshot coordinates must be finite and non-negative".into());
        }
        if x >= f64::from(visible_width) || y >= f64::from(visible_height) {
            return Err("coordinates exceed the declared visible snapshot dimensions".into());
        }
        Ok((
            x * f64::from(fresh_observation.width) / f64::from(visible_width),
            y * f64::from(fresh_observation.height) / f64::from(visible_height),
        ))
    };
    if let (Some(x), Some(y)) = (action.x, action.y) {
        (action.x, action.y) = map(x, y).map(|(x, y)| (Some(x), Some(y)))?;
    }
    for point in &mut action.path {
        (point.x, point.y) = map(point.x, point.y)?;
    }
    Ok(())
}

pub(super) fn default_activated_action_to_foreground(
    flags: &[String],
    action: &mut ComputerUseAction,
) {
    if has_flag(flags, "--activate") && action.delivery_mode.is_none() {
        action.delivery_mode = Some("foreground".into());
    }
}

pub(super) fn bind_fresh_element_token(
    action: &mut ComputerUseAction,
    accessibility: &serde_json::Value,
) {
    let Some(index) = action.element_index else {
        return;
    };
    let Some(token) = accessibility["elements"]
        .as_array()
        .and_then(|elements| {
            elements
                .iter()
                .find(|element| element["element_index"].as_u64() == Some(u64::from(index)))
        })
        .and_then(|element| element["element_token"].as_str())
    else {
        return;
    };
    action.element_token = Some(token.into());
    action.element_index = None;
}

pub(super) fn is_friendly_action(command: &str) -> bool {
    matches!(
        command,
        "click"
            | "double-click"
            | "right-click"
            | "toggle"
            | "drag"
            | "type"
            | "set-text"
            | "set-value"
            | "press"
            | "hotkey"
            | "scroll"
            | "move"
    )
}

pub(super) fn action_from_command(
    command: &str,
    flags: &[String],
) -> Result<ComputerUseAction, Box<dyn std::error::Error>> {
    let coordinate = |name: &str| -> Result<f64, Box<dyn std::error::Error>> {
        flag_value(flags, name)
            .ok_or_else(|| format!("{command} requires {name}").into())
            .and_then(|value| Ok(value.parse()?))
    };
    let mut action = ComputerUseAction {
        action: match command {
            "double-click" => "double_click",
            "right-click" => "right_click",
            "type" => "type",
            "set-text" => "set_text",
            "set-value" => "set_value",
            "press" => "keypress",
            "hotkey" => "keyboard_shortcut",
            other => other,
        }
        .into(),
        ..ComputerUseAction::default()
    };
    action.delivery_mode = parse_delivery_mode(flags)?;
    match command {
        "click" | "double-click" | "right-click" | "toggle" => {
            apply_element_selector(&mut action, flags, command)?;
            action.button = flag_value(flags, "--button");
            if command == "click" {
                action.duration_ms = flag_value(flags, "--duration-ms")
                    .map(|value| value.parse::<u64>())
                    .transpose()?;
            }
            let coordinate_present =
                flag_value(flags, "--x").is_some() || flag_value(flags, "--y").is_some();
            if (action.element_index.is_some() || action.element_token.is_some())
                && coordinate_present
            {
                return Err(format!(
                    "{command} cannot combine coordinates with an element selector"
                )
                .into());
            }
            if action.element_index.is_none() && action.element_token.is_none() {
                action.x = Some(coordinate("--x")?);
                action.y = Some(coordinate("--y")?);
            }
        }
        "move" => {
            action.x = Some(coordinate("--x")?);
            action.y = Some(coordinate("--y")?);
        }
        "drag" => {
            action.path = vec![
                dcc_cua_core::ComputerUsePoint {
                    x: coordinate("--from-x")?,
                    y: coordinate("--from-y")?,
                },
                dcc_cua_core::ComputerUsePoint {
                    x: coordinate("--to-x")?,
                    y: coordinate("--to-y")?,
                },
            ];
            action.button = flag_value(flags, "--button");
            action.modifiers = flag_values(flags, "--modifier");
            action.duration_ms = flag_value(flags, "--duration-ms")
                .map(|value| value.parse::<u64>())
                .transpose()?;
            action.steps = flag_value(flags, "--steps")
                .map(|value| value.parse::<u32>())
                .transpose()?;
        }
        "type" => {
            action.text = Some(flag_value(flags, "--text").ok_or("type requires --text")?);
            apply_element_selector(&mut action, flags, command)?;
            action.type_chars_only = has_flag(flags, "--focused");
            if action.type_chars_only {
                action.action = "type_chars".into();
            }
            if action.type_chars_only
                && (action.element_index.is_some() || action.element_token.is_some())
            {
                return Err("type cannot combine --focused with an element selector".into());
            }
            let coordinates = optional_coordinate_pair(flags, command)?;
            if let Some((x, y)) = coordinates {
                if action.type_chars_only
                    || action.element_index.is_some()
                    || action.element_token.is_some()
                {
                    return Err(
                        "type coordinates cannot combine with --focused or an element selector"
                            .into(),
                    );
                }
                action.action = "type".into();
                action.x = Some(x);
                action.y = Some(y);
            }
            action.delay_ms = flag_value(flags, "--delay-ms")
                .map(|value| value.parse::<u64>())
                .transpose()?;
            if coordinates.is_some() && action.delay_ms.is_some() {
                return Err("coordinate type does not support --delay-ms".into());
            }
            if !action.type_chars_only
                && coordinates.is_none()
                && action.element_index.is_none()
                && action.element_token.is_none()
            {
                return Err(
                    "type requires --focused, --x/--y, or --element-index/--element-token".into(),
                );
            }
        }
        "set-text" | "set-value" => {
            let flag = value_flag(command);
            action.text =
                Some(flag_value(flags, flag).ok_or_else(|| format!("{command} requires {flag}"))?);
            apply_element_selector(&mut action, flags, command)?;
            if action.element_index.is_none() && action.element_token.is_none() {
                return Err(
                    format!("{command} requires --element-index or --element-token").into(),
                );
            }
        }
        "press" => {
            action.keys = vec![flag_value(flags, "--key").ok_or("press requires --key")?];
            apply_element_selector(&mut action, flags, command)?;
            let coordinates = optional_coordinate_pair(flags, command)?;
            if coordinates.is_some()
                && (action.element_index.is_some() || action.element_token.is_some())
            {
                return Err("press cannot combine coordinates with an element selector".into());
            }
            if let Some((x, y)) = coordinates {
                action.x = Some(x);
                action.y = Some(y);
            }
            action.modifiers = flag_values(flags, "--modifier");
        }
        "hotkey" => {
            action.keys = flag_values(flags, "--key");
            if action.keys.len() < 2 {
                return Err("hotkey requires at least two repeated --key values".into());
            }
            apply_element_selector(&mut action, flags, command)?;
            let coordinates = optional_coordinate_pair(flags, command)?;
            if coordinates.is_some()
                && (action.element_index.is_some() || action.element_token.is_some())
            {
                return Err("hotkey cannot combine coordinates with an element selector".into());
            }
            if let Some((x, y)) = coordinates {
                action.x = Some(x);
                action.y = Some(y);
            }
        }
        "scroll" => {
            apply_element_selector(&mut action, flags, command)?;
            let coordinates = optional_coordinate_pair(flags, command)?;
            if coordinates.is_some()
                && (action.element_index.is_some() || action.element_token.is_some())
            {
                return Err("scroll cannot combine coordinates with an element selector".into());
            }
            if let Some((x, y)) = coordinates {
                action.x = Some(x);
                action.y = Some(y);
            }
            action.scroll_x = flag_value(flags, "--scroll-x")
                .map(|value| value.parse::<i32>())
                .transpose()?;
            action.scroll_y = flag_value(flags, "--scroll-y")
                .map(|value| value.parse::<i32>())
                .transpose()?;
            action.scroll_by = flag_value(flags, "--by");
            if action.scroll_x.is_none()
                && action.scroll_y.is_none()
                && action.element_index.is_none()
                && action.element_token.is_none()
                && action.x.is_none()
            {
                return Err(
                    "scroll requires --scroll-x/--scroll-y, coordinates, or an element selector"
                        .into(),
                );
            }
        }
        _ => unreachable!("friendly action command is validated before parsing"),
    }
    Ok(action)
}

fn parse_delivery_mode(flags: &[String]) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(mode) = flag_value(flags, "--delivery-mode") else {
        return Ok(None);
    };
    if !matches!(mode.as_str(), "background" | "foreground") {
        return Err("--delivery-mode must be background or foreground".into());
    }
    Ok(Some(mode))
}

fn value_flag(command: &str) -> &'static str {
    if command == "set-value" {
        "--value"
    } else {
        "--text"
    }
}

fn apply_element_selector(
    action: &mut ComputerUseAction,
    flags: &[String],
    command: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let element_index = flag_value(flags, "--element-index")
        .map(|value| value.parse::<u32>())
        .transpose()?;
    let element_token = flag_value(flags, "--element-token");
    if element_index.is_some() && element_token.is_some() {
        return Err(format!("{command} accepts only one element selector").into());
    }
    action.element_index = element_index;
    action.element_token = element_token;
    Ok(())
}

fn optional_coordinate_pair(
    flags: &[String],
    command: &str,
) -> Result<Option<(f64, f64)>, Box<dyn std::error::Error>> {
    let x = flag_value(flags, "--x");
    let y = flag_value(flags, "--y");
    match (x, y) {
        (None, None) => Ok(None),
        (Some(x), Some(y)) => Ok(Some((x.parse()?, y.parse()?))),
        _ => Err(format!("{command} requires both --x and --y").into()),
    }
}

pub(super) async fn verify_state(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let scope = select_scope(driver, flags).await?;
    let app = application_label(flags);
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-verify-cli".into());
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

pub(super) async fn desktop_act(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let action_json = flag_value(flags, "--action-json")
        .ok_or("desktop-act requires --action-json with a raw coordinate action")?;
    let mut action: ComputerUseAction = serde_json::from_str(&action_json)?;
    let session_id = flag_value(flags, "--session").unwrap_or_else(|| "dcc-cua-desktop-cli".into());
    let mut session = driver.desktop_session(session_id.clone())?;
    session.start().await?;
    let result = async {
        let snapshot = session.screenshot().await?;
        action.observation_id = Some(snapshot.observation_id.clone());
        let action_result = session.perform_action(&action).await?;
        let post_snapshot = session.screenshot().await;
        Ok::<_, dcc_cua_core::ComputerUseError>((snapshot, action_result, post_snapshot))
    }
    .await;
    let stop_result = session.stop().await;
    let (snapshot, action_result, post_snapshot) = result?;
    stop_result?;
    let post_snapshot = desktop_post_snapshot_value(post_snapshot, flag_value(flags, "--output"));
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "success": true,
            "session_id": session_id,
            "observation_id": snapshot.observation_id,
            "state": snapshot.state,
            "action": action_result_value(action_result),
            "post_snapshot": post_snapshot,
        }))?
    );
    Ok(())
}

pub(super) fn action_result_value(result: ComputerUseToolResult) -> serde_json::Value {
    let mut value = result.value;
    value["text"] = json!(result.text);
    value["degraded"] = json!(result.degraded);
    value["image_count"] = json!(result.images.len());
    value
}

pub(super) fn window_post_snapshot_value(
    result: dcc_cua_core::ComputerUseResult<dcc_cua_core::ComputerUseScreenshot>,
    output: Option<String>,
) -> serde_json::Value {
    match result {
        Ok(snapshot) => {
            let node_count = snapshot.accessibility["elements"]
                .as_array()
                .map_or(0, Vec::len);
            let (output, output_error) = snapshot_output(&snapshot.data, output);
            json!({
                "success": true,
                "observation": snapshot.observation,
                "accessibility": snapshot.accessibility,
                "node_count": node_count,
                "output": output,
                "output_error": output_error,
            })
        }
        Err(error) => json!({
            "success": false,
            "action_was_executed": true,
            "code": error.code,
            "message": error.message,
        }),
    }
}

pub(super) fn semantic_post_snapshot_value(
    result: dcc_cua_core::ComputerUseResult<serde_json::Value>,
    output: Option<String>,
) -> serde_json::Value {
    match result {
        Ok(accessibility) => json!({
            "success": true,
            "observation_kind": "accessibility",
            "accessibility": accessibility,
            "node_count": accessibility["elements"].as_array().map_or(0, Vec::len),
            "output": serde_json::Value::Null,
            "output_error": output.map(|_| "semantic post-snapshot has no pixel output"),
        }),
        Err(error) => json!({
            "success": false,
            "action_was_executed": true,
            "code": error.code,
            "message": error.message,
        }),
    }
}

fn desktop_post_snapshot_value(
    result: dcc_cua_core::ComputerUseResult<dcc_cua_core::ComputerUseDesktopSnapshot>,
    output: Option<String>,
) -> serde_json::Value {
    match result {
        Ok(snapshot) => {
            let (output, output_error) = snapshot_output(&snapshot.data, output);
            json!({
                "success": true,
                "observation_id": snapshot.observation_id,
                "state": snapshot.state,
                "output": output,
                "output_error": output_error,
            })
        }
        Err(error) => json!({
            "success": false,
            "action_was_executed": true,
            "code": error.code,
            "message": error.message,
        }),
    }
}

fn snapshot_output(data: &[u8], output: Option<String>) -> (Option<String>, Option<String>) {
    let Some(path) = output else {
        return (None, None);
    };
    match fs::write(&path, data) {
        Ok(()) => (Some(path), None),
        Err(error) => (None, Some(format!("write post-action snapshot: {error}"))),
    }
}
