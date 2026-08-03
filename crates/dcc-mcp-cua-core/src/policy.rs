// Trust-boundary validation, action translation, and SDK result normalization.
use std::io::Cursor;

use base64::Engine;
use serde_json::{Value, json};

use crate::contracts::*;
use crate::window_target::WindowTarget;

pub(crate) fn bounded_snapshot_elements(value: u32) -> u32 {
    if value == 0 {
        DEFAULT_SNAPSHOT_MAX_ELEMENTS
    } else {
        value.min(MAX_SNAPSHOT_ELEMENTS)
    }
}

pub(crate) fn bounded_snapshot_depth(value: u32) -> u32 {
    if value == 0 {
        DEFAULT_SNAPSHOT_MAX_DEPTH
    } else {
        value.min(MAX_SNAPSHOT_DEPTH)
    }
}

pub(crate) fn validate_clipboard_write_request(
    request: &ComputerUseClipboardWriteRequest,
) -> ComputerUseResult<()> {
    let values = [
        request.text.is_some(),
        request.image_path.is_some(),
        request.file_path.is_some(),
    ];
    if values.into_iter().filter(|present| *present).count() != 1 {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "clipboard_write requires exactly one of text, image_path, or file_path",
        ));
    }
    if request
        .text
        .as_deref()
        .is_some_and(|text| text.encode_utf16().count() > MAX_TEXT_UTF16_UNITS)
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "clipboard text exceeds the host UTF-16 limit",
        ));
    }
    for path in [request.image_path.as_deref(), request.file_path.as_deref()]
        .into_iter()
        .flatten()
    {
        validate_local_file_path(path)?;
    }
    Ok(())
}

fn validate_local_file_path(path: &str) -> ComputerUseResult<()> {
    let candidate = std::path::Path::new(path);
    if path.is_empty()
        || path.chars().count() > MAX_LOCAL_PATH_CHARS
        || path.contains('\0')
        || !candidate.is_absolute()
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "clipboard file paths must be absolute and bounded",
        ));
    }
    let metadata = std::fs::symlink_metadata(candidate).map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            format!("clipboard file path is unavailable: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "clipboard file paths must name regular files directly",
        ));
    }
    Ok(())
}

pub(crate) fn validate_recording_start_request(
    request: &ComputerUseRecordingStartRequest,
) -> ComputerUseResult<()> {
    let path = request.output_dir.trim();
    if path.is_empty()
        || path.chars().count() > MAX_LOCAL_PATH_CHARS
        || path.contains('\0')
        || (!std::path::Path::new(path).is_absolute() && !path.starts_with("~/"))
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "recording output_dir must be an absolute path or ~/ path",
        ));
    }
    Ok(())
}

pub(crate) fn validate_verify_state_request(
    expect: &Value,
    timeout_ms: Option<u64>,
    stable_samples: Option<u64>,
) -> ComputerUseResult<()> {
    let predicates = expect.as_array().ok_or_else(|| {
        ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "verify_state expect must be an array",
        )
    })?;
    if !(1..=8).contains(&predicates.len()) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "verify_state expect must contain 1..8 predicates",
        ));
    }
    if timeout_ms.is_some_and(|value| value > 10_000) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "verify_state timeout_ms must be at most 10000",
        ));
    }
    if stable_samples.is_some_and(|value| !(1..=5).contains(&value)) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "verify_state stable_samples must be 1..5",
        ));
    }
    Ok(())
}

pub(crate) fn validate_native_tool_request(name: &str, arguments: &Value) -> ComputerUseResult<()> {
    if name.is_empty()
        || name.chars().count() > MAX_NATIVE_TOOL_NAME_CHARS
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "native CUA tool name must be 1..128 ASCII letters, digits, or underscores",
        ));
    }
    if !arguments.is_object() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "native CUA tool arguments must be a JSON object",
        ));
    }
    let encoded = serde_json::to_vec(arguments).map_err(|error| {
        ComputerUseError::new(ComputerUseErrorCode::InvalidAction, error.to_string())
    })?;
    if encoded.len() > MAX_NATIVE_TOOL_ARGUMENT_BYTES {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "native CUA tool arguments exceed 1 MiB",
        ));
    }
    if arguments
        .as_object()
        .is_some_and(|object| object.keys().any(|key| key.starts_with('_')))
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "native CUA reserved arguments are host-owned",
        ));
    }
    Ok(())
}

pub(crate) fn validate_escalation_request(
    reason: &str,
    detail: Option<&str>,
) -> ComputerUseResult<()> {
    const REASONS: [&str; 5] = [
        "ax_tree_pixel_mismatch",
        "background_delivery_failed",
        "foreground_ineffective",
        "no_window_target",
        "other",
    ];
    if !REASONS.contains(&reason) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "unsupported CUA session escalation reason",
        ));
    }
    if detail.is_some_and(|value| value.chars().count() > 200) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "CUA session escalation detail exceeds 200 characters",
        ));
    }
    Ok(())
}

pub(crate) fn cursor_tool_allowed(name: &str) -> bool {
    matches!(
        name,
        "move_cursor"
            | "set_agent_cursor_enabled"
            | "set_agent_cursor_motion"
            | "get_agent_cursor_state"
            | "set_agent_cursor_theme"
    )
}

pub(crate) fn validate_window_cursor_move(
    arguments: &serde_json::Map<String, Value>,
) -> ComputerUseResult<()> {
    let x = arguments.get("x").and_then(Value::as_f64).ok_or_else(|| {
        ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "move_cursor requires numeric x and y",
        )
    })?;
    let y = arguments.get("y").and_then(Value::as_f64).ok_or_else(|| {
        ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "move_cursor requires numeric x and y",
        )
    })?;
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "move_cursor coordinates must be finite and non-negative",
        ));
    }
    if arguments
        .get("scope")
        .is_some_and(|scope| scope.as_str() != Some("window"))
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "window cursor control only supports scope=window",
        ));
    }
    Ok(())
}

pub(crate) fn map_window_cursor_move(
    arguments: &mut serde_json::Map<String, Value>,
    target: &WindowTarget,
) -> ComputerUseResult<()> {
    validate_window_cursor_move(arguments)?;
    let x = arguments["x"].as_f64().expect("validated cursor x");
    let y = arguments["y"].as_f64().expect("validated cursor y");
    let [left, top, width, height] = target.bounds;
    if x >= f64::from(width) || y >= f64::from(height) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "move_cursor coordinates must stay inside the target window",
        ));
    }
    arguments.insert("x".into(), json!(f64::from(left) + x));
    arguments.insert("y".into(), json!(f64::from(top) + y));
    arguments.insert("scope".into(), Value::String("window".into()));
    Ok(())
}

pub(crate) fn native_tool_allowed_globally(name: &str) -> bool {
    matches!(
        name,
        "check_permissions"
            | "health_report"
            | "get_accessibility_tree"
            | "get_config"
            | "set_config"
            | "replay_trajectory"
            | "install_ffmpeg"
    )
}

pub(crate) fn native_tool_allowed_in_window_session(name: &str) -> bool {
    const DEDICATED_TOOLS: [&str; 34] = [
        "get_window_state",
        "set_window_frame",
        "invoke_menu",
        "zoom",
        "verify_state",
        "click",
        "double_click",
        "right_click",
        "drag",
        "type_text",
        "press_key",
        "hotkey",
        "set_value",
        "scroll",
        "clipboard_read",
        "clipboard_write",
        "get_desktop_state",
        "get_accessibility_tree",
        "move_cursor",
        "set_agent_cursor_enabled",
        "set_agent_cursor_motion",
        "get_agent_cursor_state",
        "set_agent_cursor_theme",
        "browser_prepare",
        "browser_navigate",
        "browser_click",
        "browser_type",
        "browser_dialog",
        "browser_set_input_files",
        "browser_download",
        "browser_pointer",
        "start_recording",
        "stop_recording",
        "get_recording_state",
    ];
    if DEDICATED_TOOLS.contains(&name) || name.starts_with("browser_") {
        return false;
    }
    !matches!(
        name,
        "launch_app"
            | "kill_app"
            | "bring_to_front"
            | "start_session"
            | "escalate_session"
            | "end_session"
    )
}

pub(crate) fn native_tool_result(
    result: cua_driver_sdk::ToolResult,
) -> ComputerUseResult<ComputerUseToolResult> {
    if result.images.len() > MAX_NATIVE_TOOL_IMAGES {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            format!("CUA native tool returned more than {MAX_NATIVE_TOOL_IMAGES} images"),
        ));
    }
    let mut images = Vec::with_capacity(result.images.len());
    let mut total_bytes = 0_usize;
    for image in result.images {
        if image.data_base64.len() > MAX_NATIVE_TOOL_IMAGE_BYTES * 2 {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "CUA native tool image exceeds 64 MiB",
            ));
        }
        let data = base64::engine::general_purpose::STANDARD
            .decode(image.data_base64)
            .map_err(|error| {
                ComputerUseError::new(ComputerUseErrorCode::CaptureFailed, error.to_string())
            })?;
        if data.is_empty() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "CUA native tool returned an empty image",
            ));
        }
        if data.len() > MAX_NATIVE_TOOL_IMAGE_BYTES {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "CUA native tool image exceeds 64 MiB",
            ));
        }
        total_bytes = total_bytes.checked_add(data.len()).ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "CUA native tool images exceed 64 MiB in total",
            )
        })?;
        if total_bytes > MAX_NATIVE_TOOL_TOTAL_IMAGE_BYTES {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "CUA native tool images exceed 64 MiB in total",
            ));
        }
        images.push(ComputerUseImage {
            data,
            mime_type: image.mime_type,
        });
    }
    let value = serde_json::from_str(&result.raw_json).map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            format!("CUA native tool returned invalid JSON: {error}"),
        )
    })?;
    Ok(ComputerUseToolResult {
        value,
        text: result.text,
        images,
        degraded: result.degraded,
    })
}

pub(crate) fn validate_action(action: &ComputerUseAction) -> ComputerUseResult<()> {
    const ACTIONS: [&str; 13] = [
        "click",
        "double_click",
        "right_click",
        "toggle",
        "move",
        "scroll",
        "drag",
        "type",
        "type_chars",
        "set_text",
        "set_value",
        "keypress",
        "keyboard_shortcut",
    ];
    if !ACTIONS.contains(&action.action.as_str()) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "unsupported Computer Use action",
        ));
    }
    if action.keys.len() > MAX_KEY_TOKENS
        || action.modifiers.len() > MAX_MODIFIER_TOKENS
        || action.path.len() > MAX_DRAG_POINTS
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "action exceeds the host safety limits",
        ));
    }
    if action
        .modifiers
        .iter()
        .any(|modifier| modifier.is_empty() || modifier.chars().count() > MAX_MODIFIER_CHARS)
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "modifiers must be non-empty and at most 32 characters",
        ));
    }
    if action
        .button
        .as_deref()
        .is_some_and(|button| !matches!(button, "left" | "middle" | "right"))
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "button must be left, middle, or right",
        ));
    }
    if action.x.is_some() != action.y.is_some() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "x and y must be supplied together",
        ));
    }
    if action
        .text
        .as_deref()
        .is_some_and(|text| text.encode_utf16().count() > MAX_TEXT_UTF16_UNITS)
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "text exceeds the host UTF-16 limit",
        ));
    }
    if action
        .delay_ms
        .is_some_and(|delay| delay > MAX_TYPE_CHAR_DELAY_MS)
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "delay_ms must be at most 200",
        ));
    }
    if action
        .element_token
        .as_deref()
        .is_some_and(|token| token.is_empty() || token.chars().count() > MAX_ELEMENT_TOKEN_CHARS)
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "element_token must be non-empty and at most 512 characters",
        ));
    }
    if action
        .delivery_mode
        .as_deref()
        .is_some_and(|mode| !matches!(mode, "background" | "foreground"))
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "delivery_mode must be background or foreground",
        ));
    }
    if action.action == "scroll" {
        validate_scroll(action)?;
    } else if action.scroll_x.is_some() || action.scroll_y.is_some() || action.scroll_by.is_some() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "scroll_x, scroll_y, and scroll_by are supported only for scroll",
        ));
    }
    for point in action.path.iter().chain(
        [action
            .x
            .zip(action.y)
            .map(|(x, y)| ComputerUsePoint { x, y })]
        .iter()
        .flatten(),
    ) {
        if !point.x.is_finite() || !point.y.is_finite() || point.x < 0.0 || point.y < 0.0 {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "coordinates must be finite and non-negative",
            ));
        }
    }
    if matches!(action.action.as_str(), "set_text" | "set_value")
        && action.element_index.is_none()
        && action.element_token.is_none()
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "set_text and set_value require a semantic element_index",
        ));
    }
    if action.action == "type_chars" {
        if action.text.is_none() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "type_chars requires text",
            ));
        }
        if action.type_chars_only
            && (action.element_index.is_some() || action.element_token.is_some())
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "type_chars_only cannot be combined with an element locator",
            ));
        }
        if !action.type_chars_only
            && action.element_index.is_none()
            && action.element_token.is_none()
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "type_chars requires an element locator unless type_chars_only is true",
            ));
        }
        if action.x.is_some() || action.y.is_some() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "type_chars does not accept screen coordinates",
            ));
        }
    }
    if action.action == "type" && action.text.is_none() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "type requires text",
        ));
    }
    if let Some(duration_ms) = action.duration_ms {
        if duration_ms > MAX_DRAG_DURATION_MS {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "duration_ms must be at most 10000",
            ));
        }
        if action.action != "drag" {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "duration_ms is supported only for drag",
            ));
        }
    }
    if let Some(steps) = action.steps {
        if !(1..=MAX_DRAG_STEPS).contains(&steps) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "steps must be between 1 and 200",
            ));
        }
        if action.action != "drag" {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "steps is supported only for drag",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_zoom_request(
    request: &ComputerUseZoomRequest,
    observation: &ComputerUseObservation,
) -> ComputerUseResult<()> {
    if request.observation_id != observation.observation_id {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "zoom observation_id does not match the latest screenshot",
        ));
    }
    if [request.x1, request.y1, request.x2, request.y2]
        .into_iter()
        .any(|coordinate| !coordinate.is_finite() || coordinate < 0.0)
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "zoom coordinates must be finite and non-negative",
        ));
    }
    if request.x2 <= request.x1 || request.y2 <= request.y1 {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "zoom region must have positive width and height",
        ));
    }
    if request.x2 - request.x1 > 500.0 {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "zoom region width must not exceed 500 pixels",
        ));
    }
    if request.x2 > f64::from(observation.width) || request.y2 > f64::from(observation.height) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "zoom region must stay within the latest screenshot",
        ));
    }
    Ok(())
}

pub(crate) fn action_arguments(
    action: &ComputerUseAction,
    session: &str,
    target: &WindowTarget,
) -> Value {
    let scope = json!({
        "_tool": match action.action.as_str() {
            "click" | "double_click" | "right_click" | "toggle" => "click",
            "drag" => "drag",
            "scroll" => "scroll",
            "type" => "type_text",
            "type_chars" => "type_text",
            "set_text" | "set_value" => "set_value",
            "keypress" => "press_key",
            "keyboard_shortcut" => "hotkey",
            "move" => "move_cursor",
            _ => "wait",
        },
        "pid": target.pid,
        "window_id": target.window_id,
        "session": session,
    });
    let mut args = scope;
    let object = args
        .as_object_mut()
        .expect("action arguments are an object");
    if action.action != "type_chars" {
        object.insert(
            "delivery_mode".into(),
            json!(action.delivery_mode.as_deref().unwrap_or("background")),
        );
    }
    match action.action.as_str() {
        "move" => {
            object.insert("x".into(), json!(action.x));
            object.insert("y".into(), json!(action.y));
        }
        "click" | "double_click" | "right_click" | "toggle" => {
            object.insert("x".into(), json!(action.x));
            object.insert("y".into(), json!(action.y));
            object.insert(
                "count".into(),
                json!(if action.action == "double_click" {
                    2
                } else {
                    1
                }),
            );
            object.insert(
                "button".into(),
                json!(
                    action
                        .button
                        .as_deref()
                        .unwrap_or(if action.action == "right_click" {
                            "right"
                        } else {
                            "left"
                        })
                ),
            );
        }
        "drag" => {
            let first = action.path.first().copied().unwrap_or(ComputerUsePoint {
                x: action.x.unwrap_or_default(),
                y: action.y.unwrap_or_default(),
            });
            let last = action.path.last().copied().unwrap_or(first);
            object.insert("from_x".into(), json!(first.x));
            object.insert("from_y".into(), json!(first.y));
            object.insert("to_x".into(), json!(last.x));
            object.insert("to_y".into(), json!(last.y));
            if let Some(button) = action.button.as_deref() {
                object.insert("button".into(), json!(button));
            }
            if !action.modifiers.is_empty() {
                object.insert("modifier".into(), json!(action.modifiers));
            }
            if let Some(duration_ms) = action.duration_ms {
                object.insert("duration_ms".into(), json!(duration_ms));
            }
            if let Some(steps) = action.steps {
                object.insert("steps".into(), json!(steps));
            }
        }
        "scroll" => {
            object.insert("x".into(), json!(action.x));
            object.insert("y".into(), json!(action.y));
            insert_scroll_arguments(object, action);
        }
        "type" => {
            object.insert(
                "text".into(),
                json!(action.text.as_deref().unwrap_or_default()),
            );
            if let (Some(x), Some(y)) = (action.x, action.y) {
                object.insert("x".into(), json!(x));
                object.insert("y".into(), json!(y));
            }
        }
        "type_chars" => {
            object.insert(
                "text".into(),
                json!(action.text.as_deref().unwrap_or_default()),
            );
            if let Some(delay_ms) = action.delay_ms {
                object.insert("delay_ms".into(), json!(delay_ms));
            }
        }
        "set_text" | "set_value" => {
            object.insert(
                "value".into(),
                json!(action.text.as_deref().unwrap_or_default()),
            );
        }
        "keypress" => {
            object.insert(
                "key".into(),
                json!(action.keys.first().cloned().unwrap_or_default()),
            );
            if let (Some(x), Some(y)) = (action.x, action.y) {
                object.insert("x".into(), json!(x));
                object.insert("y".into(), json!(y));
            }
            if !action.modifiers.is_empty() {
                object.insert("modifiers".into(), json!(action.modifiers));
            }
        }
        "keyboard_shortcut" => {
            object.insert("keys".into(), json!(action.keys));
            if let (Some(x), Some(y)) = (action.x, action.y) {
                object.insert("x".into(), json!(x));
                object.insert("y".into(), json!(y));
            }
        }
        _ => {}
    }
    if let Some(element_token) = action.element_token.as_deref() {
        object.insert("element_token".into(), json!(element_token));
        object.remove("element_index");
        object.remove("x");
        object.remove("y");
    } else if let Some(element_index) = action.element_index {
        object.insert("element_index".into(), json!(element_index));
        object.remove("x");
        object.remove("y");
    }
    args
}

#[cfg(any(windows, test))]
pub(crate) fn is_windows_uia_semantic_action(
    action: &ComputerUseAction,
    observation: &ComputerUseObservation,
) -> bool {
    matches!(
        action.action.as_str(),
        "click" | "toggle" | "set_text" | "set_value"
    ) && (action
        .element_token
        .as_deref()
        .is_some_and(|token| token.starts_with("dcc-wuia:"))
        || (action.element_index.is_some()
            && observation.capture_provenance["accessibility_backend"] == "windows_uia"))
}

pub(crate) fn validate_action_observation(
    action: &ComputerUseAction,
    observation: &ComputerUseObservation,
) -> ComputerUseResult<()> {
    if observation.capture_provenance["pixels_captured"] == false
        && action.element_index.is_none()
        && action.element_token.is_none()
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "coordinate and unscoped keyboard actions require a fresh pixel screenshot",
        ));
    }
    Ok(())
}

pub(crate) fn desktop_action_arguments(action: &ComputerUseAction, session: &str) -> Value {
    let mut args = json!({
        "_tool": match action.action.as_str() {
            "click" | "double_click" | "right_click" | "toggle" => "click",
            "drag" => "drag",
            "scroll" => "scroll",
            "type" | "type_chars" => "type_text",
            "keypress" => "press_key",
            "keyboard_shortcut" => "hotkey",
            "move" => "move_cursor",
            _ => "wait",
        },
        "session": session,
        "scope": "desktop",
    });
    let object = args
        .as_object_mut()
        .expect("desktop action arguments are an object");
    match action.action.as_str() {
        "move" => {
            object.insert("x".into(), json!(action.x));
            object.insert("y".into(), json!(action.y));
        }
        "click" | "double_click" | "right_click" | "toggle" => {
            object.insert("x".into(), json!(action.x));
            object.insert("y".into(), json!(action.y));
            object.insert(
                "count".into(),
                json!(if action.action == "double_click" {
                    2
                } else {
                    1
                }),
            );
            object.insert(
                "button".into(),
                json!(
                    action
                        .button
                        .as_deref()
                        .unwrap_or(if action.action == "right_click" {
                            "right"
                        } else {
                            "left"
                        })
                ),
            );
        }
        "drag" => {
            let first = action.path.first().copied().unwrap_or(ComputerUsePoint {
                x: action.x.unwrap_or_default(),
                y: action.y.unwrap_or_default(),
            });
            let last = action.path.last().copied().unwrap_or(first);
            object.insert("from_x".into(), json!(first.x));
            object.insert("from_y".into(), json!(first.y));
            object.insert("to_x".into(), json!(last.x));
            object.insert("to_y".into(), json!(last.y));
            if let Some(button) = action.button.as_deref() {
                object.insert("button".into(), json!(button));
            }
            if !action.modifiers.is_empty() {
                object.insert("modifier".into(), json!(action.modifiers));
            }
            if let Some(duration_ms) = action.duration_ms {
                object.insert("duration_ms".into(), json!(duration_ms));
            }
            if let Some(steps) = action.steps {
                object.insert("steps".into(), json!(steps));
            }
        }
        "scroll" => {
            object.insert("x".into(), json!(action.x));
            object.insert("y".into(), json!(action.y));
            insert_scroll_arguments(object, action);
        }
        "type" | "type_chars" => {
            object.insert(
                "text".into(),
                json!(action.text.as_deref().unwrap_or_default()),
            );
            if let Some(delay_ms) = action.delay_ms {
                object.insert("delay_ms".into(), json!(delay_ms));
            }
        }
        "keypress" => {
            object.insert(
                "key".into(),
                json!(action.keys.first().cloned().unwrap_or_default()),
            );
            if !action.modifiers.is_empty() {
                object.insert("modifiers".into(), json!(action.modifiers));
            }
        }
        "keyboard_shortcut" => {
            object.insert("keys".into(), json!(action.keys));
        }
        _ => {}
    }
    args
}

fn validate_scroll(action: &ComputerUseAction) -> ComputerUseResult<()> {
    let horizontal = action.scroll_x.unwrap_or_default();
    let vertical = action.scroll_y.unwrap_or_default();
    if horizontal != 0 && vertical != 0 {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "scroll supports one axis per action",
        ));
    }
    let amount = if horizontal != 0 {
        horizontal.unsigned_abs()
    } else {
        vertical.unsigned_abs()
    };
    if amount > MAX_SCROLL_AMOUNT {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            format!("scroll amount must be at most {MAX_SCROLL_AMOUNT}"),
        ));
    }
    if amount == 0
        && action.element_index.is_none()
        && action.element_token.is_none()
        && action.x.is_none()
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "scroll requires a non-zero axis, element locator, or coordinates",
        ));
    }
    if action
        .scroll_by
        .as_deref()
        .is_some_and(|value| !matches!(value, "line" | "page"))
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "scroll_by must be line or page",
        ));
    }
    Ok(())
}

fn insert_scroll_arguments(
    object: &mut serde_json::Map<String, Value>,
    action: &ComputerUseAction,
) {
    let horizontal = action.scroll_x.unwrap_or_default();
    let vertical = action.scroll_y.unwrap_or_default();
    let (direction, amount) = if horizontal < 0 {
        ("left", horizontal.unsigned_abs())
    } else if horizontal > 0 {
        ("right", horizontal.unsigned_abs())
    } else if vertical < 0 {
        ("up", vertical.unsigned_abs())
    } else if vertical > 0 {
        ("down", vertical.unsigned_abs())
    } else {
        ("down", 0)
    };
    object.insert("direction".into(), json!(direction));
    if amount > 0 {
        object.insert("amount".into(), json!(amount));
    }
    if let Some(by) = action.scroll_by.as_deref() {
        object.insert("by".into(), json!(by));
    }
}

pub(crate) fn action_for_window_visual_fallback(
    action: &ComputerUseAction,
    observation: &ComputerUseObservation,
) -> ComputerUseResult<ComputerUseAction> {
    if action.element_index.is_some()
        || action.element_token.is_some()
        || matches!(action.action.as_str(), "set_text" | "set_value")
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "window visual fallback does not support semantic element actions",
        ));
    }
    if matches!(
        action.action.as_str(),
        "click" | "double_click" | "right_click" | "toggle" | "move" | "scroll"
    ) && action.x.is_none()
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "window visual fallback pointer actions require screenshot coordinates",
        ));
    }
    if action.action == "drag" && action.path.is_empty() && action.x.is_none() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "window visual fallback drag requires screenshot coordinates",
        ));
    }

    let map_point = |point: ComputerUsePoint| -> ComputerUseResult<ComputerUsePoint> {
        if point.x >= f64::from(observation.width) || point.y >= f64::from(observation.height) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "window visual fallback coordinates exceed the latest screenshot",
            ));
        }
        Ok(ComputerUsePoint {
            x: point.x * f64::from(observation.source_rect[2]) / f64::from(observation.width),
            y: point.y * f64::from(observation.source_rect[3]) / f64::from(observation.height),
        })
    };
    let mut validated = action.clone();
    if let (Some(x), Some(y)) = (action.x, action.y) {
        let point = map_point(ComputerUsePoint { x, y })?;
        validated.x = Some(point.x);
        validated.y = Some(point.y);
    }
    validated.path = action
        .path
        .iter()
        .copied()
        .map(map_point)
        .collect::<ComputerUseResult<_>>()?;
    Ok(validated)
}

pub(crate) fn ensure_tool_ok(
    context: &str,
    result: &cua_driver_sdk::ToolResult,
) -> ComputerUseResult<()> {
    if result.is_error {
        let code = result.error_code.as_deref().unwrap_or_default();
        let message = if result.text.is_empty() {
            result.raw_json.clone()
        } else {
            result.text.clone()
        };
        let mapped = classify_driver_failure(code, &message, ComputerUseErrorCode::InputFailed);
        return Err(ComputerUseError::new(
            mapped,
            format!("{context}: {message}"),
        ));
    }
    Ok(())
}

pub(crate) fn map_driver_error(context: &str, error: impl std::fmt::Display) -> ComputerUseError {
    let message = error.to_string();
    let code = classify_driver_failure("", &message, ComputerUseErrorCode::BackendUnavailable);
    ComputerUseError::new(code, format!("{context}: {message}"))
}

fn classify_driver_failure(
    code: &str,
    message: &str,
    fallback: ComputerUseErrorCode,
) -> ComputerUseErrorCode {
    let lower = format!("{code} {message}").to_ascii_lowercase();
    if lower.contains("interrupt") || lower.contains("user_interrupted") {
        ComputerUseErrorCode::UserInterrupted
    } else if lower.contains("browser_") || lower.contains("browser") {
        ComputerUseErrorCode::BrowserRefused
    } else if lower.contains("clipboard") {
        ComputerUseErrorCode::ClipboardRefused
    } else if lower.contains("recording") || lower.contains("record_") {
        ComputerUseErrorCode::RecordingRefused
    } else if lower.contains("uia")
        || lower.contains("provider")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("invalid handle")
    {
        ComputerUseErrorCode::BackendUnavailable
    } else if lower.contains("window") || lower.contains("target") {
        ComputerUseErrorCode::InvalidTarget
    } else {
        fallback
    }
}

pub(crate) fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 || !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    Some((
        u32::from_be_bytes(data[16..20].try_into().ok()?),
        u32::from_be_bytes(data[20..24].try_into().ok()?),
    ))
}

pub(crate) fn is_uia_snapshot_failure(result: &cua_driver_sdk::ToolResult) -> bool {
    let message = format!(
        "{} {} {}",
        result.error_code.as_deref().unwrap_or_default(),
        result.text,
        result.raw_json
    );
    is_uia_snapshot_message(&message)
}

pub(crate) fn is_uia_snapshot_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("uia provider unresponsive")
        || lower.contains("get_window_state timed out")
        || (lower.contains("get_window_state") && lower.contains("desktop scope"))
}

#[cfg(any(windows, test))]
pub(crate) fn scale_bounds_for_dpi(
    bounds: [i32; 4],
    window_dpi: u32,
) -> ComputerUseResult<[i32; 4]> {
    if window_dpi == 0 || window_dpi == 96 {
        return Ok(bounds);
    }
    let [x, y, width, height] = bounds;
    let right = x.checked_add(width).ok_or_else(|| {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "desktop fallback target bounds overflow",
        )
    })?;
    let bottom = y.checked_add(height).ok_or_else(|| {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "desktop fallback target bounds overflow",
        )
    })?;
    let scale = 96.0 / f64::from(window_dpi);
    let left = (f64::from(x) * scale).floor() as i32;
    let top = (f64::from(y) * scale).floor() as i32;
    let right = (f64::from(right) * scale).ceil() as i32;
    let bottom = (f64::from(bottom) * scale).ceil() as i32;
    Ok([left, top, right - left, bottom - top])
}

pub(crate) fn crop_png_to_bounds(data: &[u8], bounds: [i32; 4]) -> ComputerUseResult<Vec<u8>> {
    let decoder = png::Decoder::new(Cursor::new(data));
    let mut reader = decoder.read_info().map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            format!("desktop fallback PNG decode failed: {error}"),
        )
    })?;
    let output_size = reader.output_buffer_size();
    let mut source = vec![0_u8; output_size];
    let info = reader.next_frame(&mut source).map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            format!("desktop fallback PNG frame decode failed: {error}"),
        )
    })?;
    if info.bit_depth != png::BitDepth::Eight {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "desktop fallback only supports 8-bit PNG screenshots",
        ));
    }
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "desktop fallback does not support indexed PNG screenshots",
            ));
        }
    };
    let [x, y, width, height] = bounds;
    if x < 0 || y < 0 || width <= 0 || height <= 0 {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "desktop fallback target bounds are outside the capturable display",
        ));
    }
    let right = x.checked_add(width).ok_or_else(|| {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "desktop fallback target bounds overflow",
        )
    })?;
    let bottom = y.checked_add(height).ok_or_else(|| {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "desktop fallback target bounds overflow",
        )
    })?;
    if right as u32 > info.width || bottom as u32 > info.height {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            format!(
                "desktop fallback target bounds {x},{y},{width},{height} exceed screenshot {}x{}",
                info.width, info.height
            ),
        ));
    }
    let row_bytes = (info.width as usize).checked_mul(channels).ok_or_else(|| {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "desktop fallback PNG row size overflow",
        )
    })?;
    let crop_row_bytes = (width as usize).checked_mul(channels).ok_or_else(|| {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            "desktop fallback crop row size overflow",
        )
    })?;
    let mut cropped = vec![0_u8; crop_row_bytes * height as usize];
    let source = &source[..info.buffer_size()];
    for row in 0..height as usize {
        let source_start = (y as usize + row) * row_bytes + x as usize * channels;
        let source_end = source_start + crop_row_bytes;
        let target_start = row * crop_row_bytes;
        cropped[target_start..target_start + crop_row_bytes]
            .copy_from_slice(&source[source_start..source_end]);
    }
    let mut output = Vec::new();
    let mut encoder = png::Encoder::new(&mut output, width as u32, height as u32);
    encoder.set_color(info.color_type);
    encoder.set_depth(info.bit_depth);
    let mut writer = encoder.write_header().map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            format!("desktop fallback PNG encoder failed: {error}"),
        )
    })?;
    writer.write_image_data(&cropped).map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            format!("desktop fallback PNG crop failed: {error}"),
        )
    })?;
    writer.finish().map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::CaptureFailed,
            format!("desktop fallback PNG finalize failed: {error}"),
        )
    })?;
    Ok(output)
}
