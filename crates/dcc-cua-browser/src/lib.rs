//! Typed browser operations built on CUA's exact native-window binding.
//!
//! This crate deliberately owns browser target/tab/ref state. The generic CUA
//! core remains responsible for native-window scope and the allow-listed CUA
//! tool boundary; Host only transports these requests.

use base64::Engine;
use dcc_cua_core::{ComputerUseError, ComputerUseErrorCode, ComputerUseResult, ComputerUseSession};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

const MAX_URL_CHARS: usize = 4_096;
const MAX_TEXT_CHARS: usize = 4_096;
const MAX_BROWSER_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_LOCAL_PATH_CHARS: usize = 4_096;
const MAX_UPLOAD_FILES: usize = 32;

#[derive(Debug)]
pub struct BrowserResult {
    pub value: Value,
    pub images: Vec<BrowserImage>,
}

#[derive(Debug)]
pub struct BrowserImage {
    pub mime_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct BrowserSession {
    target_id: Option<String>,
    mutation_allowed: bool,
    latest_snapshot_id: Option<String>,
    latest_tab_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BrowserSnapshotRequest {
    #[serde(default)]
    pub target_id: Option<String>,
    #[serde(default)]
    pub tab_id: Option<String>,
    #[serde(default = "default_snapshot_format")]
    pub snapshot_format: String,
    #[serde(default)]
    pub scope_ref: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub continuation: Option<String>,
    #[serde(default)]
    pub include_screenshot: bool,
}

#[derive(Debug, Deserialize)]
pub struct BrowserPrepareRequest {
    #[serde(default)]
    pub approval_token: Option<String>,
    #[serde(default)]
    pub allow_launch: bool,
    #[serde(default)]
    pub profile: Option<Value>,
    #[serde(default)]
    pub strategy: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct BrowserNavigateRequest {
    pub target_id: String,
    pub tab_id: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct BrowserClickRequest {
    pub target_id: String,
    pub tab_id: String,
    pub snapshot_id: String,
    #[serde(rename = "ref", default)]
    pub element_ref: Option<String>,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub input_route: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BrowserTypeRequest {
    pub target_id: String,
    pub tab_id: String,
    pub snapshot_id: String,
    #[serde(rename = "ref")]
    pub element_ref: String,
    pub text: String,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub replace: bool,
}

#[derive(Debug, Deserialize)]
pub struct BrowserPointerRequest {
    pub target_id: String,
    pub tab_id: String,
    pub snapshot_id: String,
    #[serde(rename = "ref", default)]
    pub element_ref: Option<String>,
    #[serde(rename = "destination_ref", default)]
    pub destination_ref: Option<String>,
    pub action: String,
    #[serde(default)]
    pub input_route: Option<String>,
    #[serde(default)]
    pub delta_x: Option<f64>,
    #[serde(default)]
    pub delta_y: Option<f64>,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct BrowserSetInputFilesRequest {
    pub target_id: String,
    pub tab_id: String,
    pub snapshot_id: String,
    #[serde(rename = "ref")]
    pub element_ref: String,
    pub files: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct BrowserDownloadRequest {
    pub target_id: String,
    pub tab_id: String,
    pub snapshot_id: String,
    #[serde(rename = "ref")]
    pub element_ref: String,
    pub destination_root: String,
}

#[derive(Debug, Deserialize)]
pub struct BrowserDialogRequest {
    pub target_id: String,
    pub tab_id: String,
    pub action: String,
    #[serde(default)]
    pub dialog_id: Option<String>,
    #[serde(default)]
    pub prompt_text: Option<String>,
    #[serde(default)]
    pub delivery_mode: Option<String>,
}

impl BrowserSession {
    /// Preserve the exact browser binding but fence coordinates from an old viewport.
    pub fn invalidate_snapshot(&mut self) {
        self.clear_snapshot();
    }

    pub async fn prepare(
        &mut self,
        native: &mut ComputerUseSession,
        request: BrowserPrepareRequest,
    ) -> ComputerUseResult<BrowserResult> {
        if request
            .approval_token
            .as_deref()
            .is_some_and(|value| value.chars().count() > MAX_TEXT_CHARS)
        {
            return Err(invalid(
                "browser approval_token exceeds the 4096-character limit",
            ));
        }
        let mut args = json!({"allow_launch": request.allow_launch});
        if let Some(value) = request.approval_token {
            args["approval_token"] = json!(value);
        }
        if let Some(value) = request.profile {
            args["profile"] = value;
        }
        if let Some(value) = request.strategy {
            args["strategy"] = value;
        }
        self.begin_snapshot_sensitive_native_attempt();
        BrowserResult::from_value(native.call_browser_tool("browser_prepare", args).await?)
    }

    pub async fn snapshot(
        &mut self,
        native: &mut ComputerUseSession,
        request: BrowserSnapshotRequest,
    ) -> ComputerUseResult<BrowserResult> {
        if request.snapshot_format != "dom_refs_v1" && request.snapshot_format != "semantic_v2" {
            return Err(invalid(
                "snapshot_format must be dom_refs_v1 or semantic_v2",
            ));
        }
        let mut args = json!({
            "snapshot_format": request.snapshot_format,
            "include_screenshot": request.include_screenshot,
        });
        if let Some(value) = request.scope_ref {
            args["scope_ref"] = json!(value);
        }
        if let Some(value) = request.query {
            args["query"] = json!(value);
        }
        if let Some(value) = request.continuation {
            args["continuation"] = json!(value);
        }

        match request.target_id {
            None => {
                if request.tab_id.is_some() {
                    return Err(invalid("tab_id requires target_id"));
                }
                self.begin_snapshot_sensitive_native_attempt();
                let result = BrowserResult::from_value(
                    native.call_browser_tool("get_browser_state", args).await?,
                )?;
                self.store_binding(&result.value)?;
                Ok(result)
            }
            Some(target_id) => {
                self.require_bound_target(&target_id)?;
                let tab_id = request
                    .tab_id
                    .ok_or_else(|| invalid("tab_id is required for browser snapshot mode"))?;
                if tab_id.trim().is_empty() {
                    return Err(invalid("tab_id must not be empty"));
                }
                args["target_id"] = json!(target_id);
                args["tab_id"] = json!(tab_id);
                self.begin_snapshot_sensitive_native_attempt();
                let result = BrowserResult::from_value(
                    native.call_browser_tool("get_browser_state", args).await?,
                )?;
                self.latest_snapshot_id = browser_snapshot_id(&result.value);
                self.latest_tab_id = Some(tab_id);
                Ok(result)
            }
        }
    }

    pub async fn navigate(
        &mut self,
        native: &mut ComputerUseSession,
        request: BrowserNavigateRequest,
    ) -> ComputerUseResult<BrowserResult> {
        self.require_exact_target(&request.target_id)?;
        validate_url(&request.url)?;
        self.begin_snapshot_sensitive_native_attempt();
        let result = BrowserResult::from_value(
            native
                .call_browser_tool(
                    "browser_navigate",
                    json!({
                        "target_id": request.target_id,
                        "tab_id": request.tab_id,
                        "url": request.url,
                    }),
                )
                .await?,
        )?;
        Ok(result)
    }

    pub async fn click(
        &mut self,
        native: &mut ComputerUseSession,
        request: BrowserClickRequest,
    ) -> ComputerUseResult<BrowserResult> {
        self.require_mutation_target(&request.target_id, &request.tab_id, &request.snapshot_id)?;
        let route = validate_route(request.input_route.as_deref())?;
        if request.element_ref.is_none() && request.x.is_none() {
            return Err(invalid("browser_click requires ref or x/y coordinates"));
        }
        if request.x.is_some() != request.y.is_some() {
            return Err(invalid("browser_click coordinates require both x and y"));
        }
        if route == "dom_event" && request.element_ref.is_none() {
            return Err(invalid("dom_event browser_click requires ref"));
        }
        let mut args = json!({
            "target_id": request.target_id,
            "tab_id": request.tab_id,
            "input_route": route,
        });
        if let Some(value) = request.element_ref {
            args["ref"] = json!(value);
        }
        if let Some(value) = request.x {
            args["x"] = json!(value);
            args["y"] = json!(request.y.expect("x/y pair validated"));
        }
        self.begin_snapshot_sensitive_native_attempt();
        let result =
            BrowserResult::from_value(native.call_browser_tool("browser_click", args).await?)?;
        Ok(result)
    }

    pub async fn type_text(
        &mut self,
        native: &mut ComputerUseSession,
        request: BrowserTypeRequest,
    ) -> ComputerUseResult<BrowserResult> {
        self.require_mutation_target(&request.target_id, &request.tab_id, &request.snapshot_id)?;
        if request.element_ref.trim().is_empty() {
            return Err(invalid("browser_type ref must not be empty"));
        }
        if request.text.chars().count() > MAX_TEXT_CHARS {
            return Err(invalid(
                "browser_type text exceeds the 4096-character limit",
            ));
        }
        let mode = request.mode.as_deref().unwrap_or("insert_text");
        if mode != "insert_text" && mode != "keystrokes" {
            return Err(invalid(
                "browser_type mode must be insert_text or keystrokes",
            ));
        }
        self.begin_snapshot_sensitive_native_attempt();
        let result = BrowserResult::from_value(
            native
                .call_browser_tool(
                    "browser_type",
                    json!({
                        "target_id": request.target_id,
                        "tab_id": request.tab_id,
                        "ref": request.element_ref,
                        "text": request.text,
                        "mode": mode,
                        "replace": request.replace,
                    }),
                )
                .await?,
        )?;
        Ok(result)
    }

    pub async fn pointer(
        &mut self,
        native: &mut ComputerUseSession,
        request: BrowserPointerRequest,
    ) -> ComputerUseResult<BrowserResult> {
        self.require_mutation_target(&request.target_id, &request.tab_id, &request.snapshot_id)?;
        if !matches!(
            request.action.as_str(),
            "hover" | "right_click" | "double_click" | "scroll" | "drag"
        ) {
            return Err(invalid("unsupported browser_pointer action"));
        }
        let route = validate_route(request.input_route.as_deref())?;
        if route == "dom_event" && request.element_ref.is_none() {
            return Err(invalid("dom_event browser_pointer requires ref"));
        }
        if request.action == "drag" && request.destination_ref.is_none() {
            return Err(invalid("browser drag requires destination_ref"));
        }
        let mut args = json!({
            "target_id": request.target_id,
            "tab_id": request.tab_id,
            "action": request.action,
            "input_route": route,
        });
        if let Some(value) = request.element_ref {
            args["ref"] = json!(value);
        }
        if let Some(value) = request.destination_ref {
            args["destination_ref"] = json!(value);
        }
        for (key, value) in [
            ("delta_x", request.delta_x),
            ("delta_y", request.delta_y),
            ("x", request.x),
            ("y", request.y),
        ] {
            if let Some(value) = value {
                args[key] = json!(value);
            }
        }
        self.begin_snapshot_sensitive_native_attempt();
        let result =
            BrowserResult::from_value(native.call_browser_tool("browser_pointer", args).await?)?;
        Ok(result)
    }

    pub async fn set_input_files(
        &mut self,
        native: &mut ComputerUseSession,
        request: BrowserSetInputFilesRequest,
    ) -> ComputerUseResult<BrowserResult> {
        self.require_mutation_target(&request.target_id, &request.tab_id, &request.snapshot_id)?;
        validate_ref(&request.element_ref, "browser_set_input_files ref")?;
        let files = validate_upload_paths(&request.files)?;
        self.begin_snapshot_sensitive_native_attempt();
        let result = BrowserResult::from_value(
            native
                .call_browser_tool(
                    "browser_set_input_files",
                    json!({
                        "target_id": request.target_id,
                        "tab_id": request.tab_id,
                        "ref": request.element_ref,
                        "files": files,
                    }),
                )
                .await?,
        )?;
        Ok(result)
    }

    pub async fn download(
        &mut self,
        native: &mut ComputerUseSession,
        request: BrowserDownloadRequest,
    ) -> ComputerUseResult<BrowserResult> {
        self.require_mutation_target(&request.target_id, &request.tab_id, &request.snapshot_id)?;
        validate_ref(&request.element_ref, "browser_download ref")?;
        let destination_root = validate_destination_root(&request.destination_root)?;
        self.begin_snapshot_sensitive_native_attempt();
        let result = BrowserResult::from_value(
            native
                .call_browser_download_tool(json!({
                    "target_id": request.target_id,
                    "tab_id": request.tab_id,
                    "ref": request.element_ref,
                    "destination_root": destination_root,
                }))
                .await?,
        )?;
        Ok(result)
    }

    pub async fn dialog(
        &mut self,
        native: &mut ComputerUseSession,
        request: BrowserDialogRequest,
    ) -> ComputerUseResult<BrowserResult> {
        self.require_exact_target(&request.target_id)?;
        validate_dialog_request(&request)?;
        let delivery_mode = request.delivery_mode.as_deref().unwrap_or("background");
        let mut args = json!({
            "target_id": request.target_id,
            "tab_id": request.tab_id,
            "action": request.action,
            "delivery_mode": delivery_mode,
        });
        if let Some(dialog_id) = request.dialog_id {
            args["dialog_id"] = json!(dialog_id);
        }
        if let Some(prompt_text) = request.prompt_text {
            args["prompt_text"] = json!(prompt_text);
        }
        let action = request.action;
        if action != "inspect" {
            self.begin_snapshot_sensitive_native_attempt();
        }
        let result =
            BrowserResult::from_value(native.call_browser_tool("browser_dialog", args).await?)?;
        Ok(result)
    }

    fn store_binding(&mut self, value: &Value) -> ComputerUseResult<()> {
        let structured = structured_content(value)?;
        if let Some(refusal) = structured["refusal"].as_object() {
            let code = refusal["code"].as_str().unwrap_or("browser_refused");
            let message = refusal["message"]
                .as_str()
                .unwrap_or("CUA refused the browser binding");
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BrowserRefused,
                format!("{code}: {message}"),
            ));
        }
        let target_id = structured["target_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid("browser bind omitted target_id"))?;
        self.target_id = Some(target_id.to_owned());
        self.mutation_allowed =
            structured["binding_quality"] == "exact" && structured["mutation_allowed"] == true;
        self.clear_snapshot();
        Ok(())
    }

    fn require_bound_target(&self, target_id: &str) -> ComputerUseResult<()> {
        if self.target_id.as_deref() != Some(target_id) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "browser target is not bound to this Host session",
            ));
        }
        Ok(())
    }

    fn require_exact_target(&self, target_id: &str) -> ComputerUseResult<()> {
        self.require_bound_target(target_id)?;
        if !self.mutation_allowed {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BrowserRefused,
                "browser binding is not exact and is read-only",
            ));
        }
        Ok(())
    }

    fn require_mutation_target(
        &self,
        target_id: &str,
        tab_id: &str,
        snapshot_id: &str,
    ) -> ComputerUseResult<()> {
        self.require_exact_target(target_id)?;
        if self.latest_snapshot_id.is_none() || self.latest_tab_id.as_deref() != Some(tab_id) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "take a fresh browser snapshot before mutating the tab",
            ));
        }
        if self.latest_snapshot_id.as_deref() != Some(snapshot_id) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "browser snapshot_id does not match the latest browser snapshot",
            ));
        }
        Ok(())
    }

    fn clear_snapshot(&mut self) {
        self.latest_snapshot_id = None;
        self.latest_tab_id = None;
    }

    fn begin_snapshot_sensitive_native_attempt(&mut self) {
        self.clear_snapshot();
    }
}

impl BrowserResult {
    fn from_value(mut value: Value) -> ComputerUseResult<Self> {
        let mut images = Vec::new();
        if let Some(content) = value["content"].as_array_mut() {
            for item in content.iter_mut() {
                if item["type"] != "image" {
                    continue;
                }
                let mime_type = item["mimeType"].as_str().unwrap_or("image/png");
                let data = item["data"].as_str().ok_or_else(|| {
                    ComputerUseError::new(
                        ComputerUseErrorCode::BackendUnavailable,
                        "CUA browser image omitted base64 data",
                    )
                })?;
                let data = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|error| {
                        ComputerUseError::new(
                            ComputerUseErrorCode::BackendUnavailable,
                            format!("CUA browser image is not valid base64: {error}"),
                        )
                    })?;
                if data.len() > MAX_BROWSER_IMAGE_BYTES {
                    return Err(ComputerUseError::new(
                        ComputerUseErrorCode::CaptureFailed,
                        "CUA browser image exceeds the 64 MiB frame limit",
                    ));
                }
                images.push(BrowserImage {
                    mime_type: mime_type.to_owned(),
                    data,
                });
                if let Some(object) = item.as_object_mut() {
                    object.insert("data".into(), Value::Null);
                }
            }
        }
        Ok(Self { value, images })
    }
}

fn default_snapshot_format() -> String {
    "semantic_v2".into()
}

fn structured_content(value: &Value) -> ComputerUseResult<&Value> {
    value["structuredContent"]
        .as_object()
        .map(|_| &value["structuredContent"])
        .ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "CUA browser tool omitted structuredContent",
            )
        })
}

fn browser_snapshot_id(value: &Value) -> Option<String> {
    value["structuredContent"]["snapshot_id"]
        .as_str()
        .or_else(|| value["structuredContent"]["snapshot"]["id"].as_str())
        .map(ToOwned::to_owned)
}

fn validate_url(url: &str) -> ComputerUseResult<()> {
    if url.chars().count() > MAX_URL_CHARS {
        return Err(invalid("browser URL exceeds the 4096-character limit"));
    }
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("about:"))
    {
        return Err(invalid(
            "browser_navigate accepts only http, https, and about URLs",
        ));
    }
    Ok(())
}

fn validate_route(route: Option<&str>) -> ComputerUseResult<&str> {
    let route = route.unwrap_or("trusted");
    if route != "trusted" && route != "dom_event" {
        return Err(invalid("input_route must be trusted or dom_event"));
    }
    Ok(route)
}

fn validate_ref(value: &str, field: &str) -> ComputerUseResult<()> {
    if value.trim().is_empty() {
        return Err(invalid(format!("{field} must not be empty")));
    }
    Ok(())
}

fn validate_upload_paths(paths: &[String]) -> ComputerUseResult<Vec<String>> {
    if paths.is_empty() || paths.len() > MAX_UPLOAD_FILES {
        return Err(invalid(format!(
            "browser upload requires between 1 and {MAX_UPLOAD_FILES} files"
        )));
    }
    paths
        .iter()
        .map(|raw| {
            validate_local_path(raw, "upload path")?;
            let path = Path::new(raw);
            let metadata = std::fs::symlink_metadata(path).map_err(|_| {
                invalid("browser upload paths must name existing regular files")
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(invalid(
                    "browser upload paths must name regular files directly, not links or directories",
                ));
            }
            std::fs::canonicalize(path)
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|_| invalid("browser upload path could not be canonicalized"))
        })
        .collect()
}

fn validate_destination_root(raw: &str) -> ComputerUseResult<String> {
    validate_local_path(raw, "browser download destination_root")?;
    let path = Path::new(raw);
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| invalid("browser download destination_root must be an existing directory"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid(
            "browser download destination_root must be a direct directory, not a link",
        ));
    }
    std::fs::canonicalize(path)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|_| invalid("browser download destination_root could not be canonicalized"))
}

fn validate_dialog_request(request: &BrowserDialogRequest) -> ComputerUseResult<()> {
    if !matches!(request.action.as_str(), "inspect" | "accept" | "dismiss") {
        return Err(invalid(
            "browser_dialog action must be inspect, accept, or dismiss",
        ));
    }
    if request.action != "inspect" && request.dialog_id.is_none() {
        return Err(invalid("browser_dialog accept/dismiss requires dialog_id"));
    }
    if let Some(prompt_text) = request.prompt_text.as_deref() {
        if request.action != "accept" {
            return Err(invalid(
                "browser_dialog prompt_text is valid only for accept",
            ));
        }
        if prompt_text.chars().count() > MAX_TEXT_CHARS {
            return Err(invalid(
                "browser_dialog prompt_text exceeds the 4096-character limit",
            ));
        }
    }
    if let Some(delivery_mode) = request.delivery_mode.as_deref()
        && delivery_mode != "background"
        && delivery_mode != "foreground"
    {
        return Err(invalid(
            "browser_dialog delivery_mode must be background or foreground",
        ));
    }
    Ok(())
}

fn validate_local_path(raw: &str, field: &str) -> ComputerUseResult<()> {
    if raw.is_empty() || raw.chars().count() > MAX_LOCAL_PATH_CHARS || raw.contains('\0') {
        return Err(invalid(format!(
            "{field} must be a bounded absolute path without NUL bytes"
        )));
    }
    if !Path::new(raw).is_absolute() {
        return Err(invalid(format!("{field} must be absolute")));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> ComputerUseError {
    ComputerUseError::new(ComputerUseErrorCode::InvalidAction, message)
}

#[cfg(test)]
mod tests;
