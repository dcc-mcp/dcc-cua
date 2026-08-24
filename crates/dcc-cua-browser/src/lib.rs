//! Typed browser operations built on CUA's exact native-window binding.
//!
//! This crate deliberately owns browser target/tab/ref state. The generic CUA
//! core remains responsible for native-window scope and the allow-listed CUA
//! tool boundary; Host only transports these requests.

use base64::Engine;
use dcc_cua_core::{ComputerUseError, ComputerUseErrorCode, ComputerUseResult, ComputerUseSession};
use dcc_cua_protocol::{
    MAX_BINARY_FRAME_BYTES, validate_absolute_local_path, validate_secret_handle,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::path::Path;

const MAX_URL_CHARS: usize = 4_096;
const MAX_TEXT_CHARS: usize = 4_096;
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
    latest_origin: Option<String>,
    pending_ancestor_continuation: Option<PendingAncestorContinuation>,
}

#[derive(Clone, Debug)]
struct PendingAncestorContinuation {
    token: String,
    target_id: String,
    tab_id: String,
    snapshot_id: String,
    anchor: Value,
}

#[derive(Clone, Debug)]
enum AncestorScopeExpectation {
    Initial {
        requested_ref: String,
        role: String,
        target_id: String,
        tab_id: String,
    },
    Continuation(PendingAncestorContinuation),
    UntrackedContinuation {
        target_id: String,
        tab_id: String,
    },
}

#[derive(Debug)]
struct ValidatedAncestorScope {
    snapshot_id: String,
    anchor: Value,
    continuation: Option<String>,
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
    #[serde(default, deserialize_with = "deserialize_optional_nonnull_string")]
    pub scope_ancestor_role: Option<String>,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BrowserTypeRequest {
    pub target_id: String,
    pub tab_id: String,
    pub snapshot_id: String,
    #[serde(rename = "ref")]
    pub element_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_handle: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub replace: bool,
}

pub struct ResolvedBrowserTypeRequest {
    target_id: String,
    tab_id: String,
    snapshot_id: String,
    element_ref: String,
    text: String,
    mode: Option<String>,
    replace: bool,
}

impl std::fmt::Debug for ResolvedBrowserTypeRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedBrowserTypeRequest")
            .field("target_id", &self.target_id)
            .field("tab_id", &self.tab_id)
            .field("snapshot_id", &self.snapshot_id)
            .field("element_ref", &self.element_ref)
            .field("text", &"[REDACTED]")
            .field("mode", &self.mode)
            .field("replace", &self.replace)
            .finish()
    }
}

impl BrowserTypeRequest {
    pub fn validate_source(&self) -> ComputerUseResult<()> {
        match (&self.text, &self.secret_handle) {
            (Some(text), None) => {
                if text.chars().count() > MAX_TEXT_CHARS {
                    return Err(invalid(
                        "browser_type text exceeds the 4096-character limit",
                    ));
                }
            }
            (None, Some(handle)) => {
                validate_secret_handle(handle)
                    .map_err(|_| invalid("browser_type secret_handle is invalid"))?;
            }
            (Some(_), Some(_)) => {
                return Err(invalid(
                    "browser_type text and secret_handle are mutually exclusive",
                ));
            }
            (None, None) => {
                return Err(invalid(
                    "browser_type requires exactly one of text or secret_handle",
                ));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn secret_handle(&self) -> Option<&str> {
        self.secret_handle.as_deref()
    }

    #[must_use]
    pub fn snapshot_id(&self) -> &str {
        &self.snapshot_id
    }

    #[must_use]
    pub fn tab_id(&self) -> &str {
        &self.tab_id
    }

    pub fn resolve(
        self,
        resolved_secret: Option<&str>,
    ) -> ComputerUseResult<ResolvedBrowserTypeRequest> {
        self.validate_source()?;
        let text = match (self.text, self.secret_handle, resolved_secret) {
            (Some(text), None, None) => text,
            (None, Some(handle), Some(secret)) => {
                debug_assert!(validate_secret_handle(&handle).is_ok());
                secret.to_owned()
            }
            (Some(_), Some(_), _) | (None, None, _) => unreachable!("source was validated"),
            (Some(_), None, Some(_)) | (None, Some(_), None) => {
                return Err(invalid(
                    "browser_type secret resolution does not match its input",
                ));
            }
        };
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(invalid(
                "browser_type text exceeds the 4096-character limit",
            ));
        }
        Ok(ResolvedBrowserTypeRequest {
            target_id: self.target_id,
            tab_id: self.tab_id,
            snapshot_id: self.snapshot_id,
            element_ref: self.element_ref,
            text,
            mode: self.mode,
            replace: self.replace,
        })
    }
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
        let mut args = browser_snapshot_args(&request)?;
        let expected_ancestor_scope = self.ancestor_scope_expectation(&request)?;
        match request.target_id {
            None => {
                if request.tab_id.is_some() {
                    return Err(invalid("tab_id requires target_id"));
                }
                self.begin_snapshot_sensitive_native_attempt();
                let result = BrowserResult::from_value(
                    native.call_browser_tool("get_browser_state", args).await?,
                )?;
                let validated = validate_ancestor_scope_response(
                    &result.value,
                    expected_ancestor_scope.as_ref(),
                )?;
                if validated.is_some() {
                    return Err(ComputerUseError::new(
                        ComputerUseErrorCode::BackendUnavailable,
                        "CUA ancestor-scoped snapshot omitted its exact target and tab",
                    ));
                }
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
                if browser_result_is_structured_refusal(&result.value)? {
                    return Ok(result);
                }
                let validated = validate_ancestor_scope_response(
                    &result.value,
                    expected_ancestor_scope.as_ref(),
                )?;
                self.latest_snapshot_id = validated
                    .as_ref()
                    .map(|scope| scope.snapshot_id.clone())
                    .or_else(|| browser_snapshot_id(&result.value));
                self.latest_tab_id = Some(tab_id.clone());
                self.latest_origin = browser_snapshot_origin(&result.value, &tab_id);
                self.pending_ancestor_continuation = validated.and_then(|scope| {
                    scope.continuation.map(|token| PendingAncestorContinuation {
                        token,
                        target_id: target_id.clone(),
                        tab_id: tab_id.clone(),
                        snapshot_id: scope.snapshot_id,
                        anchor: scope.anchor,
                    })
                });
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
        request: ResolvedBrowserTypeRequest,
    ) -> ComputerUseResult<BrowserResult> {
        self.require_mutation_target(&request.target_id, &request.tab_id, &request.snapshot_id)?;
        if request.element_ref.trim().is_empty() {
            return Err(invalid("browser_type ref must not be empty"));
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

    pub fn validate_type_request(&self, request: &BrowserTypeRequest) -> ComputerUseResult<()> {
        request.validate_source()?;
        self.require_mutation_target(&request.target_id, &request.tab_id, &request.snapshot_id)?;
        if request.element_ref.trim().is_empty() {
            return Err(invalid("browser_type ref must not be empty"));
        }
        let mode = request.mode.as_deref().unwrap_or("insert_text");
        if mode != "insert_text" && mode != "keystrokes" {
            return Err(invalid(
                "browser_type mode must be insert_text or keystrokes",
            ));
        }
        Ok(())
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
        self.begin_dialog_attempt();
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
        self.latest_origin = None;
        self.pending_ancestor_continuation = None;
    }

    #[must_use]
    pub fn latest_origin(&self) -> Option<&str> {
        self.latest_origin.as_deref()
    }

    fn ancestor_scope_expectation(
        &self,
        request: &BrowserSnapshotRequest,
    ) -> ComputerUseResult<Option<AncestorScopeExpectation>> {
        if let Some(role) = request.scope_ancestor_role.as_deref() {
            return Ok(Some(AncestorScopeExpectation::Initial {
                requested_ref: request.scope_ref.as_deref().unwrap_or_default().to_owned(),
                role: role.to_ascii_lowercase(),
                target_id: request.target_id.as_deref().unwrap_or_default().to_owned(),
                tab_id: request.tab_id.as_deref().unwrap_or_default().to_owned(),
            }));
        }
        let Some(token) = request.continuation.as_deref() else {
            return Ok(None);
        };
        let Some(pending) = self.pending_ancestor_continuation.as_ref() else {
            return Ok(
                match (request.target_id.as_deref(), request.tab_id.as_deref()) {
                    (Some(target_id), Some(tab_id)) => {
                        Some(AncestorScopeExpectation::UntrackedContinuation {
                            target_id: target_id.to_owned(),
                            tab_id: tab_id.to_owned(),
                        })
                    }
                    _ => None,
                },
            );
        };
        if token == pending.token.as_str()
            && request.target_id.as_deref() == Some(pending.target_id.as_str())
            && request.tab_id.as_deref() == Some(pending.tab_id.as_str())
        {
            if request.snapshot_format != "semantic_v2"
                || request.scope_ref.is_some()
                || request.query.is_some()
            {
                return Err(invalid(
                    "tracked ancestor continuation requires semantic_v2 and cannot include scope_ref or query",
                ));
            }
            Ok(Some(AncestorScopeExpectation::Continuation(
                pending.clone(),
            )))
        } else {
            Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "ancestor-scoped continuation does not match the latest exact tab evidence",
            ))
        }
    }

    fn begin_snapshot_sensitive_native_attempt(&mut self) {
        self.clear_snapshot();
    }

    fn begin_dialog_attempt(&mut self) {
        self.begin_snapshot_sensitive_native_attempt();
    }
}

impl BrowserResult {
    #[must_use]
    pub fn publishes_snapshot_evidence(&self) -> bool {
        structured_content(&self.value).is_ok_and(|structured| {
            structured["status"] == "ok"
                && structured["mode"] == "snapshot"
                && structured["refusal"].is_null()
                && browser_snapshot_id(&self.value).is_some()
        })
    }

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
                if data.len() > MAX_BINARY_FRAME_BYTES {
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

fn browser_snapshot_origin(value: &Value, tab_id: &str) -> Option<String> {
    let structured = &value["structuredContent"];
    let explicit = structured["origin"].as_str();
    if let Some(origin) = explicit.filter(|origin| valid_origin(origin)) {
        return Some(origin.to_owned());
    }
    let url = structured["url"]
        .as_str()
        .or_else(|| structured["snapshot"]["url"].as_str())
        .or_else(|| {
            (structured["tab_id"].as_str() == Some(tab_id))
                .then(|| structured["page"]["url"].as_str())
                .flatten()
        })
        .or_else(|| {
            structured["tabs"].as_array().and_then(|tabs| {
                tabs.iter()
                    .find(|tab| tab["tab_id"].as_str() == Some(tab_id))
                    .and_then(|tab| tab["url"].as_str())
            })
        })?;
    let parsed = url::Url::parse(url).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| parsed.origin().ascii_serialization())
}

fn valid_origin(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.origin().ascii_serialization() == value
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

fn deserialize_optional_nonnull_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::String(value) => Ok(Some(value)),
        _ => Err(serde::de::Error::custom(
            "scope_ancestor_role must be a string when present",
        )),
    }
}

fn browser_snapshot_args(request: &BrowserSnapshotRequest) -> ComputerUseResult<Value> {
    validate_ancestor_scope_request(request)?;
    let mut args = json!({
        "snapshot_format": request.snapshot_format,
        "include_screenshot": request.include_screenshot,
    });
    if let Some(value) = request.scope_ref.as_deref() {
        args["scope_ref"] = json!(value);
    }
    if let Some(value) = request.scope_ancestor_role.as_deref() {
        args["scope_ancestor_role"] = json!(value);
    }
    if let Some(value) = request.query.as_deref() {
        args["query"] = json!(value);
    }
    if let Some(value) = request.continuation.as_deref() {
        args["continuation"] = json!(value);
    }
    Ok(args)
}

fn validate_ancestor_scope_request(request: &BrowserSnapshotRequest) -> ComputerUseResult<()> {
    let Some(role) = request.scope_ancestor_role.as_deref() else {
        return Ok(());
    };
    if request.snapshot_format != "semantic_v2" {
        return Err(invalid(
            "scope_ancestor_role requires snapshot_format semantic_v2",
        ));
    }
    if request.target_id.as_deref().is_none_or(str::is_empty)
        || request.tab_id.as_deref().is_none_or(str::is_empty)
    {
        return Err(invalid(
            "scope_ancestor_role requires both target_id and tab_id",
        ));
    }
    if request.scope_ref.as_deref().is_none_or(str::is_empty) || request.query.is_none() {
        return Err(invalid(
            "scope_ancestor_role requires both scope_ref and query",
        ));
    }
    if request.continuation.is_some() {
        return Err(invalid(
            "scope_ancestor_role cannot be combined with continuation",
        ));
    }
    if role.is_empty()
        || role.len() > 128
        || !role.is_ascii()
        || !role.as_bytes()[0].is_ascii_alphabetic()
        || !role
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid(
            "scope_ancestor_role must be an ASCII accessibility role of 1 to 128 bytes",
        ));
    }
    Ok(())
}

fn validate_ancestor_scope_response(
    value: &Value,
    expected: Option<&AncestorScopeExpectation>,
) -> ComputerUseResult<Option<ValidatedAncestorScope>> {
    let Some(expected) = expected else {
        // Legacy direct scope_ref snapshots also carry a distance-zero anchor.
        // They predate nearest_ancestor_role_v1 and remain pass-through.
        return Ok(None);
    };
    let structured = structured_content(value)?;
    if structured["status"] != "ok" || !structured["refusal"].is_null() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "CUA ancestor-scoped snapshot omitted a successful status",
        ));
    }
    let snapshot = structured["snapshot"].as_object().ok_or_else(|| {
        ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "CUA ancestor-scoped snapshot omitted snapshot evidence",
        )
    })?;
    let (expected_target_id, expected_tab_id) = match expected {
        AncestorScopeExpectation::Initial {
            target_id, tab_id, ..
        }
        | AncestorScopeExpectation::UntrackedContinuation { target_id, tab_id } => {
            (target_id.as_str(), tab_id.as_str())
        }
        AncestorScopeExpectation::Continuation(pending) => {
            (pending.target_id.as_str(), pending.tab_id.as_str())
        }
    };
    if structured["target_id"].as_str() != Some(expected_target_id)
        || structured["tab_id"].as_str() != Some(expected_tab_id)
        || snapshot.get("format").and_then(Value::as_str) != Some("semantic_v2")
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "CUA ancestor-scoped snapshot returned inconsistent target, tab, or format evidence",
        ));
    }
    let snapshot_id = snapshot
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "CUA ancestor-scoped snapshot omitted its snapshot id",
            )
        })?;
    match structured.get("snapshot_id") {
        None | Some(Value::Null) => {}
        Some(Value::String(legacy_id)) if legacy_id == snapshot_id => {}
        Some(_) => {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "CUA ancestor-scoped snapshot returned conflicting snapshot ids",
            ));
        }
    }
    if matches!(
        expected,
        AncestorScopeExpectation::UntrackedContinuation { .. }
    ) {
        if snapshot.get("scope").and_then(Value::as_str) != Some("continuation") {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "CUA untracked continuation returned inconsistent scope evidence",
            ));
        }
        match snapshot.get("scope_anchor") {
            Some(Value::Null) => return Ok(None),
            Some(anchor) => match anchor.get("distance").and_then(Value::as_u64) {
                Some(0) => return Ok(None),
                Some(_) => {
                    return Err(ComputerUseError::new(
                        ComputerUseErrorCode::StaleObservation,
                        "ancestor-scoped continuation is no longer tracked by this Host session",
                    ));
                }
                None => {
                    return Err(ComputerUseError::new(
                        ComputerUseErrorCode::BackendUnavailable,
                        "CUA continuation returned malformed scope anchor evidence",
                    ));
                }
            },
            None => {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::BackendUnavailable,
                    "CUA untracked continuation omitted scope anchor evidence",
                ));
            }
        }
    }
    let anchor = snapshot
        .get("scope_anchor")
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "CUA ancestor-scoped snapshot omitted scope_anchor evidence",
            )
        })?;
    let scope = snapshot.get("scope").and_then(Value::as_str);
    let evidence_matches = match expected {
        AncestorScopeExpectation::Initial {
            requested_ref,
            role,
            ..
        } => {
            scope == Some("ancestor_subtree")
                && anchor.get("requested_ref").and_then(Value::as_str)
                    == Some(requested_ref.as_str())
                && anchor.get("role").and_then(Value::as_str) == Some(role.as_str())
                && matches!(
                    anchor.get("frame").and_then(Value::as_str),
                    Some("main" | "iframe" | "oopif")
                )
                && anchor
                    .get("distance")
                    .and_then(Value::as_u64)
                    .is_some_and(|distance| distance > 0)
        }
        AncestorScopeExpectation::Continuation(pending) => {
            scope == Some("continuation")
                && snapshot_id == pending.snapshot_id
                && anchor == &pending.anchor
        }
        AncestorScopeExpectation::UntrackedContinuation { .. } => unreachable!("handled above"),
    };
    if !evidence_matches {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "CUA ancestor-scoped snapshot returned inconsistent scope evidence",
        ));
    }
    let continuation = match snapshot.get("continuation") {
        None | Some(Value::Null) => None,
        Some(Value::String(token)) if !token.is_empty() => Some(token.clone()),
        Some(_) => {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "CUA ancestor-scoped snapshot returned an invalid continuation token",
            ));
        }
    };
    if matches!(
        expected,
        AncestorScopeExpectation::Continuation(pending)
            if continuation.as_deref() == Some(pending.token.as_str())
    ) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "CUA ancestor-scoped continuation token was not rotated",
        ));
    }
    if continuation.is_some() && snapshot.get("complete").and_then(Value::as_bool) != Some(false) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "CUA ancestor-scoped snapshot returned a continuation for a complete page",
        ));
    }
    Ok(Some(ValidatedAncestorScope {
        snapshot_id: snapshot_id.to_owned(),
        anchor: anchor.clone(),
        continuation,
    }))
}

fn browser_result_is_structured_refusal(value: &Value) -> ComputerUseResult<bool> {
    let structured = structured_content(value)?;
    Ok(structured["status"] == "refused" || !structured["refusal"].is_null())
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
    validate_absolute_local_path(raw).map_err(|_| {
        invalid(format!(
            "{field} must be a bounded absolute path without NUL bytes"
        ))
    })?;
    Ok(())
}

fn invalid(message: impl Into<String>) -> ComputerUseError {
    ComputerUseError::new(ComputerUseErrorCode::InvalidAction, message)
}

#[cfg(test)]
mod tests;
