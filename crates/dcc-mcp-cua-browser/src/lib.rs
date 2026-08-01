//! Typed browser operations built on CUA's exact native-window binding.
//!
//! This crate deliberately owns browser target/tab/ref state. The generic CUA
//! core remains responsible for native-window scope and the allow-listed CUA
//! tool boundary; Host only transports these requests.

use base64::Engine;
use dcc_mcp_cua_core::{
    ComputerUseError, ComputerUseErrorCode, ComputerUseResult, ComputerUseSession,
};
use serde::Deserialize;
use serde_json::{Value, json};

const MAX_URL_CHARS: usize = 4_096;
const MAX_TEXT_CHARS: usize = 4_096;
const MAX_BROWSER_IMAGE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub struct BrowserResult {
    pub value: Value,
    pub image: Option<BrowserImage>,
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

impl BrowserSession {
    pub async fn prepare(
        &self,
        native: &ComputerUseSession,
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
        BrowserResult::from_value(native.call_browser_tool("browser_prepare", args).await?)
    }

    pub async fn snapshot(
        &mut self,
        native: &ComputerUseSession,
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
        native: &ComputerUseSession,
        request: BrowserNavigateRequest,
    ) -> ComputerUseResult<BrowserResult> {
        self.require_exact_target(&request.target_id)?;
        validate_url(&request.url)?;
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
        self.clear_snapshot();
        Ok(result)
    }

    pub async fn click(
        &mut self,
        native: &ComputerUseSession,
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
        let result =
            BrowserResult::from_value(native.call_browser_tool("browser_click", args).await?)?;
        self.clear_snapshot();
        Ok(result)
    }

    pub async fn type_text(
        &mut self,
        native: &ComputerUseSession,
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
        self.clear_snapshot();
        Ok(result)
    }

    pub async fn pointer(
        &mut self,
        native: &ComputerUseSession,
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
        let result =
            BrowserResult::from_value(native.call_browser_tool("browser_pointer", args).await?)?;
        self.clear_snapshot();
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
}

impl BrowserResult {
    fn from_value(mut value: Value) -> ComputerUseResult<Self> {
        let mut image = None;
        if let Some(content) = value["content"].as_array_mut() {
            for item in content.iter_mut() {
                if item["type"] != "image" || image.is_some() {
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
                image = Some(BrowserImage {
                    mime_type: mime_type.to_owned(),
                    data,
                });
                item.as_object_mut().map(|object| {
                    object.insert("data".into(), Value::String("[binary_frame]".into()));
                });
            }
        }
        Ok(Self { value, image })
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

fn invalid(message: impl Into<String>) -> ComputerUseError {
    ComputerUseError::new(ComputerUseErrorCode::InvalidAction, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_urls_are_scheme_and_length_bounded() {
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("file:///secret").is_err());
        assert!(validate_url(&format!("https://{}", "a".repeat(MAX_URL_CHARS))).is_err());
    }

    #[test]
    fn browser_routes_keep_dom_event_explicit() {
        assert_eq!(validate_route(None).unwrap(), "trusted");
        assert_eq!(validate_route(Some("dom_event")).unwrap(), "dom_event");
        assert!(validate_route(Some("fallback")).is_err());
    }

    #[test]
    fn browser_snapshot_id_supports_both_cua_formats() {
        assert_eq!(
            browser_snapshot_id(&json!({"structuredContent":{"snapshot_id":"p1"}})),
            Some("p1".into())
        );
        assert_eq!(
            browser_snapshot_id(&json!({"structuredContent":{"snapshot":{"id":"p2"}}})),
            Some("p2".into())
        );
    }
}
