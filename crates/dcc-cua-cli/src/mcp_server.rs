use std::collections::BTreeMap;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use dcc_cua_client::{HostClient, LogicalTaskSession, SnapshotTransport};
use dcc_cua_core::{
    ComputerUseDriver, ComputerUseOwnedBrowserLaunchSpec, ConfiguredDriverOptions,
    DriverAuthorizationAction, DriverAuthorizationDecision, DriverAuthorizationHost,
    DriverAuthorizationHostError, DriverAuthorizationRequest, RuntimeAuthorizationOptions,
    SessionPermissionMode,
};
use dcc_cua_host::{
    HostSecurityServices, TrustedTaskActionScope, TrustedTaskAuthorizationHost,
    TrustedTaskAuthorizationIssuer, TrustedTaskAuthorizationReceipt,
    TrustedTaskAuthorizationRegistration, TrustedTaskAuthorizationTarget,
    process_connection_with_security_services,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use uuid::Uuid;

use super::authorization_integration;
use super::trusted_embedding::verify_trusted_embedding_parent;

const SERVER_NAME: &str = "dcc-cua-task-authorization";
const AUTHORIZATION_CARD_URI: &str = "ui://dcc-cua/task-authorization-v1.html";
const AUTHORIZATION_CARD_MIME: &str = "text/html;profile=mcp-app";
const MAX_PENDING_TASKS: usize = 64;
const MAX_TTL_MINUTES: u64 = 24 * 60;
const DEFAULT_TTL_MINUTES: u64 = 60;
const DEFAULT_IDLE_TIMEOUT_MS: u64 = 15 * 60 * 1_000;
const MAX_ALLOWED_METHODS: usize = 32;
const AUTHORIZATION_CARD_HTML: &str = include_str!("task_authorization_card.html");
const CUA_RUNTIME_SESSION_PREFIX: &str = "__cua_runtime_";
const CLIENT_MANAGED_CONFIRMATION: &str = "client_managed";

struct TaskBrowserPrepareAuthorizationHost {
    expected_public_session: String,
    bound_driver_session: Mutex<Option<BoundDriverSession>>,
}

struct BoundDriverSession {
    public_session: String,
    transport_session: String,
}

impl TaskBrowserPrepareAuthorizationHost {
    fn new(expected_public_session: impl Into<String>) -> Self {
        Self {
            expected_public_session: expected_public_session.into(),
            bound_driver_session: Mutex::new(None),
        }
    }

    fn binds_driver_session(&self, request: &DriverAuthorizationRequest) -> bool {
        // Each authorized task owns a fresh driver and an in-memory Host
        // connection that is not exposed to MCP callers. The Host replaces the
        // logical task id with an opaque runtime window id before CUA sees it,
        // so bind the first structurally valid driver request and require that
        // exact public/transport pair for the rest of this task.
        let Ok(mut binding) = self.bound_driver_session.lock() else {
            return false;
        };
        match binding.as_ref() {
            Some(binding) => {
                binding.public_session == request.public_session
                    && binding.transport_session == request.transport_session
            }
            None => {
                *binding = Some(BoundDriverSession {
                    public_session: request.public_session.clone(),
                    transport_session: request.transport_session.clone(),
                });
                true
            }
        }
    }
}

fn split_runtime_session(observed: &str) -> Option<(&str, &str)> {
    let namespaced = observed.strip_prefix(CUA_RUNTIME_SESSION_PREFIX)?;
    let (runtime_scope, public_session) = namespaced.split_once(':')?;
    (runtime_scope.len() == 32
        && runtime_scope
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then_some((runtime_scope, public_session))
}

fn matches_task_session(observed: &str, expected: &str) -> bool {
    observed == expected
        || split_runtime_session(observed)
            .is_some_and(|(_, public_session)| public_session == expected)
}

fn is_runtime_window_session(observed: &str) -> bool {
    let Some((_, public_session)) = split_runtime_session(observed) else {
        return false;
    };
    public_session
        .strip_prefix("dcc-cua-window-")
        .and_then(|value| Uuid::parse_str(value).ok())
        .is_some_and(|id| id.get_version_num() == 4)
}

#[async_trait]
impl DriverAuthorizationHost for TaskBrowserPrepareAuthorizationHost {
    async fn authorize(
        &self,
        request: DriverAuthorizationRequest,
    ) -> Result<DriverAuthorizationDecision, DriverAuthorizationHostError> {
        let allowed = request.schema == "cua-driver-authorization-request-v1"
            && request.permission_mode == "standard"
            && request.adapter_id == "browser_prepare.existing_profile"
            && request.risk_class == "r2"
            && !request.transport_session.is_empty()
            && (matches_task_session(&request.public_session, &self.expected_public_session)
                || is_runtime_window_session(&request.public_session))
            && self.binds_driver_session(&request);
        Ok(DriverAuthorizationDecision {
            action: if allowed {
                DriverAuthorizationAction::Allow
            } else {
                DriverAuthorizationAction::Deny
            },
            request_digest: request.request_digest,
        })
    }
}

fn task_session_id(proposal_id: &str) -> String {
    format!(
        "mcp-task-{}",
        proposal_id.trim_start_matches("task-proposal-")
    )
}

fn browser_prepare_authorization_host(
    proposal_id: &str,
    proposal: &TaskProposal,
) -> Option<TaskBrowserPrepareAuthorizationHost> {
    proposal
        .authorizes_existing_profile_prepare()
        .then(|| TaskBrowserPrepareAuthorizationHost::new(task_session_id(proposal_id)))
}

fn driver_with_browser_prepare_authorization(
    authorization_host: TaskBrowserPrepareAuthorizationHost,
) -> Result<ComputerUseDriver, String> {
    ComputerUseDriver::create_with_authorization_host(
        ConfiguredDriverOptions {
            claude_code_compatibility: false,
            authorization: RuntimeAuthorizationOptions {
                allowed_modes: vec![SessionPermissionMode::Standard],
                compatibility_mode: SessionPermissionMode::Standard,
                compatibility_bounded_manifest_path: None,
                compatibility_capability_manifest_path: None,
                unrestricted_acknowledged: false,
                max_session_ttl_seconds: 8 * 60 * 60,
                max_idle_ttl_seconds: 30 * 60,
            },
        },
        Arc::new(authorization_host),
    )
    .map_err(|error| error.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareTaskInput {
    application_label: String,
    #[serde(default)]
    target_process_id: Option<u32>,
    #[serde(default)]
    target_window_handle: Option<u64>,
    #[serde(default)]
    owned_browser_launch: Option<ComputerUseOwnedBrowserLaunchSpec>,
    surface: TaskSurface,
    allowed_methods: Vec<String>,
    allowed_actions: Vec<TrustedTaskActionScope>,
    #[serde(default)]
    allowed_browser_origins: Vec<String>,
    #[serde(default = "default_ttl_minutes")]
    ttl_minutes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizeTaskInput {
    proposal_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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
    surface: TaskSurface,
    allowed_methods: Vec<String>,
    registration: TrustedTaskAuthorizationRegistration,
    receipt: Option<TrustedTaskAuthorizationReceipt>,
    session: Option<LogicalTaskSession>,
    revoked: bool,
}

impl TaskProposal {
    fn authorizes_existing_profile_prepare(&self) -> bool {
        self.receipt.is_some()
            && self.surface == TaskSurface::Browser
            && matches!(
                self.registration.target,
                TrustedTaskAuthorizationTarget::ExactWindow { .. }
            )
            && self
                .allowed_methods
                .iter()
                .any(|method| method == "browser_prepare")
    }
}

struct TaskAuthorizationAuthority {
    issuer: TrustedTaskAuthorizationIssuer,
    authorization_host: std::sync::Arc<dyn TrustedTaskAuthorizationHost>,
}

struct TaskAuthorizationServer {
    authority: TaskAuthorizationAuthority,
    parent_identity_available: bool,
    proposals: BTreeMap<String, TaskProposal>,
}

impl TaskAuthorizationServer {
    fn new(parent_identity_available: bool) -> Self {
        let (issuer, authorization_host) = dcc_cua_host::trusted_task_authorization_broker();
        Self {
            authority: TaskAuthorizationAuthority {
                issuer,
                authorization_host,
            },
            parent_identity_available,
            proposals: BTreeMap::new(),
        }
    }

    fn integration_status(&self) -> Value {
        let mut status = authorization_integration::status();
        status["parent_identity_available"] = json!(self.parent_identity_available);
        status
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
                    "description": "DCC-CUA exact-task authorization delegated to the connected agent host."
                },
                "instructions": "Prepare one exact bounded task, obtain approval through the connected agent host, call authorize_task with only the retained proposal_id, then start the task. DCC-CUA preserves exact-target, method, action, origin, expiry, and revocation bounds without an operating-system confirmation step."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tool_definitions()})),
            "tools/call" => self.call_tool(params).await,
            "resources/list" => Ok(json!({"resources": [authorization_card_resource()]})),
            "resources/read" => read_resource(&params),
            "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
            "prompts/list" => Ok(json!({"prompts": []})),
            _ => {
                return Some(rpc_error(id, -32601, "Method not found"));
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
            "authorization_integration_status" => Ok(self.integration_status()),
            "prepare_task_authorization" => self.prepare_task(arguments),
            "authorize_task" => self.authorize_task(arguments),
            "start_authorized_task" => self.start_task(arguments).await,
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
        let allowed_browser_origins = input
            .allowed_browser_origins
            .iter()
            .cloned()
            .chain(
                input
                    .allowed_actions
                    .iter()
                    .filter_map(|action| action.browser_origin.clone()),
            )
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let target = match (
            input.target_process_id,
            input.target_window_handle,
            input.owned_browser_launch,
        ) {
            (Some(process_id), Some(window_handle), None) => {
                TrustedTaskAuthorizationTarget::ExactWindow {
                    process_id,
                    window_handle,
                }
            }
            (None, None, Some(launch)) if input.surface == TaskSurface::Browser => {
                if input
                    .allowed_methods
                    .iter()
                    .any(|method| method == "browser_prepare")
                {
                    return Err(
                        "owned browser tasks derive their target internally and cannot grant browser_prepare"
                            .into(),
                    );
                }
                if allowed_browser_origins.is_empty() {
                    return Err(
                        "owned browser tasks require at least one exact authorized browser origin"
                            .into(),
                    );
                }
                TrustedTaskAuthorizationTarget::OwnedBrowser(launch)
            }
            _ => {
                return Err(
                    "provide either an exact target_process_id/target_window_handle pair or owned_browser_launch"
                        .into(),
                );
            }
        };
        let now = unix_time_millis();
        self.proposals.retain(|_, proposal| {
            proposal.session.is_some() || proposal.registration.expires_at_unix_ms > now
        });
        if self.proposals.len() >= MAX_PENDING_TASKS {
            return Err("too many live task authorization proposals".into());
        }
        let proposal_id = format!("task-proposal-{}", Uuid::new_v4());
        let registration = TrustedTaskAuthorizationRegistration {
            connection_id: None,
            task_id: None,
            task_grant_id: format!("task-grant-{}", Uuid::new_v4()),
            application_label: input.application_label,
            target,
            allowed_host_methods: input.allowed_methods.clone(),
            allowed_actions: input.allowed_actions,
            allowed_browser_origins,
            browser_scope: None,
            expires_at_unix_ms: now.saturating_add(input.ttl_minutes * 60_000),
        };
        registration.validate().map_err(|error| error.to_string())?;
        let proposal = TaskProposal {
            surface: input.surface,
            allowed_methods: input.allowed_methods,
            registration,
            receipt: None,
            session: None,
            revoked: false,
        };
        let payload = proposal_payload(&proposal_id, &proposal, "awaiting_client_authorization");
        self.proposals.insert(proposal_id, proposal);
        Ok(payload)
    }

    fn authorize_task(&mut self, arguments: Value) -> Result<Value, String> {
        if arguments.get("acknowledgement").is_some() {
            return Err(
                "authorize_task accepts only a server-generated proposal_id; client approval state is not passed as text"
                    .into(),
            );
        }
        let input: AuthorizeTaskInput = serde_json::from_value(arguments)
            .map_err(|error| format!("invalid task authorization decision: {error}"))?;
        if input.proposal_id.trim().is_empty() {
            return Err("proposal_id must be a non-empty string".into());
        }
        let proposal = self
            .proposals
            .get(&input.proposal_id)
            .ok_or_else(|| "task authorization proposal was not found".to_owned())?;
        if proposal.revoked {
            return Err("task authorization proposal was revoked".into());
        }
        if proposal.registration.expires_at_unix_ms <= unix_time_millis() {
            return Err("task authorization proposal expired".into());
        }
        if proposal.receipt.is_some() {
            return Ok(proposal_payload(&input.proposal_id, proposal, "authorized"));
        }
        let proposal = self
            .proposals
            .get_mut(&input.proposal_id)
            .ok_or_else(|| "task authorization proposal was not found".to_owned())?;
        if proposal.revoked {
            return Err("task authorization proposal was revoked".into());
        }
        if proposal.registration.expires_at_unix_ms <= unix_time_millis() {
            return Err("task authorization proposal expired".into());
        }
        if proposal.receipt.is_none() {
            proposal.receipt = Some(
                self.authority
                    .issuer
                    .register(proposal.registration.clone())
                    .map_err(|error| error.to_string())?,
            );
        }
        Ok(proposal_payload(&input.proposal_id, proposal, "authorized"))
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
        self.authority
            .issuer
            .revoke(&receipt.authorization_id)
            .map_err(|error| error.to_string())?;
        proposal.revoked = true;
        Ok(proposal_payload(proposal_id, proposal, "revoked"))
    }

    async fn start_task(&mut self, arguments: Value) -> Result<Value, String> {
        let proposal_id = required_string(&arguments, "proposal_id")?.to_owned();
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
            return Err("task requires client authorization".into());
        }
        if proposal.session.is_none() {
            proposal.session = Some(
                open_task_session(
                    &proposal_id,
                    proposal,
                    self.authority.authorization_host.clone(),
                )
                .await?,
            );
        }
        let target = proposal
            .session
            .as_ref()
            .expect("task session was initialized")
            .target()
            .clone();
        Ok(json!({
            "ok": true,
            "provider": "dcc-cua",
            "runtime_version": env!("CARGO_PKG_VERSION"),
            "proposal_id": proposal_id,
            "status": "started",
            "target": target,
            "report_before_first_observation_or_input": true,
            "native_action_popups": false,
        }))
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
        } else if proposal.session.is_some() {
            "started"
        } else if proposal.receipt.is_some() {
            "authorized"
        } else {
            "awaiting_client_authorization"
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
            return Err("task requires client authorization".into());
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
            return Err(
                "call start_authorized_task and report provider/runtime/PID/HWND before the first observation or input"
                    .into(),
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
            "target": proposal.session.as_ref().map(LogicalTaskSession::target),
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
    let driver = match browser_prepare_authorization_host(proposal_id, proposal) {
        Some(host) => driver_with_browser_prepare_authorization(host)?,
        None => ComputerUseDriver::create().map_err(|error| error.to_string())?,
    };
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
    let grant = task_session_grant(proposal, receipt);
    client
        .open_logical_task_session(task_session_id(proposal_id), grant, DEFAULT_IDLE_TIMEOUT_MS)
        .await
        .map_err(|error| error.to_string())
}

fn task_session_grant(proposal: &TaskProposal, receipt: &TrustedTaskAuthorizationReceipt) -> Value {
    let browser = matches!(proposal.surface, TaskSurface::Browser);
    let allow_raw_input = proposal
        .registration
        .allowed_actions
        .iter()
        .any(|scope| scope.input_kind == "raw_input");
    let allow_clipboard = proposal
        .registration
        .allowed_actions
        .iter()
        .any(|scope| scope.input_kind == "clipboard");
    let allowed_browser_origins = &proposal.registration.allowed_browser_origins;
    let (process_id, window_handle, owned_browser_launch) = match proposal.registration.target {
        TrustedTaskAuthorizationTarget::ExactWindow {
            process_id,
            window_handle,
        } => (Some(process_id), Some(window_handle), None),
        TrustedTaskAuthorizationTarget::OwnedBrowser(launch) => (None, None, Some(launch)),
    };
    json!({
        "task_grant_id": proposal.registration.task_grant_id,
        "application_label": proposal.registration.application_label,
        "process_id": process_id,
        "window_handle": window_handle,
        "owned_browser_launch": owned_browser_launch,
        "allowed_browser_origins": allowed_browser_origins,
        "allow_raw_input": allow_raw_input,
        "allow_clipboard_read": allow_clipboard,
        "allow_clipboard_write": allow_clipboard,
        "allow_live_observation": true,
        "allow_browser_input": browser,
        "allow_browser_prepare": proposal.authorizes_existing_profile_prepare(),
        "allow_trusted_confirmation": true,
        "task_authorization_id": receipt.authorization_id,
        "task_authorization_window_capability": receipt.window_capability,
    })
}

fn proposal_payload(proposal_id: &str, proposal: &TaskProposal, status: &str) -> Value {
    let target = match proposal.registration.target {
        TrustedTaskAuthorizationTarget::ExactWindow {
            process_id,
            window_handle,
        } => json!({
            "kind": "exact_window",
            "process_id": process_id,
            "window_handle": window_handle,
        }),
        TrustedTaskAuthorizationTarget::OwnedBrowser(launch) => json!({
            "kind": "owned_browser",
            "browser": launch.browser,
            "profile": launch.profile,
            "process_id": Value::Null,
            "window_handle": Value::Null,
            "derived_after_authorization": true,
        }),
    };
    json!({
        "ok": true,
        "provider": "dcc-cua",
        "runtime_version": env!("CARGO_PKG_VERSION"),
        "confirmation_method": CLIENT_MANAGED_CONFIRMATION,
        "confirmation_digest_sha256": confirmation_digest(proposal_id, proposal),
        "proposal_id": proposal_id,
        "status": status,
        "application_label": proposal.registration.application_label,
        "surface": proposal.surface.as_str(),
        "target": target,
        "allowed_methods": proposal.allowed_methods,
        "allowed_actions": proposal.registration.allowed_actions,
        "allowed_browser_origins": proposal.registration.allowed_browser_origins,
        "expires_at_unix_ms": proposal.registration.expires_at_unix_ms,
        "requires_user_text_input": false,
        "requires_system_user_verification": false,
        "native_action_popups": false,
        "secrets_accepted": false,
    })
}

fn confirmation_digest(proposal_id: &str, proposal: &TaskProposal) -> String {
    let target = match proposal.registration.target {
        TrustedTaskAuthorizationTarget::ExactWindow {
            process_id,
            window_handle,
        } => json!({
            "kind": "exact_window",
            "process_id": process_id,
            "window_handle": window_handle,
        }),
        TrustedTaskAuthorizationTarget::OwnedBrowser(launch) => json!({
            "kind": "owned_browser",
            "browser": launch.browser,
            "profile": launch.profile,
        }),
    };
    let scope = json!({
        "schema": "dcc-cua.task-authorization-confirmation.v1",
        "provider": "dcc-cua",
        "runtime_version": env!("CARGO_PKG_VERSION"),
        "proposal_id": proposal_id,
        "application_label": proposal.registration.application_label,
        "surface": proposal.surface.as_str(),
        "target": target,
        "allowed_methods": proposal.allowed_methods,
        "allowed_actions": proposal.registration.allowed_actions,
        "allowed_browser_origins": proposal.registration.allowed_browser_origins,
        "expires_at_unix_ms": proposal.registration.expires_at_unix_ms,
    });
    let encoded = serde_json::to_vec(&scope)
        .expect("a retained task-authorization scope is always JSON serializable");
    let digest = Sha256::digest(encoded);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
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
        "description": "Review one exact bounded DCC-CUA task before the connected agent host authorizes it.",
        "mimeType": AUTHORIZATION_CARD_MIME,
        "_meta": authorization_card_resource_meta(),
    })
}

fn authorization_card_resource_meta() -> Value {
    json!({
        "openai/widgetDescription": "Review the exact DCC-CUA task scope, then authorize it through the connected agent host.",
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

fn task_action_scope_schema() -> Value {
    let required = [
        "action",
        "input_kind",
        "secret_input",
        "authorization_category",
    ];
    json!({
        "oneOf": [
            {
                "title": "Semantic exact-window input",
                "type": "object",
                "additionalProperties": false,
                "required": required,
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": TrustedTaskActionScope::NATIVE_ACTIONS,
                        "description": "Final input action, not a Host method name. For example, browser_click resolves to click."
                    },
                    "input_kind": {"const": "semantic"},
                    "secret_input": {"type": "boolean"},
                    "authorization_category": {"type": "string", "enum": TrustedTaskActionScope::SEMANTIC_CATEGORIES},
                    "browser_origin": {"type": "null"}
                }
            },
            {
                "title": "Raw exact-window input",
                "type": "object",
                "additionalProperties": false,
                "required": required,
                "properties": {
                    "action": {"type": "string", "enum": TrustedTaskActionScope::NATIVE_ACTIONS},
                    "input_kind": {"const": "raw_input"},
                    "secret_input": {"type": "boolean"},
                    "authorization_category": {"type": "string", "enum": TrustedTaskActionScope::RAW_INPUT_CATEGORIES},
                    "browser_origin": {"type": "null"}
                }
            },
            {
                "title": "Browser credential input",
                "type": "object",
                "additionalProperties": false,
                "required": ["action", "input_kind", "secret_input", "authorization_category", "browser_origin"],
                "properties": {
                    "action": {"const": "browser_type"},
                    "input_kind": {"const": "browser"},
                    "secret_input": {"const": true},
                    "authorization_category": {"const": "credential"},
                    "browser_origin": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 2048,
                        "pattern": "^https?://[^/?#@]+$"
                    }
                }
            },
            {
                "title": "Clipboard credential capture",
                "type": "object",
                "additionalProperties": false,
                "required": required,
                "properties": {
                    "action": {"const": "clipboard_capture_secret"},
                    "input_kind": {"const": "clipboard"},
                    "secret_input": {"const": true},
                    "authorization_category": {"const": "credential"},
                    "browser_origin": {"type": "null"}
                }
            }
        ]
    })
}

fn tool_definitions() -> Vec<Value> {
    let action_scope = task_action_scope_schema();
    vec![
        authorization_integration::tool(),
        json!({
            "name": "prepare_task_authorization",
            "title": "Prepare DCC-CUA task authorization",
            "description": "Prepare and optionally render one exact bounded task proposal. The connected Agent host owns user approval; never include credentials or secret values.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["application_label", "surface", "allowed_methods", "allowed_actions"],
                "oneOf": [
                    {
                        "required": ["target_process_id", "target_window_handle"],
                        "not": {"required": ["owned_browser_launch"]}
                    },
                    {
                        "required": ["owned_browser_launch"],
                        "not": {"anyOf": [
                            {"required": ["target_process_id"]},
                            {"required": ["target_window_handle"]}
                        ]}
                    }
                ],
                "properties": {
                    "application_label": {"type": "string", "minLength": 1, "maxLength": 80},
                    "target_process_id": {"type": "integer", "minimum": 1},
                    "target_window_handle": {"type": "integer", "minimum": 1},
                    "owned_browser_launch": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["browser", "profile"],
                        "properties": {
                            "browser": {"type": "string", "enum": ["chromium"]},
                            "profile": {"type": "string", "enum": ["isolated_new"]}
                        }
                    },
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
                    "allowed_actions": {
                        "type": "array",
                        "description": "Closed final input scopes. Use click/type action names for browser_click/browser input methods; only secret-handle browser typing uses browser_type.",
                        "minItems": 1,
                        "maxItems": 32,
                        "uniqueItems": true,
                        "items": action_scope
                    },
                    "allowed_browser_origins": {
                        "type": "array",
                        "maxItems": 32,
                        "uniqueItems": true,
                        "items": {"type": "string", "format": "uri"}
                    },
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
        agent_host_tool(
            "authorize_task",
            "Authorize DCC-CUA task",
            "Call only after the connected Agent host has approved the exact retained proposal. This issues no wider scope than the server-generated proposal_id.",
            proposal_id_schema(),
            true,
        ),
        agent_host_tool(
            "revoke_task_authorization",
            "Revoke DCC-CUA task",
            "Revoke the exact task immediately from any connected Agent host.",
            proposal_id_schema(),
            false,
        ),
        json!({
            "name": "task_authorization_status",
            "title": "DCC-CUA task authorization status",
            "description": "Read the current state of an exact task proposal without exposing authorization capabilities.",
            "inputSchema": proposal_id_schema(),
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "start_authorized_task",
            "title": "Start authorized DCC-CUA task",
            "description": "After the user authorizes, open the exact task session and return provider, runtime version, PID, and HWND. Report that binding before the first observation or input.",
            "inputSchema": proposal_id_schema(),
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "dcc_cua_task_call",
            "title": "Run authorized DCC-CUA task call",
            "description": "Call one closed Host method after start_authorized_task returned and its provider/runtime/PID/HWND binding was reported. Out-of-scope, expired, revoked, or changed targets fail without falling back to a popup. Never pass credential values; use secret handles.",
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

fn agent_host_tool(
    name: &str,
    title: &str,
    description: &str,
    input_schema: Value,
    destructive: bool,
) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {"readOnlyHint": false, "destructiveHint": destructive, "idempotentHint": true, "openWorldHint": false}
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
    // Identity is a diagnostic only. In particular, unpackaged desktop sidecars
    // must not disappear from discovery when version resources are unavailable.
    let parent_identity_available = verify_trusted_embedding_parent()
        .map(|attestation| !attestation.label().is_empty())
        .unwrap_or(false);
    let mut server = TaskAuthorizationServer::new(parent_identity_available);
    let mut input = BufReader::new(tokio::io::stdin());
    let mut output = BufWriter::new(tokio::io::stdout());
    loop {
        let mut line = Vec::new();
        let count = (&mut input)
            .take(dcc_cua_protocol::MAX_JSON_FRAME_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .await?;
        if count == 0 {
            break;
        }
        if count > dcc_cua_protocol::MAX_JSON_FRAME_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "MCP request exceeds frame limit",
            )
            .into());
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let decoded: Value = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(_) => {
                output
                    .write_all(
                        format!("{}\n", rpc_error(Value::Null, -32700, "Parse error")).as_bytes(),
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
