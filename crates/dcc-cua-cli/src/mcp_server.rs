use std::collections::BTreeMap;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use dcc_cua_client::{HostClient, HostClientError, LogicalTaskSession, SnapshotTransport};
use dcc_cua_core::{
    ComputerUseDriver, ComputerUseOwnedBrowserLaunchSpec, ConfiguredDriverOptions,
    DriverAuthorizationAction, DriverAuthorizationDecision, DriverAuthorizationHost,
    DriverAuthorizationHostError, DriverAuthorizationRequest, RuntimeAuthorizationOptions,
    SessionPermissionMode,
};
use dcc_cua_host::{
    HostSecurityServices, TaskObservationMode, TrustedTaskActionScope,
    TrustedTaskAuthorizationHost, TrustedTaskAuthorizationIssuer, TrustedTaskAuthorizationReceipt,
    TrustedTaskAuthorizationRegistration, TrustedTaskAuthorizationTarget,
    process_connection_with_security_services,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use uuid::Uuid;

use super::connection_diagnostics::{
    AssociatedHost, CloseReason, ConnectionDiagnostics, RESOURCE_URI as CONNECTION_RESOURCE,
    RuntimeBuildIdentity, TaskCounts,
};

const SERVER_NAME: &str = "dcc-cua-task-automation";
const MAX_PENDING_TASKS: usize = 64;
const MAX_TTL_MINUTES: u64 = 24 * 60;
const DEFAULT_TTL_MINUTES: u64 = 60;
const DEFAULT_IDLE_TIMEOUT_MS: u64 = 15 * 60 * 1_000;
const MAX_ALLOWED_METHODS: usize = 32;
const CUA_RUNTIME_SESSION_PREFIX: &str = "__cua_runtime_";

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
    format!("mcp-{proposal_id}")
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
    #[serde(default)]
    observation_mode: TaskObservationMode,
    allowed_methods: Vec<String>,
    allowed_actions: Vec<TrustedTaskActionScope>,
    #[serde(default)]
    allow_recording: bool,
    #[serde(default)]
    allow_capture_preparation: bool,
    #[serde(default)]
    allowed_browser_origins: Vec<String>,
    #[serde(default = "default_ttl_minutes")]
    ttl_minutes: u64,
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
    observation_mode: TaskObservationMode,
    allowed_methods: Vec<String>,
    registration: TrustedTaskAuthorizationRegistration,
    receipt: Option<TrustedTaskAuthorizationReceipt>,
    session: Option<LogicalTaskSession>,
    session_open_attempted: bool,
    revoked: bool,
    cleanup: Option<Value>,
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
    proposals: BTreeMap<String, TaskProposal>,
    recording_output_root: Option<std::path::PathBuf>,
    capture_preparation_journal_root: Option<std::path::PathBuf>,
    diagnostics: Option<ConnectionDiagnostics>,
}

impl TaskAuthorizationServer {
    fn automatic() -> Self {
        let (issuer, authorization_host) = dcc_cua_host::trusted_task_authorization_broker();
        Self {
            authority: TaskAuthorizationAuthority {
                issuer,
                authorization_host,
            },
            proposals: BTreeMap::new(),
            recording_output_root: std::env::var_os("DCC_CUA_RECORDING_OUTPUT_ROOT")
                .map(Into::into),
            capture_preparation_journal_root: std::env::var_os(
                "DCC_CUA_CAPTURE_PREPARATION_JOURNAL_ROOT",
            )
            .map(Into::into),
            diagnostics: None,
        }
    }

    async fn handle_rpc(&mut self, message: Value) -> Option<Value> {
        self.update_diagnostics();
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
        if method == "initialize"
            && let Some(diagnostics) = self.diagnostics.as_mut()
        {
            diagnostics.initialized(&params);
        }
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").cloned().unwrap_or_else(|| json!("2024-11-05")),
                "capabilities": {
                    "tools": {"listChanged": false},
                    "resources": {"subscribe": false, "listChanged": false}
                },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "title": "DCC-CUA task automation",
                    "version": env!("CARGO_PKG_VERSION"),
                    "description": "Start and operate exact bounded DCC-CUA tasks through the connected Agent Host."
                },
                "instructions": "Call start_task once with the exact target and bounded task scope. The connected Agent Host owns user authorization; DCC-CUA does not show a second confirmation card. Report provider/runtime/PID/HWND before the first observation or input, then use dcc_cua_task_call and verify every mutation."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tool_definitions()})),
            "tools/call" => self.call_tool(params).await,
            "resources/list" => Ok(json!({"resources": [{
                "uri": CONNECTION_RESOURCE,
                "name": "dcc-cua-connection",
                "description": "Read-only diagnostics for this MCP connection; client identifiers are self-reported.",
                "mimeType": "application/json"
            }]})),
            "resources/read" => self.read_connection_resource(&params),
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

    fn read_connection_resource(&self, params: &Value) -> Result<Value, String> {
        if params.get("uri").and_then(Value::as_str) != Some(CONNECTION_RESOURCE) {
            return Err("unknown DCC-CUA MCP resource".into());
        }
        let diagnostics = self
            .diagnostics
            .as_ref()
            .ok_or_else(|| "connection diagnostics unavailable".to_owned())?;
        Ok(json!({"contents": [{
            "uri": CONNECTION_RESOURCE,
            "mimeType": "application/json",
            "text": serde_json::to_string(&diagnostics.snapshot()).map_err(|_| "connection diagnostics unavailable")?
        }]}))
    }

    fn update_diagnostics(&mut self) {
        let Some(diagnostics) = self.diagnostics.as_mut() else {
            return;
        };
        let mut counts = TaskCounts::default();
        let mut hosts = Vec::new();
        let now = unix_time_millis();
        for proposal in self.proposals.values() {
            if proposal.revoked {
                counts.stopped += 1;
            } else if proposal.registration.expires_at_unix_ms <= now {
                counts.expired += 1;
            } else if proposal.session.is_some() {
                counts.active += 1;
            } else {
                counts.pending += 1;
            }
            if let Some(session) = proposal.session.as_ref() {
                hosts.push(AssociatedHost::embedded(
                    session.connection_id().map(str::to_owned),
                ));
            }
        }
        diagnostics.set_tasks(counts, hosts);
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
            "start_task" => {
                return Ok(match self.start_automatic_task(arguments).await {
                    Ok(payload) => tool_result(payload),
                    Err(message) => tool_error(message),
                });
            }
            "stop_task" => self.revoke_task(arguments).await,
            "task_status" => self.task_status(arguments),
            _ => Err(format!("unknown DCC-CUA MCP tool: {name}")),
        };
        Ok(match result {
            Ok(payload) => tool_result(payload),
            Err(message) => tool_error(message),
        })
    }

    fn prepare_task(&mut self, arguments: Value) -> Result<Value, String> {
        let authority = &self.authority;
        let input: PrepareTaskInput = serde_json::from_value(arguments)
            .map_err(|error| format!("invalid task request: {error}"))?;
        if !(1..=MAX_TTL_MINUTES).contains(&input.ttl_minutes) {
            return Err(format!(
                "ttl_minutes must be between 1 and {MAX_TTL_MINUTES}"
            ));
        }
        validate_allowed_methods(input.surface, &input.allowed_methods)?;
        validate_native_lifecycle_methods(&input.allowed_methods)?;
        let preparation_requested = input
            .allowed_methods
            .iter()
            .any(|method| method.starts_with("capture_preparation_"));
        let preparation_action = input
            .allowed_actions
            .iter()
            .any(TrustedTaskActionScope::is_capture_preparation);
        if input.allow_capture_preparation != preparation_requested
            || input.allow_capture_preparation != preparation_action
            || (input.allow_capture_preparation
                && (input.surface != TaskSurface::Window
                    || input.observation_mode != TaskObservationMode::PixelsOnly
                    || ![
                        "get_window_state",
                        "capture_preparation_begin",
                        "capture_preparation_state",
                        "capture_preparation_stop",
                    ]
                    .iter()
                    .all(|required| {
                        input
                            .allowed_methods
                            .iter()
                            .any(|method| method == required)
                    })))
        {
            return Err("capture preparation requires explicit allow_capture_preparation=true, pixels_only exact window, get_window_state, complete begin/state/stop and its closed action scope".into());
        }
        let requested_recording = input
            .allowed_methods
            .iter()
            .any(|method| method.starts_with("recording_"));
        if input.allow_recording != requested_recording
            || (input.allow_recording
                && (input.observation_mode != TaskObservationMode::PixelsOnly
                    || input.surface != TaskSurface::Window
                    || !["recording_start", "recording_state", "recording_stop"]
                        .iter()
                        .all(|required| {
                            input
                                .allowed_methods
                                .iter()
                                .any(|method| method == required)
                        })))
        {
            return Err("recording methods require explicit allow_recording=true on a pixels_only window task".into());
        }
        if input.observation_mode == TaskObservationMode::PixelsOnly {
            if input.surface != TaskSurface::Window
                || !matches!(input.target_process_id, Some(pid) if pid != 0)
                || !matches!(input.target_window_handle, Some(hwnd) if hwnd != 0)
                || input.owned_browser_launch.is_some()
                || !input.allowed_browser_origins.is_empty()
            {
                return Err(
                    "pixels_only requires the window surface with an exact PID/HWND".into(),
                );
            }
            if input
                .allowed_methods
                .iter()
                .any(|method| !input.observation_mode.permits_method(method))
                || input.allowed_actions.iter().any(|action| {
                    !action.is_window_minimize()
                        && !action.is_window_frame()
                        && !action.is_capture_preparation()
                        && !action.is_pixels_input()
                })
            {
                return Err(
                    "pixels_only permits only closed native window_state or covered raw_input scopes, without semantic/browser methods or secrets"
                        .into(),
                );
            }
            if input
                .allowed_methods
                .iter()
                .any(|method| method == "execute_action")
                && !input
                    .allowed_actions
                    .iter()
                    .any(TrustedTaskActionScope::is_pixels_input)
            {
                return Err("pixels_only execute_action requires an actual supported raw_input action scope".into());
            }
        }
        if input
            .allowed_methods
            .iter()
            .any(|method| method == "set_window_frame")
            && (input.observation_mode != TaskObservationMode::PixelsOnly
                || !input
                    .allowed_actions
                    .iter()
                    .any(TrustedTaskActionScope::is_window_frame)
                || !input
                    .allowed_methods
                    .iter()
                    .any(|method| method == "get_window_state"))
        {
            return Err("set_window_frame requires explicit pixels_only, get_window_state, and its closed window_state action scope".into());
        }
        if input
            .allowed_methods
            .iter()
            .any(|method| method == "minimize_window")
            && !input
                .allowed_actions
                .iter()
                .any(TrustedTaskActionScope::is_window_minimize)
        {
            return Err(
                "minimize_window requires the closed window_state/minimize_window action scope"
                    .into(),
            );
        }
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
            return Err("too many live tasks".into());
        }
        let proposal_id = format!("task-{}", Uuid::new_v4());
        let capture_preparation = if input.allow_capture_preparation {
            let root = self.capture_preparation_journal_root.as_ref().ok_or_else(||
                "capture preparation requires operator configuration DCC_CUA_CAPTURE_PREPARATION_JOURNAL_ROOT; callers cannot nominate a journal root".to_owned())?;
            let directory = root
                .to_str()
                .ok_or_else(|| "capture preparation journal root must be Unicode".to_owned())?;
            TrustedTaskAuthorizationRegistration::validate_capture_preparation_directory(directory)
                .map_err(|error| error.to_string())?;
            if self
                .recording_output_root
                .as_ref()
                .is_some_and(|recording| {
                    recording == root
                        || std::fs::canonicalize(recording)
                            .ok()
                            .is_some_and(|canonical| {
                                std::fs::canonicalize(root).ok().as_ref() == Some(&canonical)
                            })
                })
            {
                return Err(
                    "capture preparation requires a journal root separate from recording output"
                        .into(),
                );
            }
            Some(
                dcc_cua_protocol::capture_preparation::CapturePreparationAuthorization {
                    journal_directory: directory.to_owned(),
                    max_lifetime_ms:
                        dcc_cua_protocol::capture_preparation::MAX_PREPARATION_LIFETIME_MS,
                },
            )
        } else {
            None
        };
        let mut registration = TrustedTaskAuthorizationRegistration {
            connection_id: None,
            task_id: None,
            task_grant_id: format!("task-grant-{}", Uuid::new_v4()),
            application_label: input.application_label,
            target,
            allowed_host_methods: input.allowed_methods.clone(),
            allowed_actions: input.allowed_actions,
            allowed_browser_origins,
            browser_scope: None,
            recording_output_dir: None,
            capture_preparation,
            expires_at_unix_ms: now.saturating_add(input.ttl_minutes * 60_000),
        };
        registration.validate().map_err(|error| error.to_string())?;
        if input.allow_recording {
            let root = self.recording_output_root.as_ref().ok_or_else(||
                "recording requires operator configuration DCC_CUA_RECORDING_OUTPUT_ROOT; callers cannot nominate an output root".to_owned())?;
            let root_text = root
                .to_str()
                .ok_or_else(|| "recording output root must be Unicode".to_owned())?;
            TrustedTaskAuthorizationRegistration::validate_recording_directory(root_text)
                .map_err(|error| error.to_string())?;
            let directory = root.join(&proposal_id);
            std::fs::create_dir(&directory)
                .map_err(|error| format!("create owned recording task directory: {error}"))?;
            registration.recording_output_dir = Some(directory.to_string_lossy().into_owned());
            registration.validate().map_err(|error| error.to_string())?;
        }
        let receipt = authority
            .issuer
            .register(registration.clone())
            .map_err(|error| error.to_string())?;
        let proposal = TaskProposal {
            surface: input.surface,
            observation_mode: input.observation_mode,
            allowed_methods: input.allowed_methods,
            registration,
            receipt: Some(receipt),
            session: None,
            session_open_attempted: false,
            revoked: false,
            cleanup: None,
        };
        let payload = proposal_payload(&proposal_id, &proposal, "ready");
        self.proposals.insert(proposal_id, proposal);
        Ok(payload)
    }

    async fn start_automatic_task(&mut self, arguments: Value) -> Result<Value, String> {
        let prepared = self.prepare_task(arguments)?;
        let proposal_id = prepared["task_id"]
            .as_str()
            .expect("a prepared task always has an id")
            .to_owned();
        match self.start_task(json!({"task_id": proposal_id})).await {
            Ok(started) => Ok(started),
            Err(error) => {
                let cleanup = self.revoke_task(json!({"task_id":proposal_id})).await;
                match cleanup {
                    Ok(cleanup) if cleanup["status"] == "stopped" => Err(error),
                    Ok(cleanup) => Err(format!("{error}; startup cleanup: {cleanup}")),
                    Err(cleanup) => Err(format!("{error}; startup cleanup failed: {cleanup}")),
                }
            }
        }
    }

    async fn revoke_task(&mut self, arguments: Value) -> Result<Value, String> {
        let authority = &self.authority;
        let proposal_id = required_string(&arguments, "task_id")?;
        let proposal = self
            .proposals
            .get_mut(proposal_id)
            .ok_or_else(|| "task was not found".to_owned())?;
        if proposal.revoked {
            return Ok(proposal_payload(
                proposal_id,
                proposal,
                stopped_task_status(proposal.cleanup.as_ref()),
            ));
        }
        let receipt = proposal
            .receipt
            .as_ref()
            .ok_or_else(|| "task runtime lease was not issued".to_owned())?;
        authority
            .issuer
            .revoke(&receipt.authorization_id)
            .map_err(|error| error.to_string())?;
        proposal.revoked = true;
        proposal.cleanup = Some(match proposal.session.take() {
            Some(session) => stop_owned_task_session(session).await,
            None if !proposal.session_open_attempted => {
                json!({"success":true,"active":false,"cleanup_pending":false,"cleanup_issues":[]})
            }
            None => json!({"success":false,"active":null,"cleanup_pending":true,
                "error":"task session startup did not return an owned session cleanup acknowledgement"}),
        });
        Ok(proposal_payload(
            proposal_id,
            proposal,
            stopped_task_status(proposal.cleanup.as_ref()),
        ))
    }

    async fn shutdown_tasks(&mut self) -> Vec<Value> {
        let ids = self.proposals.keys().cloned().collect::<Vec<_>>();
        let mut failures = Vec::new();
        for id in ids {
            match self.revoke_task(json!({"task_id":id})).await {
                Ok(result) if result["status"] == "stopped" => {}
                Ok(result) => failures.push(result),
                Err(error) => {
                    // Even a lease-service failure must not silently drop an owned recorder.
                    let cleanup = match self
                        .proposals
                        .get_mut(&id)
                        .and_then(|task| task.session.take())
                    {
                        Some(session) => stop_owned_task_session(session).await,
                        None => json!({"active":false,"cleanup_pending":false}),
                    };
                    failures.push(json!({"task_id":id,"error":error,"cleanup":cleanup}));
                }
            }
        }
        failures
    }

    async fn start_task(&mut self, arguments: Value) -> Result<Value, String> {
        let authority = &self.authority;
        let proposal_id = required_string(&arguments, "task_id")?.to_owned();
        let proposal = self
            .proposals
            .get_mut(&proposal_id)
            .ok_or_else(|| "task was not found".to_owned())?;
        if proposal.revoked {
            return Err("task was stopped".into());
        }
        if proposal.registration.expires_at_unix_ms <= unix_time_millis() {
            return Err("task expired".into());
        }
        if proposal.receipt.is_none() {
            return Err("task runtime lease was not issued".into());
        }
        if proposal.session.is_none() {
            proposal.session_open_attempted = true;
            proposal.session = Some(
                open_task_session(&proposal_id, proposal, authority.authorization_host.clone())
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
            "task_id": proposal_id,
            "status": "started",
            "target": target,
            "report_before_first_observation_or_input": true,
            "native_action_popups": false,
            "recording_output_dir": proposal.registration.recording_output_dir,
        }))
    }

    fn task_status(&self, arguments: Value) -> Result<Value, String> {
        let proposal_id = required_string(&arguments, "task_id")?;
        let proposal = self
            .proposals
            .get(proposal_id)
            .ok_or_else(|| "task was not found".to_owned())?;
        let status = if proposal.revoked {
            stopped_task_status(proposal.cleanup.as_ref())
        } else if proposal.registration.expires_at_unix_ms <= unix_time_millis() {
            "expired"
        } else if proposal.session.is_some() {
            "started"
        } else if proposal.receipt.is_some() {
            "ready"
        } else {
            "initializing"
        };
        Ok(proposal_payload(proposal_id, proposal, status))
    }

    async fn task_call(&mut self, arguments: Value) -> Result<Value, String> {
        let proposal_id = required_string(&arguments, "task_id")?.to_owned();
        let method = required_string(&arguments, "method")?.to_owned();
        let mut params = arguments
            .get("params")
            .filter(|value| value.is_object())
            .cloned()
            .ok_or_else(|| "task call params must be an object".to_owned())?;
        validate_task_method_params(&method, &params)?;
        let proposal = self
            .proposals
            .get_mut(&proposal_id)
            .ok_or_else(|| "task was not found".to_owned())?;
        if proposal.revoked {
            return Err("task was stopped".into());
        }
        if proposal.registration.expires_at_unix_ms <= unix_time_millis() {
            return Err("task expired".into());
        }
        if proposal.receipt.is_none() {
            return Err("task runtime lease was not issued".into());
        }
        if !proposal
            .allowed_methods
            .iter()
            .any(|allowed| allowed == &method)
        {
            return Err(format!(
                "Host method {method:?} is outside the configured task method scope"
            ));
        }
        if proposal.session.is_none() {
            return Err(
                "call start_task and report provider/runtime/PID/HWND before the first observation or input"
                    .into(),
            );
        }
        if method == "recording_start" {
            bind_recording_request(
                &mut params,
                proposal.registration.recording_output_dir.as_deref(),
            )?;
        } else if method == "live_observation_start" && params.get("request").is_none() {
            params["request"] = json!({});
        }
        let response = proposal
            .session
            .as_mut()
            .expect("task session was initialized")
            .request(method.as_str(), params)
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error @ HostClientError::Remote { .. }) => {
                return Ok(task_remote_error(
                    &error,
                    &method,
                    &proposal_id,
                    proposal
                        .session
                        .as_ref()
                        .expect("task session was initialized"),
                ));
            }
            Err(error) => return Err(error.to_string()),
        };
        let mut host = response.value;
        host["task_context"] = json!({
            "provider": "dcc-cua",
            "runtime_version": env!("CARGO_PKG_VERSION"),
            "task_id": proposal_id,
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
        .ok_or_else(|| "task runtime lease was not issued".to_owned())?;
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
    let allow_raw_input = proposal.registration.allowed_actions.iter().any(|scope| {
        scope.input_kind == "raw_input"
            && (proposal.observation_mode == TaskObservationMode::Semantic
                || scope.is_pixels_input())
    });
    let allow_clipboard = proposal
        .registration
        .allowed_actions
        .iter()
        .any(|scope| scope.input_kind == "clipboard");
    let allow_browser_download = proposal
        .allowed_methods
        .iter()
        .any(|method| method == "browser_download");
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
        "observation_mode": proposal.observation_mode,
        "process_id": process_id,
        "window_handle": window_handle,
        "owned_browser_launch": owned_browser_launch,
        "allowed_browser_origins": allowed_browser_origins,
        "allow_raw_input": allow_raw_input,
        "allow_clipboard_read": allow_clipboard,
        "allow_clipboard_write": allow_clipboard,
        "allow_live_observation": true,
        "allow_recording": proposal.registration.recording_output_dir.is_some(),
        "allow_capture_preparation": proposal.registration.capture_preparation.is_some(),
        "recording_output_dir": proposal.registration.recording_output_dir,
        "allow_browser_input": browser,
        "allow_browser_prepare": proposal.authorizes_existing_profile_prepare(),
        "allow_browser_download": allow_browser_download,
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
            "derived_on_start": true,
        }),
    };
    json!({
        "ok": !matches!(status, "cleanup_failed" | "cleanup_unknown"),
        "provider": "dcc-cua",
        "runtime_version": env!("CARGO_PKG_VERSION"),
        "task_id": proposal_id,
        "status": status,
        "application_label": proposal.registration.application_label,
        "surface": proposal.surface.as_str(),
        "observation_mode": proposal.observation_mode,
        "target": target,
        "allowed_methods": proposal.allowed_methods,
        "allowed_actions": proposal.registration.allowed_actions,
        "allowed_browser_origins": proposal.registration.allowed_browser_origins,
        "expires_at_unix_ms": proposal.registration.expires_at_unix_ms,
        "confirmation_required": false,
        "native_action_popups": false,
        "secrets_accepted": false,
        "recording_output_dir": proposal.registration.recording_output_dir,
        "cleanup": proposal.cleanup,
    })
}

fn method_allowed(surface: TaskSurface, method: &str) -> bool {
    let common = matches!(
        method,
        "get_window_state"
            | "change_window_state"
            | "minimize_window"
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
            | "live_observation_start"
            | "live_observation_state"
            | "live_observation_stop"
            | "recording_start"
            | "recording_state"
            | "recording_stop"
    );
    common
        || (surface == TaskSurface::Window
            && matches!(
                method,
                "set_window_frame"
                    | "capture_preparation_begin"
                    | "capture_preparation_state"
                    | "capture_preparation_stop"
                    | "capture_preparation_snapshot"
            ))
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
                    | "browser_download"
                    | "browser_dialog"
            )
        )
}

fn validate_task_method_params(method: &str, params: &Value) -> Result<(), String> {
    if method == "capture_preparation_begin" {
        let object = params
            .as_object()
            .ok_or_else(|| "capture preparation params must be an object".to_owned())?;
        if object.len() != 1 || !object.contains_key("request") {
            return Err("capture preparation begin accepts only its window-state request; journal and target are constructor-owned".into());
        }
        let request: dcc_cua_protocol::capture_preparation::CapturePreparationBeginRequest =
            serde_json::from_value(object["request"].clone())
                .map_err(|error| format!("invalid capture preparation request: {error}"))?;
        request.validate().map_err(|error| error.to_string())?;
    }
    if matches!(
        method,
        "capture_preparation_state" | "capture_preparation_stop" | "capture_preparation_snapshot"
    ) && params.as_object().is_none_or(|params| !params.is_empty())
    {
        return Err("capture preparation state/stop/snapshot accept no caller parameters".into());
    }
    if method == "set_window_frame" {
        validate_native_frame_params(params)?;
    }
    if matches!(
        method,
        "recording_state" | "recording_stop" | "live_observation_state" | "live_observation_stop"
    ) && params.as_object().is_none_or(|params| !params.is_empty())
    {
        return Err("recording/live state and stop accept no caller parameters".into());
    }
    if matches!(method, "recording_start" | "live_observation_start") {
        let object = params
            .as_object()
            .ok_or_else(|| "lifecycle parameters must be an object".to_owned())?;
        if object.keys().any(|key| key != "request") {
            return Err("lifecycle start accepts only its bounded request object".into());
        }
        if let Some(request) = object.get("request") {
            let request = request
                .as_object()
                .ok_or_else(|| "lifecycle request must be an object".to_owned())?;
            if method == "recording_start" {
                if request
                    .keys()
                    .any(|key| !matches!(key.as_str(), "output_dir" | "record_video"))
                    || request
                        .get("record_video")
                        .is_some_and(|value| value != &json!(true))
                    || request
                        .get("output_dir")
                        .is_some_and(|value| !value.is_string())
                {
                    return Err("native recording is video-only and cannot override its authorized output directory".into());
                }
            } else if request
                .keys()
                .any(|key| !matches!(key.as_str(), "fps" | "max_dimension"))
                || request.get("fps").is_some_and(|value| {
                    value
                        .as_u64()
                        .is_none_or(|value| !(1..=30).contains(&value))
                })
                || request.get("max_dimension").is_some_and(|value| {
                    value
                        .as_u64()
                        .is_none_or(|value| !(256..=4096).contains(&value))
                })
            {
                return Err(
                    "live observation requires fps 1..30 and max_dimension 256..4096".into(),
                );
            }
        }
    }
    if let Some(value) = params.get("capture_diagnostics")
        && (method != "snapshot" || !value.is_boolean())
    {
        return Err(
            "capture_diagnostics must be a boolean on an explicit pixels_only snapshot".into(),
        );
    }
    if method == "minimize_window"
        && params
            .get("observation_id")
            .and_then(Value::as_str)
            .is_none_or(|id| id.is_empty())
    {
        return Err("minimize_window requires the latest snapshot observation_id".into());
    }
    if method == "change_window_state"
        && !matches!(
            params.get("operation").and_then(Value::as_str),
            Some("activate" | "restore_activate")
        )
    {
        return Err(
            "change_window_state requires operation activate or restore_activate in the task bridge"
                .into(),
        );
    }
    Ok(())
}

fn validate_native_frame_params(params: &Value) -> Result<(), String> {
    let invalid = || {
        "set_window_frame requires only a fresh window_state_id and exact i32 physical frame"
            .to_owned()
    };
    let object = params.as_object().ok_or_else(invalid)?;
    if object.len() != 2
        || object
            .keys()
            .any(|key| !matches!(key.as_str(), "window_state_id" | "frame"))
        || !object
            .get("window_state_id")
            .and_then(Value::as_str)
            .is_some_and(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && id.trim() == id
                    && !id.chars().any(char::is_control)
            })
    {
        return Err(invalid());
    }
    let frame = object
        .get("frame")
        .and_then(Value::as_object)
        .ok_or_else(invalid)?;
    if frame.len() != 4
        || frame
            .keys()
            .any(|key| !matches!(key.as_str(), "x" | "y" | "width" | "height"))
    {
        return Err(invalid());
    }
    let values = ["x", "y", "width", "height"].map(|key| {
        frame
            .get(key)
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
    });
    let [Some(x), Some(y), Some(width), Some(height)] = values else {
        return Err(invalid());
    };
    if width <= 0
        || height <= 0
        || x.checked_add(width).is_none()
        || y.checked_add(height).is_none()
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate_native_lifecycle_methods(methods: &[String]) -> Result<(), String> {
    if methods
        .iter()
        .any(|method| method.starts_with("capture_preparation_"))
        && ![
            "get_window_state",
            "capture_preparation_begin",
            "capture_preparation_state",
            "capture_preparation_stop",
        ]
        .iter()
        .all(|required| methods.iter().any(|method| method == required))
    {
        return Err(
            "capture preparation requires get_window_state and complete begin/state/stop methods"
                .into(),
        );
    }
    for group in [
        ["recording_start", "recording_state", "recording_stop"],
        [
            "live_observation_start",
            "live_observation_state",
            "live_observation_stop",
        ],
    ] {
        if methods.iter().any(|method| method == group[0])
            && !group
                .iter()
                .all(|required| methods.iter().any(|method| method == required))
        {
            return Err("recording/live start requires its state and stop methods in the same immutable task scope".into());
        }
    }
    Ok(())
}

fn bind_recording_request(params: &mut Value, directory: Option<&str>) -> Result<(), String> {
    let directory =
        directory.ok_or_else(|| "native recording output was not authorized".to_owned())?;
    validate_task_method_params("recording_start", params)?;
    if params
        .get("request")
        .and_then(|request| request.get("output_dir"))
        .is_some_and(|value| value.as_str() != Some(directory))
    {
        return Err(
            "recording output directory does not match the immutable task authorization".into(),
        );
    }
    params["request"] = json!({"output_dir":directory,"record_video":true});
    Ok(())
}

async fn stop_owned_task_session(session: LogicalTaskSession) -> Value {
    let session_id = session.session_id().to_owned();
    let mut client = session.into_client();
    match client
        .request_with_timeout(
            "stop_session",
            json!({"session_id":session_id}),
            std::time::Duration::from_secs(60),
        )
        .await
    {
        Ok(response)
            if response.value["type"] == "session_stopped"
                && response.value["session_id"] == session_id
                && response.value["success"].is_boolean()
                && response.value["active"] == false
                && response.value["cleanup_pending"] == false =>
        {
            response.value
        }
        Ok(response) => json!({"success":false,"active":null,"cleanup_pending":true,
            "error":"Host did not acknowledge authoritative session cleanup",
            "host_response":response.value}),
        Err(error) => json!({"success":false,"active":null,"cleanup_pending":true,
            "error":error.to_string()}),
    }
}

fn stopped_task_status(cleanup: Option<&Value>) -> &'static str {
    match cleanup {
        Some(cleanup) if cleanup["cleanup_pending"] == false && cleanup["active"] == false => {
            if cleanup["success"] == true {
                "stopped"
            } else {
                "cleanup_failed"
            }
        }
        _ => "cleanup_unknown",
    }
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

fn native_minimize_scope_schema() -> Value {
    json!({
        "title": "Observation-bound native window minimize",
        "type": "object",
        "additionalProperties": false,
        "required": ["action", "input_kind", "secret_input", "authorization_category"],
        "properties": {
            "action": {"const": "minimize_window"},
            "input_kind": {"const": "window_state"},
            "secret_input": {"const": false},
            "authorization_category": {"const": "window_state"},
            "browser_origin": {"type": "null"}
        }
    })
}

fn native_frame_scope_schema() -> Value {
    json!({
        "title":"Metadata-bound non-activating native window frame",
        "type":"object", "additionalProperties":false,
        "required":["action","input_kind","secret_input","authorization_category"],
        "properties":{
            "action":{"const":"set_window_frame"},"input_kind":{"const":"window_state"},
            "secret_input":{"const":false},"authorization_category":{"const":"window_state"},
            "browser_origin":{"type":"null"}
        }
    })
}

fn capture_preparation_scope_schema() -> Value {
    json!({
        "title":"Temporary passive exact-root capture preparation",
        "type":"object","additionalProperties":false,
        "required":["action","input_kind","secret_input","authorization_category"],
        "properties":{
            "action":{"const":"capture_preparation_begin"},"input_kind":{"const":"window_state"},
            "secret_input":{"const":false},"authorization_category":{"const":"window_state"},
            "browser_origin":{"type":"null"}
        }
    })
}

fn pixels_action_scope_schema() -> Value {
    json!({
        "oneOf": [native_minimize_scope_schema(), native_frame_scope_schema(), capture_preparation_scope_schema(), {
            "title": "Observation-bound foreground pixel input",
            "type": "object",
            "additionalProperties": false,
            "required": ["action", "input_kind", "secret_input", "authorization_category"],
            "properties": {
                "action": {"type": "string", "enum": TrustedTaskActionScope::PIXELS_INPUT_ACTIONS},
                "input_kind": {"const": "raw_input"},
                "secret_input": {"const": false},
                "authorization_category": {"const": "raw_input"},
                "browser_origin": {"type": "null"}
            }
        }]
    })
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
            native_minimize_scope_schema(),
            native_frame_scope_schema(),
            capture_preparation_scope_schema(),
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
        json!({
            "name": "start_task",
            "title": "Start DCC-CUA task",
            "description": "Start one exact bounded DCC-CUA task without a secondary confirmation step. The connected Agent Host owns user authorization. observation_mode defaults to semantic; explicitly choose pixels_only for an exact window snapshot without UIA or semantic selectors. Pixels-only tasks require complete exact-window capture proof and grant only closed native window_state methods or the advertised non-secret raw_input actions. Explicit set_window_frame requires get_window_state and its own action scope; it never grants raw input or recording. Each input requires the latest observation and a foreground, unobscured, unchanged native instance. Explicit allow_recording=true grants native video-only recording_start/state/stop in an operator-owned task directory. Live observation and recording reuse the same native source, without an accessibility tree or trajectory.",
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
                "allOf": [{
                    "if": {
                        "required": ["observation_mode"],
                        "properties": {"observation_mode": {"const": "pixels_only"}}
                    },
                    "then": {
                        "required": ["target_process_id", "target_window_handle"],
                        "not": {"required": ["owned_browser_launch"]},
                        "properties": {
                            "surface": {"const": "window"},
                            "allowed_browser_origins": {"maxItems": 0},
                            "allowed_methods": {"items": {"enum": [
                                "get_window_state", "change_window_state", "minimize_window", "set_window_frame", "snapshot",
                                "execute_action", "get_session_state", "get_input_state", "session_health", "poll_session_events",
                                "live_observation_start", "live_observation_state", "live_observation_stop",
                                "recording_start", "recording_state", "recording_stop"
                                ,"capture_preparation_begin", "capture_preparation_state", "capture_preparation_stop", "capture_preparation_snapshot"
                            ]}},
                            "allowed_actions": {"items": pixels_action_scope_schema()}
                        }
                    }
                }, {
                    "if":{"anyOf":[
                        {"required":["allow_capture_preparation"],"properties":{"allow_capture_preparation":{"const":true}}},
                        {"properties":{"allowed_methods":{"contains":{"enum":["capture_preparation_begin","capture_preparation_state","capture_preparation_stop","capture_preparation_snapshot"]}}}},
                        {"properties":{"allowed_actions":{"contains":capture_preparation_scope_schema()}}}
                    ]},
                    "then":{"required":["allow_capture_preparation","observation_mode"],"properties":{
                        "allow_capture_preparation":{"const":true},"observation_mode":{"const":"pixels_only"},"surface":{"const":"window"},
                        "allowed_methods":{"allOf":[
                            {"contains":{"const":"get_window_state"}}, {"contains":{"const":"capture_preparation_begin"}},
                            {"contains":{"const":"capture_preparation_state"}}, {"contains":{"const":"capture_preparation_stop"}}
                        ]}, "allowed_actions":{"contains":capture_preparation_scope_schema()}
                    }}
                }, {
                    "if": {"required":["allow_recording"],"properties":{"allow_recording":{"const":true}}},
                    "then": {"required":["observation_mode"],"properties":{
                        "observation_mode":{"const":"pixels_only"},"surface":{"const":"window"},
                        "allowed_methods":{"allOf":[
                            {"contains":{"const":"recording_start"}},
                            {"contains":{"const":"recording_state"}},
                            {"contains":{"const":"recording_stop"}}
                        ]}
                    }}
                }, {
                    "if":{"properties":{"allowed_methods":{"contains":{"enum":["recording_start","recording_state","recording_stop"]}}}},
                    "then":{"required":["allow_recording"],"properties":{"allow_recording":{"const":true}}}
                }, {
                    "if":{"properties":{"allowed_methods":{"contains":{"const":"live_observation_start"}}}},
                    "then":{"properties":{"allowed_methods":{"allOf":[
                        {"contains":{"const":"live_observation_state"}},
                        {"contains":{"const":"live_observation_stop"}}
                    ]}}}
                }, {
                    "if":{"properties":{"allowed_methods":{"contains":{"const":"set_window_frame"}}}},
                    "then":{"required":["observation_mode"],"properties":{
                        "observation_mode":{"const":"pixels_only"},"surface":{"const":"window"},
                        "allowed_methods":{"contains":{"const":"get_window_state"}},
                        "allowed_actions":{"contains":native_frame_scope_schema()}
                    }}
                }],
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
                    "observation_mode": {"type": "string", "enum": ["semantic", "pixels_only"], "default": "semantic"},
                    "allow_recording": {"type":"boolean","default":false,
                        "description":"Explicit native pixels video-only permission. Requires recording_start/state/stop and an operator-configured DCC_CUA_RECORDING_OUTPUT_ROOT. The server allocates an immutable task directory; callers cannot configure its root."},
                    "allow_capture_preparation":{"type":"boolean","default":false,
                        "description":"Explicit bounded passive exact-root preparation. Requires pixels_only, get_window_state, begin/state/stop, its closed action, and operator-owned DCC_CUA_CAPTURE_PREPARATION_JOURNAL_ROOT. Does not grant input or caller-selected journal/target; prepared snapshots mint no observation/input token."},
                    "allowed_methods": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_ALLOWED_METHODS,
                        "uniqueItems": true,
                        "items": {"type": "string", "enum": [
                            "get_window_state", "change_window_state", "minimize_window", "set_window_frame", "snapshot", "accessibility_snapshot", "verify_state",
                            "capture_preparation_begin", "capture_preparation_state", "capture_preparation_stop", "capture_preparation_snapshot",
                            "find", "wait_for", "execute_action", "get_session_state",
                            "get_input_state", "session_health", "poll_session_events",
                            "clipboard_capture_secret", "browser_snapshot", "browser_prepare",
                            "browser_navigate", "browser_click", "browser_type", "browser_pointer",
                            "browser_set_input_files", "browser_download", "browser_dialog",
                            "live_observation_start", "live_observation_state", "live_observation_stop",
                            "recording_start", "recording_state", "recording_stop"
                        ]}
                    },
                    "allowed_actions": {
                        "type": "array",
                        "description": "Closed final action scopes; use an empty array for observation-only tasks. pixels_only accepts minimize_window or set_window_frame with input_kind=window_state, secret_input=false, authorization_category=window_state; these grant no raw input. Pixel input accepts click, double_click, right_click, toggle, keypress, keyboard_shortcut, type, type_chars with input_kind=raw_input, secret_input=false, authorization_category=raw_input. execute_action requires an actual supported raw scope. Other modes use click/type action names for browser input methods; only secret-handle browser typing uses browser_type.",
                        "minItems": 0,
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
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
        }),
        json!({
            "name": "task_status",
            "title": "DCC-CUA task status",
            "description": "Read the current state of an exact DCC-CUA task.",
            "inputSchema": task_id_schema(),
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "stop_task",
            "title": "Stop DCC-CUA task",
            "description": "Revoke one exact task's runtime lease, then await Host session cleanup, including video and sidecar finalization. Cleanup errors or unknown acknowledgements remain visible; a dropped connection is not a successful stop.",
            "inputSchema": task_id_schema(),
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "dcc_cua_task_call",
            "title": "Run DCC-CUA task call",
            "description": "Call one closed Host method after start_task returned its provider/runtime/PID/HWND binding. A pixels_only snapshot yields a formal observation_id and accessibility_available=false, without semantic element tokens. Its optional capture_diagnostics boolean defaults to false; true adds bounded byte hashes, histograms, timing, and native provenance for the existing two captures without changing pixels or input authority. Semantic snapshots reject this diagnostic opt-in. Its execute_action requires a granted supported raw_input action and that latest observation_id; omit accessibility_state_id, element selectors, secret handles, and input_backend_id. Delivery is foreground only, without implicit activation. Use capture_after for a fresh pixel post-action observation; semantic post snapshots are refused. minimize_window requires the latest observation and same native instance, consumes it after an attempt, and reports native minimized state. set_window_frame requires a one-use window_state_id from explicit get_window_state no more than five seconds old, and frame={x,y,width,height} in exact i32 physical pixels with checked extents. It moves the same full native instance without activation or z-order change and confirms only an exact bounded native readback; every attempt consumes metadata and prior pixel evidence. Its response state does not mint another token. change_window_state accepts only activate or restore_activate; take a fresh snapshot afterward. Out-of-scope, expired, stopped, changed, or stale targets fail without prompting. Recording requires explicit allow_recording and an immutable task output directory; call recording_start with an empty params object for video-only output. Granted live_observation_start/state/stop expose the same native source. Recording start validates the actual first encoded frame, state reports source pauses and failures, and stop awaits video/sidecar finalization. Never pass credential values; use secret handles in supported semantic/browser tasks.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["task_id", "method", "params"],
                "properties": {
                    "task_id": {"type": "string"},
                    "method": {"type": "string"},
                    "params": {"type": "object"}
                },
                "allOf": [{
                    "if":{"properties":{"method":{"const":"capture_preparation_begin"}}},
                    "then":{"properties":{"params":{
                        "type":"object","additionalProperties":false,"required":["request"],
                        "properties":{"request":{"type":"object","additionalProperties":false,
                            "required":["window_state_id","lifetime_ms"],"properties":{
                                "window_state_id":{"type":"string","minLength":1,"maxLength":128},
                                "lifetime_ms":{"type":"integer","minimum":1,"maximum":dcc_cua_protocol::capture_preparation::MAX_PREPARATION_LIFETIME_MS}
                            }}}
                    }}}
                }, {
                    "if":{"properties":{"method":{"enum":["capture_preparation_state","capture_preparation_stop","capture_preparation_snapshot"]}}},
                    "then":{"properties":{"params":{"type":"object","maxProperties":0}}}
                }, {
                    "if":{"properties":{"method":{"const":"set_window_frame"}}},
                    "then":{"properties":{"params":{
                        "type":"object","additionalProperties":false,"required":["window_state_id","frame"],
                        "properties":{
                            "window_state_id":{"type":"string","minLength":1,"maxLength":128,
                                "description":"One-use token from explicit pixels_only get_window_state, at most five seconds old. Snapshot observation IDs do not authorize this method."},
                            "frame":{"type":"object","additionalProperties":false,"required":["x","y","width","height"],
                                "properties":{
                                    "x":{"type":"integer","minimum":-2147483648,"maximum":2147483647},
                                    "y":{"type":"integer","minimum":-2147483648,"maximum":2147483647},
                                    "width":{"type":"integer","minimum":1,"maximum":2147483647},
                                    "height":{"type":"integer","minimum":1,"maximum":2147483647}
                                }
                            }
                        }
                    }}}
                }, {
                    "if": {"properties": {"method": {"const": "snapshot"}}},
                    "then": {"properties": {"params": {
                        "properties": {"capture_diagnostics": {
                            "type": "boolean", "default": false,
                            "description": "Opt-in content-free byte diagnostics on an explicit pixels_only snapshot only. No extra capture or input permission."
                        }}
                    }}}
                }, {
                    "if":{"properties":{"method":{"const":"recording_start"}}},
                    "then":{"properties":{"params":{"type":"object","additionalProperties":false,
                        "properties":{"request":{"type":"object","additionalProperties":false,
                            "properties":{"record_video":{"const":true},"output_dir":{"type":"string",
                                "description":"Optional exact task-authorized directory. The server supplies its immutable path and rejects a different value."}}}}
                    }}}
                }, {
                    "if":{"properties":{"method":{"const":"live_observation_start"}}},
                    "then":{"properties":{"params":{"type":"object","additionalProperties":false,
                        "properties":{"request":{"type":"object","additionalProperties":false,
                            "properties":{"fps":{"type":"integer","minimum":1,"maximum":30},
                                "max_dimension":{"type":"integer","minimum":256,"maximum":4096}}}}
                    }}}
                }, {
                    "if":{"properties":{"method":{"enum":["recording_state","recording_stop","live_observation_state","live_observation_stop"]}}},
                    "then":{"properties":{"params":{"type":"object","maxProperties":0}}}
                }]
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true}
        }),
    ]
}

fn task_id_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["task_id"],
        "properties": {"task_id": {"type": "string"}}
    })
}

fn tool_result(payload: Value) -> Value {
    let is_error = payload.get("ok").and_then(Value::as_bool) == Some(false);
    json!({
        "content": [{"type": "text", "text": payload.to_string()}],
        "structuredContent": payload,
        "isError": is_error,
    })
}

fn tool_error(message: String) -> Value {
    let payload = json!({"ok": false, "error": message});
    json!({
        "content": [{"type": "text", "text": payload.to_string()}],
        "structuredContent": payload,
        "isError": true,
    })
}

fn task_remote_error(
    error: &HostClientError,
    method: &str,
    task_id: &str,
    session: &LogicalTaskSession,
) -> Value {
    let Some(projected) = project_task_remote_error(error, method) else {
        return tool_error(error.to_string());
    };
    let mut result = tool_result(
        serde_json::to_value(projected).expect("typed remote error projection must serialize"),
    );
    let payload = &mut result["structuredContent"];
    payload["task_context"] = json!({
        "provider": "dcc-cua",
        "runtime_version": env!("CARGO_PKG_VERSION"),
        "task_id": task_id,
        "target": {
            "process_id": session.target()["process_id"],
            "window_handle": session.target()["window_handle"],
        },
        "native_action_popups": false,
    });
    result["content"] = json!([{"type":"text", "text": result["structuredContent"].to_string()}]);
    result
}

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
struct TaskRemoteErrorProjection {
    ok: bool,
    error: String,
    code: String,
    error_code: String,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<TaskRemoteErrorReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    details: Option<TaskRemoteErrorDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diagnostic: Option<TaskRemoteErrorDiagnostics>,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
#[serde(untagged)]
enum TaskRemoteErrorReason {
    Capture(dcc_cua_core::ComputerUseCaptureReason),
    CapturePreparation(dcc_cua_protocol::capture_preparation::PreparationFailure),
}

#[derive(Debug, Clone, Default, serde::Serialize, Deserialize)]
struct TaskRemoteErrorDiagnostics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture: Option<dcc_cua_core::ComputerUseCaptureDiagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    foreground_activation: Option<dcc_cua_core::ComputerUseForegroundActivationDiagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture_preparation: Option<dcc_cua_protocol::capture_preparation::PreparationError>,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TaskRemoteSuggestedDeliveryMode {
    Foreground,
    Background,
}

#[derive(Debug, Clone, Default, serde::Serialize, Deserialize)]
struct TaskRemoteErrorDetails {
    #[serde(flatten)]
    diagnostic: TaskRemoteErrorDiagnostics,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timed_out: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    phase: Option<dcc_cua_core::ComputerUseErrorPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action_attempted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    focus_mutation_attempted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_sent: Option<dcc_cua_core::ComputerUseInputState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completion: Option<dcc_cua_core::ComputerUseCompletionState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effect_unknown: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_session_invalidated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_remains_active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    automatic_input: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blind_retry: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fresh_observation_required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exact_target_revalidation_required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    automatic_rebind: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    explicit_rebind_required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    background_delivery_viable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    suggested_delivery_mode: Option<TaskRemoteSuggestedDeliveryMode>,
}

fn remote_error_field<T: serde::de::DeserializeOwned>(details: &Value, name: &str) -> Option<T> {
    serde_json::from_value(details.get(name)?.clone()).ok()
}

fn project_task_remote_error(
    error: &HostClientError,
    method: &str,
) -> Option<TaskRemoteErrorProjection> {
    let HostClientError::Remote {
        code,
        message,
        response,
    } = error
    else {
        return None;
    };
    // The client already owns explicit code/message fields. Never recover an
    // error identity by parsing the compatibility Display string.
    let remote_details = response.get("details").unwrap_or(&Value::Null);
    let diagnostic = TaskRemoteErrorDiagnostics {
        capture: remote_error_field(remote_details, "capture"),
        foreground_activation: remote_error_field(remote_details, "foreground_activation"),
        capture_preparation: remote_error_field(remote_details, "capture_preparation"),
    };
    let mut details = TaskRemoteErrorDetails {
        diagnostic: diagnostic.clone(),
        timed_out: remote_error_field(remote_details, "timed_out"),
        phase: remote_error_field(remote_details, "phase"),
        action_attempted: remote_error_field(remote_details, "action_attempted"),
        focus_mutation_attempted: remote_error_field(remote_details, "focus_mutation_attempted"),
        input_sent: remote_error_field(remote_details, "input_sent"),
        completion: remote_error_field(remote_details, "completion"),
        effect_unknown: remote_error_field(remote_details, "effect_unknown"),
        local_session_invalidated: remote_error_field(remote_details, "local_session_invalidated"),
        session_remains_active: remote_error_field(remote_details, "session_remains_active"),
        automatic_input: remote_error_field(remote_details, "automatic_input"),
        blind_retry: remote_error_field(remote_details, "blind_retry"),
        fresh_observation_required: remote_error_field(
            remote_details,
            "fresh_observation_required",
        ),
        exact_target_revalidation_required: remote_error_field(
            remote_details,
            "exact_target_revalidation_required",
        ),
        automatic_rebind: remote_error_field(remote_details, "automatic_rebind"),
        explicit_rebind_required: remote_error_field(remote_details, "explicit_rebind_required"),
        background_delivery_viable: remote_error_field(
            remote_details,
            "background_delivery_viable",
        ),
        suggested_delivery_mode: remote_error_field(remote_details, "suggested_delivery_mode"),
    };
    if method == "set_window_frame" {
        // Preserve the original method-specific coherence fence; independent
        // typed diagnostics survive even when its mutation flags are invalid.
        details = native_frame_failure_projection(remote_details)
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        details.diagnostic = diagnostic.clone();
    }
    let reason = diagnostic
        .capture
        .as_ref()
        .map(|capture| TaskRemoteErrorReason::Capture(capture.reason))
        .or_else(|| {
            diagnostic
                .capture_preparation
                .as_ref()
                .map(|preparation| TaskRemoteErrorReason::CapturePreparation(preparation.reason))
        });
    let has_diagnostic = diagnostic.capture.is_some()
        || diagnostic.foreground_activation.is_some()
        || diagnostic.capture_preparation.is_some();
    let has_details = !serde_json::to_value(&details).ok()?.as_object()?.is_empty();
    Some(TaskRemoteErrorProjection {
        ok: false,
        error: error.to_string(),
        code: code.clone(),
        error_code: code.clone(),
        message: message.clone(),
        reason,
        details: has_details.then_some(details),
        diagnostic: has_diagnostic.then_some(diagnostic),
    })
}

#[derive(serde::Serialize, Deserialize)]
struct NativeFrameFailureProjection {
    phase: dcc_cua_core::ComputerUseErrorPhase,
    action_attempted: bool,
    input_sent: dcc_cua_core::ComputerUseInputState,
    completion: dcc_cua_core::ComputerUseCompletionState,
    effect_unknown: bool,
    automatic_input: bool,
    blind_retry: bool,
    fresh_observation_required: bool,
}

fn native_frame_failure_projection(value: &Value) -> Option<Value> {
    use dcc_cua_core::{ComputerUseCompletionState, ComputerUseErrorPhase, ComputerUseInputState};
    let projection: NativeFrameFailureProjection = serde_json::from_value(value.clone()).ok()?;
    let expected_phase = if projection.action_attempted {
        ComputerUseErrorPhase::LocalMutationDispatch
    } else {
        ComputerUseErrorPhase::PreDispatch
    };
    let expected_completion = if projection.action_attempted {
        ComputerUseCompletionState::Unknown
    } else {
        ComputerUseCompletionState::Known
    };
    if projection.phase != expected_phase
        || projection.completion != expected_completion
        || projection.input_sent != ComputerUseInputState::NotSent
        || projection.effect_unknown != projection.action_attempted
        || projection.automatic_input
        || projection.blind_retry
        || !projection.fresh_observation_required
    {
        return None;
    }
    serde_json::to_value(projection).ok()
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn runtime_build() -> RuntimeBuildIdentity {
    RuntimeBuildIdentity {
        runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
        source_revision: option_env!("CUA_BUILD_SOURCE_REVISION").map(str::to_owned),
        source_dirty: option_env!("CUA_BUILD_SOURCE_DIRTY").and_then(|value| value.parse().ok()),
        build_profile: option_env!("CUA_BUILD_PROFILE").map(str::to_owned),
        target: option_env!("CUA_BUILD_TARGET").map(str::to_owned),
    }
}

pub async fn run(diagnostics_dir: Option<std::path::PathBuf>) -> Result<(), Box<dyn Error>> {
    let mut server = TaskAuthorizationServer::automatic();
    server.diagnostics = Some(ConnectionDiagnostics::new(runtime_build(), diagnostics_dir));
    let mut input = BufReader::new(tokio::io::stdin());
    let mut output = BufWriter::new(tokio::io::stdout());
    let mut reason = CloseReason::InputError;
    let result = run_transport(&mut server, &mut input, &mut output, &mut reason).await;
    // Preserve the last observed task/Host associations at disconnect. Dropping
    // transports below does not attest that detached Host cleanup has finished.
    server.update_diagnostics();
    if let Some(diagnostics) = server.diagnostics.as_mut() {
        diagnostics.close(reason);
    }
    // Finalize only this connection's owned task sessions before dropping their
    // transports. Read-only diagnostics retain the last observed associations.
    let cleanup_failures = server.shutdown_tasks().await;
    server.proposals.clear();
    if cleanup_failures.is_empty() {
        result
    } else {
        Err(std::io::Error::other(format!(
            "MCP shutdown cleanup failed: {}; transport: {}",
            Value::Array(cleanup_failures),
            result
                .err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "closed".into())
        ))
        .into())
    }
}

async fn run_transport(
    server: &mut TaskAuthorizationServer,
    input: &mut BufReader<tokio::io::Stdin>,
    output: &mut BufWriter<tokio::io::Stdout>,
    reason: &mut CloseReason,
) -> Result<(), Box<dyn Error>> {
    loop {
        *reason = CloseReason::InputError;
        let mut line = Vec::new();
        let count = (&mut *input)
            .take(dcc_cua_protocol::MAX_JSON_FRAME_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .await?;
        if count == 0 {
            *reason = CloseReason::StdinEof;
            break;
        }
        if let Some(diagnostics) = server.diagnostics.as_mut() {
            diagnostics.activity();
        }
        if count > dcc_cua_protocol::MAX_JSON_FRAME_BYTES {
            *reason = CloseReason::FrameLimit;
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
                *reason = CloseReason::OutputError;
                output
                    .write_all(
                        format!("{}\n", rpc_error(Value::Null, -32700, "Parse error")).as_bytes(),
                    )
                    .await?;
                output.flush().await?;
                continue;
            }
        };
        if let Some(diagnostics) = server.diagnostics.as_mut() {
            diagnostics.set_request_in_flight(true);
        }
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
        server.update_diagnostics();
        if let Some(diagnostics) = server.diagnostics.as_mut() {
            diagnostics.set_request_in_flight(false);
        }
        if responses.is_empty() {
            continue;
        }
        let response = if responses.len() == 1 {
            responses.remove(0)
        } else {
            Value::Array(responses)
        };
        *reason = CloseReason::OutputError;
        output.write_all(format!("{response}\n").as_bytes()).await?;
        output.flush().await?;
    }
    Ok(())
}

#[cfg(test)]
mod capture_preparation_tests;
#[cfg(test)]
mod tests;
