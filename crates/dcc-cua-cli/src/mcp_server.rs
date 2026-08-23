use std::collections::BTreeMap;
use std::error::Error;
use std::time::{SystemTime, UNIX_EPOCH};

use dcc_cua_client::{HostClient, LogicalTaskSession, SnapshotTransport};
use dcc_cua_core::ComputerUseDriver;
use dcc_cua_host::{
    HostSecurityServices, TrustedTaskActionScope, TrustedTaskAuthorizationHost,
    TrustedTaskAuthorizationIssuer, TrustedTaskAuthorizationReceipt,
    TrustedTaskAuthorizationRegistration, process_connection_with_security_services,
    trusted_task_authorization_broker,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use uuid::Uuid;

use super::trusted_embedding::{TrustedEmbeddingAttestation, verify_trusted_embedding_parent};

const SERVER_NAME: &str = "dcc-cua-task-authorization";
const AUTHORIZATION_CARD_URI: &str = "ui://dcc-cua/task-authorization-v1.html";
const AUTHORIZATION_CARD_MIME: &str = "text/html;profile=mcp-app";
const MAX_PENDING_TASKS: usize = 64;
const MAX_TTL_MINUTES: u64 = 24 * 60;
const DEFAULT_TTL_MINUTES: u64 = 60;
const DEFAULT_IDLE_TIMEOUT_MS: u64 = 15 * 60 * 1_000;
const MAX_ALLOWED_METHODS: usize = 32;
const AUTHORIZATION_CARD_HTML: &str = include_str!("task_authorization_card.html");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareTaskInput {
    application_label: String,
    target_process_id: u32,
    target_window_handle: u64,
    surface: TaskSurface,
    allowed_methods: Vec<String>,
    allowed_actions: Vec<TrustedTaskActionScope>,
    #[serde(default = "default_ttl_minutes")]
    ttl_minutes: u64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TaskSurface {
    Window,
    Browser,
}

impl TaskSurface {
    fn as_str(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Browser => "browser",
        }
    }
}

struct TaskProposal {
    embedding: TrustedEmbeddingAttestation,
    surface: TaskSurface,
    allowed_methods: Vec<String>,
    registration: TrustedTaskAuthorizationRegistration,
    receipt: Option<TrustedTaskAuthorizationReceipt>,
    session: Option<LogicalTaskSession>,
    revoked: bool,
}

struct TaskAuthorizationServer {
    embedding: TrustedEmbeddingAttestation,
    issuer: TrustedTaskAuthorizationIssuer,
    authorization_host: std::sync::Arc<dyn TrustedTaskAuthorizationHost>,
    proposals: BTreeMap<String, TaskProposal>,
}

impl TaskAuthorizationServer {
    fn new(embedding: TrustedEmbeddingAttestation) -> Self {
        let (issuer, authorization_host) = trusted_task_authorization_broker();
        Self {
            embedding,
            issuer,
            authorization_host,
            proposals: BTreeMap::new(),
        }
    }

    async fn handle_rpc(&mut self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return Some(rpc_error(id, -32600, "Invalid Request"));
        };
        if method.starts_with("notifications/") || method == "$/cancelRequest" {
            return None;
        }
        let params = message
            .get("params")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").cloned().unwrap_or_else(|| json!("2024-11-05")),
                "capabilities": {
                    "tools": {"listChanged": false},
                    "resources": {"subscribe": false, "listChanged": false}
                },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "title": "DCC-CUA task authorization",
                    "version": env!("CARGO_PKG_VERSION"),
                    "description": "One explicit in-chat user authorization for a bounded exact-window DCC-CUA task."
                },
                "instructions": "Render the task authorization card before the first mutating task call. The user must type authorization once in the card. Never place credentials or secret values in task proposals or task_call params; use DCC-CUA secret handles."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tool_definitions()})),
            "tools/call" => self.call_tool(params).await,
            "resources/list" => Ok(json!({"resources": [authorization_card_resource()]})),
            "resources/read" => read_resource(&params),
            "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
            "prompts/list" => Ok(json!({"prompts": []})),
            _ => {
                return Some(rpc_error(
                    id,
                    -32601,
                    &format!("Method not found: {method}"),
                ));
            }
        };
        Some(match result {
            Ok(result) => rpc_result(id, result),
            Err(message) => rpc_error(id, -32602, &message),
        })
    }

    async fn call_tool(&mut self, params: Value) -> Result<Value, String> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "tools/call requires a tool name".to_owned())?;
        let arguments = params
            .get("arguments")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        if name == "dcc_cua_task_call" {
            return Ok(match self.task_call(arguments).await {
                Ok(result) => result,
                Err(message) => tool_error(message),
            });
        }
        let result = match name {
            "prepare_task_authorization" => self.prepare_task(arguments),
            "authorize_task" => self.authorize_task(arguments),
            "revoke_task_authorization" => self.revoke_task(arguments),
            "task_authorization_status" => self.task_status(arguments),
            _ => Err(format!("unknown DCC-CUA MCP tool: {name}")),
        };
        Ok(match result {
            Ok(payload) => tool_result(payload, name == "prepare_task_authorization"),
            Err(message) => tool_error(message),
        })
    }

    fn prepare_task(&mut self, arguments: Value) -> Result<Value, String> {
        let input: PrepareTaskInput = serde_json::from_value(arguments)
            .map_err(|error| format!("invalid task authorization proposal: {error}"))?;
        if !(1..=MAX_TTL_MINUTES).contains(&input.ttl_minutes) {
            return Err(format!(
                "ttl_minutes must be between 1 and {MAX_TTL_MINUTES}"
            ));
        }
        validate_allowed_methods(input.surface, &input.allowed_methods)?;
        let now = unix_time_millis();
        self.proposals.retain(|_, proposal| {
            proposal.session.is_some() || proposal.registration.expires_at_unix_ms > now
        });
        if self.proposals.len() >= MAX_PENDING_TASKS {
            return Err("too many live task authorization proposals".into());
        }
        let proposal_id = format!("task-proposal-{}", Uuid::new_v4());
        let registration = TrustedTaskAuthorizationRegistration {
            task_grant_id: format!("task-grant-{}", Uuid::new_v4()),
            application_label: input.application_label,
            target_process_id: input.target_process_id,
            target_window_handle: input.target_window_handle,
            allowed_actions: input.allowed_actions,
            expires_at_unix_ms: now.saturating_add(input.ttl_minutes * 60_000),
        };
        registration.validate().map_err(|error| error.to_string())?;
        let proposal = TaskProposal {
            embedding: self.embedding,
            surface: input.surface,
            allowed_methods: input.allowed_methods,
            registration,
            receipt: None,
            session: None,
            revoked: false,
        };
        let payload = proposal_payload(&proposal_id, &proposal, "awaiting_user_input");
        self.proposals.insert(proposal_id, proposal);
        Ok(payload)
    }

    fn authorize_task(&mut self, arguments: Value) -> Result<Value, String> {
        let proposal_id = required_string(&arguments, "proposal_id")?;
        let acknowledgement = required_string(&arguments, "acknowledgement")?;
        if acknowledgement.trim() != "授权"
            && !acknowledgement.trim().eq_ignore_ascii_case("AUTHORIZE")
        {
            return Err("type 授权 or AUTHORIZE in the task card".into());
        }
        let proposal = self
            .proposals
            .get_mut(proposal_id)
            .ok_or_else(|| "task authorization proposal was not found".to_owned())?;
        if proposal.revoked {
            return Err("task authorization proposal was revoked".into());
        }
        if proposal.registration.expires_at_unix_ms <= unix_time_millis() {
            return Err("task authorization proposal expired".into());
        }
        if proposal.receipt.is_none() {
            proposal.receipt = Some(
                self.issuer
                    .register(proposal.registration.clone())
                    .map_err(|error| error.to_string())?,
            );
        }
        Ok(proposal_payload(proposal_id, proposal, "authorized"))
    }

    fn revoke_task(&mut self, arguments: Value) -> Result<Value, String> {
        let proposal_id = required_string(&arguments, "proposal_id")?;
        let proposal = self
            .proposals
            .get_mut(proposal_id)
            .ok_or_else(|| "task authorization proposal was not found".to_owned())?;
        let receipt = proposal
            .receipt
            .as_ref()
            .ok_or_else(|| "task authorization has not been issued".to_owned())?;
        self.issuer
            .revoke(&receipt.authorization_id)
            .map_err(|error| error.to_string())?;
        proposal.revoked = true;
        Ok(proposal_payload(proposal_id, proposal, "revoked"))
    }

    fn task_status(&self, arguments: Value) -> Result<Value, String> {
        let proposal_id = required_string(&arguments, "proposal_id")?;
        let proposal = self
            .proposals
            .get(proposal_id)
            .ok_or_else(|| "task authorization proposal was not found".to_owned())?;
        let status = if proposal.revoked {
            "revoked"
        } else if proposal.registration.expires_at_unix_ms <= unix_time_millis() {
            "expired"
        } else if proposal.receipt.is_some() {
            "authorized"
        } else {
            "awaiting_user_input"
        };
        Ok(proposal_payload(proposal_id, proposal, status))
    }

    async fn task_call(&mut self, arguments: Value) -> Result<Value, String> {
        let proposal_id = required_string(&arguments, "proposal_id")?.to_owned();
        let method = required_string(&arguments, "method")?.to_owned();
        let params = arguments
            .get("params")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let proposal = self
            .proposals
            .get_mut(&proposal_id)
            .ok_or_else(|| "task authorization proposal was not found".to_owned())?;
        if proposal.revoked {
            return Err("task authorization was revoked".into());
        }
        if proposal.registration.expires_at_unix_ms <= unix_time_millis() {
            return Err("task authorization expired".into());
        }
        if proposal.receipt.is_none() {
            return Err("task requires explicit user input in the authorization card".into());
        }
        if !proposal
            .allowed_methods
            .iter()
            .any(|allowed| allowed == &method)
        {
            return Err(format!(
                "Host method {method:?} is outside the user-authorized method scope"
            ));
        }
        if proposal.session.is_none() {
            proposal.session = Some(
                open_task_session(&proposal_id, proposal, self.authorization_host.clone()).await?,
            );
        }
        let response = proposal
            .session
            .as_mut()
            .expect("task session was initialized")
            .request(method, params)
            .await
            .map_err(|error| error.to_string())?;
        let mut host = response.value;
        host["task_authorization_context"] = json!({
            "provider": "dcc-cua",
            "runtime_version": env!("CARGO_PKG_VERSION"),
            "proposal_id": proposal_id,
            "native_action_popups": false,
        });
        super::mcp_output::call_tool_result(host, response.binary_attachment.as_deref())
    }
}

async fn open_task_session(
    proposal_id: &str,
    proposal: &TaskProposal,
    authorization_host: std::sync::Arc<dyn TrustedTaskAuthorizationHost>,
) -> Result<LogicalTaskSession, String> {
    let receipt = proposal
        .receipt
        .as_ref()
        .ok_or_else(|| "task authorization has not been issued".to_owned())?;
    let (client_stream, host_stream) = tokio::io::duplex(256 * 1024);
    let driver = ComputerUseDriver::create().map_err(|error| error.to_string())?;
    let security_services = HostSecurityServices::default()
        .with_task_authorization_host(authorization_host)
        .with_secret_vault(super::secret_vault::native_secret_vault());
    tokio::spawn(async move {
        let _ =
            process_connection_with_security_services(driver, host_stream, security_services).await;
    });
    let mut client =
        HostClient::from_stream_with_transport(client_stream, SnapshotTransport::BinaryFrame);
    client
        .hello(SERVER_NAME)
        .await
        .map_err(|error| error.to_string())?;
    let browser = matches!(proposal.surface, TaskSurface::Browser);
    let allow_raw_input = proposal
        .registration
        .allowed_actions
        .iter()
        .any(|scope| scope.input_kind == "raw_input");
    let allow_clipboard_read = proposal
        .registration
        .allowed_actions
        .iter()
        .any(|scope| scope.input_kind == "clipboard");
    let grant = json!({
        "task_grant_id": proposal.registration.task_grant_id,
        "application_label": proposal.registration.application_label,
        "process_id": proposal.registration.target_process_id,
        "window_handle": proposal.registration.target_window_handle,
        "allow_raw_input": allow_raw_input,
        "allow_clipboard_read": allow_clipboard_read,
        "allow_live_observation": true,
        "allow_browser_input": browser,
        "allow_browser_prepare": browser,
        "allow_trusted_confirmation": true,
        "task_authorization_id": receipt.authorization_id,
        "task_authorization_window_capability": receipt.window_capability,
    });
    client
        .open_logical_task_session(
            format!(
                "mcp-task-{}",
                proposal_id.trim_start_matches("task-proposal-")
            ),
            grant,
            DEFAULT_IDLE_TIMEOUT_MS,
        )
        .await
        .map_err(|error| error.to_string())
}

fn proposal_payload(proposal_id: &str, proposal: &TaskProposal, status: &str) -> Value {
    json!({
        "ok": true,
        "provider": "dcc-cua",
        "runtime_version": env!("CARGO_PKG_VERSION"),
        "trusted_embedding": proposal.embedding.label(),
        "proposal_id": proposal_id,
        "status": status,
        "application_label": proposal.registration.application_label,
        "surface": proposal.surface.as_str(),
        "target": {
            "process_id": proposal.registration.target_process_id,
            "window_handle": proposal.registration.target_window_handle,
        },
        "allowed_methods": proposal.allowed_methods,
        "allowed_actions": proposal.registration.allowed_actions,
        "expires_at_unix_ms": proposal.registration.expires_at_unix_ms,
        "requires_user_text_input": true,
        "authorization_text": "授权 / AUTHORIZE",
        "native_action_popups": false,
        "secrets_accepted": false,
    })
}

fn method_allowed(surface: TaskSurface, method: &str) -> bool {
    let common = matches!(
        method,
        "get_window_state"
            | "snapshot"
            | "accessibility_snapshot"
            | "verify_state"
            | "find"
            | "wait_for"
            | "execute_action"
            | "get_session_state"
            | "get_input_state"
            | "session_health"
            | "poll_session_events"
            | "clipboard_capture_secret"
    );
    common
        || matches!(
            (surface, method),
            (
                TaskSurface::Browser,
                "browser_snapshot"
                    | "browser_prepare"
                    | "browser_navigate"
                    | "browser_click"
                    | "browser_type"
                    | "browser_pointer"
                    | "browser_set_input_files"
                    | "browser_dialog"
            )
        )
}

fn validate_allowed_methods(surface: TaskSurface, methods: &[String]) -> Result<(), String> {
    if methods.is_empty() || methods.len() > MAX_ALLOWED_METHODS {
        return Err(format!(
            "allowed_methods must contain 1..={MAX_ALLOWED_METHODS} Host methods"
        ));
    }
    let unique = methods.iter().collect::<std::collections::BTreeSet<_>>();
    if unique.len() != methods.len() {
        return Err("allowed_methods must not contain duplicates".into());
    }
    if let Some(method) = methods
        .iter()
        .find(|method| !method_allowed(surface, method))
    {
        return Err(format!(
            "Host method {method:?} is outside the closed {} task bridge",
            surface.as_str()
        ));
    }
    Ok(())
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{field} must be a non-empty string"))
}

fn default_ttl_minutes() -> u64 {
    DEFAULT_TTL_MINUTES
}

fn unix_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn authorization_card_resource() -> Value {
    json!({
        "uri": AUTHORIZATION_CARD_URI,
        "name": "dcc_cua_task_authorization_card",
        "title": "DCC-CUA task authorization",
        "description": "One inline, non-modal user authorization for an exact bounded DCC-CUA task.",
        "mimeType": AUTHORIZATION_CARD_MIME,
        "_meta": authorization_card_resource_meta(),
    })
}

fn authorization_card_resource_meta() -> Value {
    json!({
        "openai/widgetDescription": "Review exact DCC-CUA task scope and type authorization once before the task starts.",
        "openai/widgetPrefersBorder": true,
        "openai/widgetCSP": {
            "connect_domains": [],
            "resource_domains": [],
            "frame_domains": []
        },
        "ui": {
            "prefersBorder": true,
            "csp": {"connectDomains": [], "resourceDomains": [], "frameDomains": []}
        }
    })
}

fn read_resource(params: &Value) -> Result<Value, String> {
    if params.get("uri").and_then(Value::as_str) != Some(AUTHORIZATION_CARD_URI) {
        return Err("unknown DCC-CUA UI resource".into());
    }
    Ok(json!({
        "contents": [{
            "uri": AUTHORIZATION_CARD_URI,
            "mimeType": AUTHORIZATION_CARD_MIME,
            "text": AUTHORIZATION_CARD_HTML,
            "_meta": authorization_card_resource_meta(),
        }]
    }))
}

fn tool_definitions() -> Vec<Value> {
    let action_scope = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["action", "input_kind", "secret_input", "authorization_category"],
        "properties": {
            "action": {"type": "string"},
            "input_kind": {"type": "string", "enum": ["raw_input", "semantic", "browser", "clipboard"]},
            "secret_input": {"type": "boolean"},
            "authorization_category": {"type": "string"},
            "browser_origin": {"type": ["string", "null"]}
        }
    });
    vec![
        json!({
            "name": "prepare_task_authorization",
            "title": "Prepare DCC-CUA task authorization",
            "description": "Render one inline authorization card for a bounded exact PID/HWND task. This does not authorize the task; the user must type in the card. Never include credentials or secret values.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["application_label", "target_process_id", "target_window_handle", "surface", "allowed_methods", "allowed_actions"],
                "properties": {
                    "application_label": {"type": "string", "minLength": 1, "maxLength": 80},
                    "target_process_id": {"type": "integer", "minimum": 1},
                    "target_window_handle": {"type": "integer", "minimum": 1},
                    "surface": {"type": "string", "enum": ["window", "browser"]},
                    "allowed_methods": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_ALLOWED_METHODS,
                        "uniqueItems": true,
                        "items": {"type": "string", "enum": [
                            "get_window_state", "snapshot", "accessibility_snapshot", "verify_state",
                            "find", "wait_for", "execute_action", "get_session_state",
                            "get_input_state", "session_health", "poll_session_events",
                            "clipboard_capture_secret", "browser_snapshot", "browser_prepare",
                            "browser_navigate", "browser_click", "browser_type", "browser_pointer",
                            "browser_set_input_files", "browser_dialog"
                        ]}
                    },
                    "allowed_actions": {"type": "array", "minItems": 1, "maxItems": 32, "items": action_scope},
                    "ttl_minutes": {"type": "integer", "minimum": 1, "maximum": MAX_TTL_MINUTES, "default": DEFAULT_TTL_MINUTES}
                }
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false},
            "_meta": {
                "ui": {"resourceUri": AUTHORIZATION_CARD_URI, "visibility": ["model"]},
                "openai/outputTemplate": AUTHORIZATION_CARD_URI,
                "openai/widgetAccessible": true,
                "openai/toolInvocation/invoking": "Preparing task authorization",
                "openai/toolInvocation/invoked": "Task authorization ready"
            }
        }),
        app_only_tool(
            "authorize_task",
            "Authorize DCC-CUA task",
            "Called only from the authorization card after explicit user text input. Scope comes from the server-side proposal and cannot be widened by this call.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["proposal_id", "acknowledgement"],
                "properties": {
                    "proposal_id": {"type": "string"},
                    "acknowledgement": {"type": "string", "minLength": 1, "maxLength": 32}
                }
            }),
        ),
        app_only_tool(
            "revoke_task_authorization",
            "Revoke DCC-CUA task",
            "Called only from the authorization card to revoke the exact task immediately.",
            proposal_id_schema(),
        ),
        json!({
            "name": "task_authorization_status",
            "title": "DCC-CUA task authorization status",
            "description": "Read the current state of an exact task proposal without exposing authorization capabilities.",
            "inputSchema": proposal_id_schema(),
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "dcc_cua_task_call",
            "title": "Run authorized DCC-CUA task call",
            "description": "Call one closed Host method inside an exact task that the user already authorized in the inline card. Out-of-scope, expired, revoked, or changed targets fail without falling back to a popup. Never pass credential values; use secret handles.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["proposal_id", "method", "params"],
                "properties": {
                    "proposal_id": {"type": "string"},
                    "method": {"type": "string"},
                    "params": {"type": "object"}
                }
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true}
        }),
    ]
}

fn app_only_tool(name: &str, title: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
        "_meta": {
            "ui": {"visibility": ["app"]},
            "openai/visibility": "private"
        }
    })
}

fn proposal_id_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["proposal_id"],
        "properties": {"proposal_id": {"type": "string"}}
    })
}

fn tool_result(payload: Value, render_card: bool) -> Value {
    let mut result = json!({
        "content": [{"type": "text", "text": payload.to_string()}],
        "structuredContent": payload,
        "isError": false,
    });
    if render_card {
        result["_meta"] = json!({
            "ui": {"resourceUri": AUTHORIZATION_CARD_URI},
            "openai/outputTemplate": AUTHORIZATION_CARD_URI,
            "openai/widgetAccessible": true,
        });
    }
    result
}

fn tool_error(message: String) -> Value {
    let payload = json!({"ok": false, "error": message});
    json!({
        "content": [{"type": "text", "text": payload.to_string()}],
        "structuredContent": payload,
        "isError": true,
    })
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

pub async fn run() -> Result<(), Box<dyn Error>> {
    let embedding = verify_trusted_embedding_parent()?;
    let mut server = TaskAuthorizationServer::new(embedding);
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut output = BufWriter::new(tokio::io::stdout());
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let decoded: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                output
                    .write_all(
                        format!(
                            "{}\n",
                            rpc_error(Value::Null, -32700, &format!("Parse error: {error}"))
                        )
                        .as_bytes(),
                    )
                    .await?;
                output.flush().await?;
                continue;
            }
        };
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
        if responses.is_empty() {
            continue;
        }
        let response = if responses.len() == 1 {
            responses.remove(0)
        } else {
            Value::Array(responses)
        };
        output.write_all(format!("{response}\n").as_bytes()).await?;
        output.flush().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
