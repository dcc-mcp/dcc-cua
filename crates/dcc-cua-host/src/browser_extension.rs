use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot};
use uuid::Uuid;

use crate::{
    HostError, HostEvidencePublication, HostProtocolErrorCode, HostSession, MAX_REQUEST_ID_CHARS,
    Request, authorized_session,
};

const EXTENSION_SCHEMA: &str = "dcc-cua.browser-extension.v1";
const EXTENSION_PROTOCOL_VERSION: u64 = 1;
const MAX_EXTENSION_CALL_TIMEOUT_MS: u64 = 30_000;
const PROVIDER_STALE_AFTER: Duration = Duration::from_secs(15);
const PROVIDER_QUEUE_CAPACITY: usize = 16;

#[derive(Clone)]
pub(super) struct BrowserExtensionRegistry {
    providers: Arc<Mutex<HashMap<String, Arc<Provider>>>>,
}

struct Provider {
    secret: String,
    extension_id: String,
    extension_version: String,
    invocation_origin: String,
    browser_process_id: u32,
    pairing: ExtensionPairing,
    capabilities: Vec<String>,
    commands_tx: mpsc::Sender<Value>,
    commands_rx: Mutex<mpsc::Receiver<Value>>,
    pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    last_seen: Mutex<Instant>,
}

#[derive(Debug, Clone)]
struct ExtensionPairing {
    session_nonce: String,
    tab_id: u64,
    window_id: u64,
    origin: String,
    document_id: String,
}

pub(super) fn browser_extension_registry() -> BrowserExtensionRegistry {
    static REGISTRY: OnceLock<BrowserExtensionRegistry> = OnceLock::new();
    REGISTRY.get_or_init(BrowserExtensionRegistry::new).clone()
}

pub(super) enum RoutedBrowserExtensionRequest {
    Unhandled(Box<Request>),
    Handled(Result<(Value, Option<Vec<u8>>), HostError>),
}

pub(super) async fn route_host_request(
    request: Request,
    sessions: &mut HashMap<String, HostSession>,
) -> RoutedBrowserExtensionRequest {
    let result = match request {
        Request::RegisterBrowserExtension {
            hello,
            invocation_origin,
            browser_process_id,
        } => {
            browser_extension_registry()
                .register(&hello, &invocation_origin, browser_process_id)
                .await
        }
        Request::BrowserExtensionNext {
            provider_id,
            provider_secret,
            timeout_ms,
        } => {
            browser_extension_registry()
                .next_command(&provider_id, &provider_secret, timeout_ms)
                .await
        }
        Request::CompleteBrowserExtension {
            provider_id,
            provider_secret,
            response,
        } => {
            browser_extension_registry()
                .complete(&provider_id, &provider_secret, response)
                .await
        }
        Request::UnregisterBrowserExtension {
            provider_id,
            provider_secret,
        } => {
            browser_extension_registry()
                .unregister(&provider_id, &provider_secret)
                .await
        }
        Request::BrowserExtensionStatus {
            session_id,
            task_grant_id,
            window_capability,
        } => {
            let host =
                match authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await
                {
                    Ok(host) => host,
                    Err(error) => return RoutedBrowserExtensionRequest::Handled(Err(error)),
                };
            if let Err(error) = host.require_task_authorized_method("browser_extension_status") {
                return RoutedBrowserExtensionRequest::Handled(Err(error));
            }
            let mut status = browser_extension_registry()
                .status_for_process(host.target_process_id)
                .await;
            status["window_binding"] = json!({
                "process_id": host.target_process_id,
                "window_handle": host.target_window_handle,
            });
            Ok(status)
        }
        Request::BrowserExtensionCall {
            session_id,
            task_grant_id,
            window_capability,
            provider_id,
            expected_origin,
            method,
            params,
            timeout_ms,
        } => {
            let host =
                match authorized_session(sessions, &session_id, &task_grant_id, &window_capability)
                    .await
                {
                    Ok(host) => host,
                    Err(error) => return RoutedBrowserExtensionRequest::Handled(Err(error)),
                };
            if let Err(error) = host.require_task_authorized_method("browser_extension_call") {
                return RoutedBrowserExtensionRequest::Handled(Err(error));
            }
            if matches!(method.as_str(), "click" | "type" | "unpair") && !host.allow_browser_input {
                return RoutedBrowserExtensionRequest::Handled(Err(HostError::coded_protocol(
                    HostProtocolErrorCode::BrowserInputNotGranted,
                    "browser input is not granted for the logical task session",
                )));
            }
            let response = browser_extension_registry()
                .call(
                    &provider_id,
                    &expected_origin,
                    host.target_process_id,
                    &method,
                    &params,
                    timeout_ms,
                )
                .await;
            if response.is_ok() {
                if method == "snapshot" {
                    host.synchronize_action_evidence_epoch_with(
                        HostEvidencePublication::BrowserSnapshot,
                    );
                } else {
                    host.invalidate_observations();
                }
            }
            response
        }
        request => return RoutedBrowserExtensionRequest::Unhandled(Box::new(request)),
    };
    RoutedBrowserExtensionRequest::Handled(result.map(|response| (response, None)))
}

impl BrowserExtensionRegistry {
    pub(super) fn new() -> Self {
        Self {
            providers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(super) async fn register(
        &self,
        hello: &Value,
        invocation_origin: &str,
        browser_process_id: u32,
    ) -> Result<Value, HostError> {
        if browser_process_id == 0 {
            return Err(HostError::Protocol(
                "browser extension registration requires the browser process id".into(),
            ));
        }
        let (extension_id, extension_version, capabilities, pairing) =
            parse_extension_hello(hello, invocation_origin)?;
        self.prune_stale().await;
        let provider_id = format!("browser-extension-{}", Uuid::new_v4());
        let provider_secret = Uuid::new_v4().to_string();
        let (commands_tx, commands_rx) = mpsc::channel(PROVIDER_QUEUE_CAPACITY);
        let provider = Arc::new(Provider {
            secret: provider_secret.clone(),
            extension_id,
            extension_version,
            invocation_origin: invocation_origin.to_owned(),
            browser_process_id,
            pairing,
            capabilities,
            commands_tx,
            commands_rx: Mutex::new(commands_rx),
            pending: Mutex::new(HashMap::new()),
            last_seen: Mutex::new(Instant::now()),
        });
        self.providers
            .lock()
            .await
            .insert(provider_id.clone(), provider);
        Ok(json!({
            "type": "browser_extension_registered",
            "provider_id": provider_id,
            "provider_secret": provider_secret,
            "poll_timeout_ms": 5_000,
        }))
    }

    pub(super) async fn unregister(
        &self,
        provider_id: &str,
        provider_secret: &str,
    ) -> Result<Value, HostError> {
        let provider = self
            .authorized_provider(provider_id, provider_secret)
            .await?;
        self.providers.lock().await.remove(provider_id);
        provider.pending.lock().await.clear();
        Ok(json!({
            "type": "browser_extension_unregistered",
            "provider_id": provider_id,
        }))
    }

    pub(super) async fn next_command(
        &self,
        provider_id: &str,
        provider_secret: &str,
        timeout_ms: u64,
    ) -> Result<Value, HostError> {
        if timeout_ms > MAX_EXTENSION_CALL_TIMEOUT_MS {
            return Err(HostError::Protocol(format!(
                "browser extension poll timeout must be at most {MAX_EXTENSION_CALL_TIMEOUT_MS} ms"
            )));
        }
        let provider = self
            .authorized_provider(provider_id, provider_secret)
            .await?;
        *provider.last_seen.lock().await = Instant::now();
        let command = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            provider.commands_rx.lock().await.recv(),
        )
        .await
        .ok()
        .flatten();
        Ok(json!({
            "type": "browser_extension_command",
            "provider_id": provider_id,
            "command": command,
        }))
    }

    pub(super) async fn complete(
        &self,
        provider_id: &str,
        provider_secret: &str,
        response: Value,
    ) -> Result<Value, HostError> {
        let provider = self
            .authorized_provider(provider_id, provider_secret)
            .await?;
        *provider.last_seen.lock().await = Instant::now();
        validate_extension_response(&response)?;
        let request_id = bounded_string(&response, "request_id", MAX_REQUEST_ID_CHARS)?.to_owned();
        let sender = provider
            .pending
            .lock()
            .await
            .remove(&request_id)
            .ok_or_else(|| {
                HostError::Protocol(
                    "browser extension response does not match a pending request".into(),
                )
            })?;
        sender.send(response).map_err(|_| {
            HostError::Protocol("browser extension caller disconnected before completion".into())
        })?;
        Ok(json!({
            "type": "browser_extension_completion_accepted",
            "provider_id": provider_id,
            "request_id": request_id,
        }))
    }

    pub(super) async fn status_for_process(&self, browser_process_id: u32) -> Value {
        self.prune_stale().await;
        let providers = self.providers.lock().await;
        let rows = providers
            .iter()
            .filter(|(_, provider)| provider.browser_process_id == browser_process_id)
            .map(|(provider_id, provider)| provider.summary(provider_id))
            .collect::<Vec<_>>();
        json!({
            "type": "browser_extension_status",
            "available": !rows.is_empty(),
            "providers": rows,
            "fallback_provider": "cdp",
            "install_state": if rows.is_empty() { "not_connected_or_not_paired" } else { "ready" },
        })
    }

    pub(super) async fn call(
        &self,
        provider_id: &str,
        expected_origin: &str,
        expected_process_id: u32,
        method: &str,
        params: &Value,
        timeout_ms: u64,
    ) -> Result<Value, HostError> {
        if timeout_ms == 0 || timeout_ms > MAX_EXTENSION_CALL_TIMEOUT_MS {
            return Err(HostError::Protocol(format!(
                "browser extension call timeout must be between 1 and {MAX_EXTENSION_CALL_TIMEOUT_MS} ms"
            )));
        }
        self.prune_stale().await;
        let provider = self.provider(provider_id).await?;
        if provider.browser_process_id != expected_process_id {
            return Err(HostError::Protocol(
                "browser extension provider process does not match the exact window session".into(),
            ));
        }
        if provider.pairing.origin != expected_origin {
            return Err(HostError::Protocol(
                "browser extension provider origin does not match the requested origin".into(),
            ));
        }
        if !matches!(method, "snapshot" | "click" | "type" | "unpair") {
            return Err(HostError::Protocol(format!(
                "unsupported browser extension method: {method}"
            )));
        }
        let request_id = Uuid::new_v4().to_string();
        let mut command = params.as_object().cloned().ok_or_else(|| {
            HostError::Protocol("browser extension call params must be a JSON object".into())
        })?;
        for forbidden in ["schema", "request_id", "method", "session_nonce", "tab_id"] {
            if command.contains_key(forbidden) {
                return Err(HostError::Protocol(format!(
                    "browser extension call params cannot override {forbidden}"
                )));
            }
        }
        command.insert("schema".into(), Value::String(EXTENSION_SCHEMA.into()));
        command.insert("request_id".into(), Value::String(request_id.clone()));
        command.insert("method".into(), Value::String(method.into()));
        command.insert(
            "session_nonce".into(),
            Value::String(provider.pairing.session_nonce.clone()),
        );
        command.insert("tab_id".into(), Value::from(provider.pairing.tab_id));
        let (sender, receiver) = oneshot::channel();
        provider
            .pending
            .lock()
            .await
            .insert(request_id.clone(), sender);
        if provider
            .commands_tx
            .send(Value::Object(command))
            .await
            .is_err()
        {
            provider.pending.lock().await.remove(&request_id);
            return Err(HostError::Protocol(
                "browser extension provider disconnected".into(),
            ));
        }
        let response = match tokio::time::timeout(Duration::from_millis(timeout_ms), receiver).await
        {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                provider.pending.lock().await.remove(&request_id);
                return Err(HostError::Protocol(
                    "browser extension provider dropped the pending call".into(),
                ));
            }
            Err(_) => {
                provider.pending.lock().await.remove(&request_id);
                return Err(HostError::Protocol(
                    "browser extension call timed out".into(),
                ));
            }
        };
        if response["schema"] != EXTENSION_SCHEMA || response["type"] != "response" {
            return Err(HostError::Protocol(
                "browser extension returned an invalid response envelope".into(),
            ));
        }
        if response["ok"] == false {
            let code = response["error"]["code"]
                .as_str()
                .unwrap_or("browser_extension_error");
            let message = response["error"]["message"]
                .as_str()
                .unwrap_or("browser extension call failed");
            return Err(HostError::Protocol(format!("{code}: {message}")));
        }
        Ok(json!({
            "type": "browser_extension_result",
            "provider_id": provider_id,
            "origin": provider.pairing.origin,
            "method": method,
            "result": response["result"],
        }))
    }

    async fn provider(&self, provider_id: &str) -> Result<Arc<Provider>, HostError> {
        self.providers
            .lock()
            .await
            .get(provider_id)
            .cloned()
            .ok_or_else(|| HostError::Protocol("browser extension provider not found".into()))
    }

    async fn authorized_provider(
        &self,
        provider_id: &str,
        secret: &str,
    ) -> Result<Arc<Provider>, HostError> {
        let provider = self.provider(provider_id).await?;
        if provider.secret != secret {
            return Err(HostError::Protocol(
                "browser extension provider secret mismatch".into(),
            ));
        }
        Ok(provider)
    }

    async fn prune_stale(&self) {
        let providers = self.providers.lock().await;
        let snapshots = providers
            .iter()
            .map(|(id, provider)| (id.clone(), Arc::clone(provider)))
            .collect::<Vec<_>>();
        drop(providers);
        let mut stale = Vec::new();
        for (id, provider) in snapshots {
            if provider.last_seen.lock().await.elapsed() >= PROVIDER_STALE_AFTER {
                stale.push(id);
            }
        }
        let mut providers = self.providers.lock().await;
        for id in stale {
            providers.remove(&id);
        }
    }
}

impl Provider {
    fn summary(&self, provider_id: &str) -> Value {
        json!({
            "provider_id": provider_id,
            "provider": "extension",
            "extension": {
                "id": self.extension_id,
                "version": self.extension_version,
                "invocation_origin": self.invocation_origin,
                "browser_process_id": self.browser_process_id,
            },
            "pairing": {
                "tab_id": self.pairing.tab_id,
                "window_id": self.pairing.window_id,
                "origin": self.pairing.origin,
                "document_id": self.pairing.document_id,
            },
            "capabilities": self.capabilities,
        })
    }
}

fn parse_extension_hello(
    hello: &Value,
    invocation_origin: &str,
) -> Result<(String, String, Vec<String>, ExtensionPairing), HostError> {
    ensure_exact_fields(
        hello,
        &[
            "schema",
            "type",
            "protocol",
            "extension",
            "capabilities",
            "pairing",
        ],
        "hello",
    )?;
    if hello["schema"] != EXTENSION_SCHEMA || hello["type"] != "hello" {
        return Err(HostError::Protocol(
            "browser extension hello schema is unsupported".into(),
        ));
    }
    ensure_exact_fields(&hello["protocol"], &["min", "max"], "protocol")?;
    let minimum = hello["protocol"]["min"].as_u64().unwrap_or(0);
    let maximum = hello["protocol"]["max"].as_u64().unwrap_or(0);
    if minimum > EXTENSION_PROTOCOL_VERSION || maximum < EXTENSION_PROTOCOL_VERSION {
        return Err(HostError::Protocol(
            "browser extension protocol ranges do not overlap".into(),
        ));
    }
    ensure_exact_fields(&hello["extension"], &["id", "version"], "extension")?;
    let extension_id = bounded_string(&hello["extension"], "id", 128)?.to_owned();
    let extension_version = bounded_string(&hello["extension"], "version", 64)?.to_owned();
    if !invocation_matches_extension(invocation_origin, &extension_id) {
        return Err(HostError::Protocol(
            "native messaging invocation origin does not match the extension id".into(),
        ));
    }
    let pairing = &hello["pairing"];
    ensure_exact_fields(
        pairing,
        &[
            "session_nonce",
            "tab_id",
            "window_id",
            "origin",
            "document_id",
        ],
        "pairing",
    )?;
    let capability_values = hello["capabilities"]
        .as_array()
        .filter(|values| !values.is_empty() && values.len() <= 32)
        .ok_or_else(|| {
            HostError::Protocol(
                "browser extension capabilities must contain between 1 and 32 entries".into(),
            )
        })?;
    let capabilities = capability_values
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty() && value.len() <= 128)
                .map(str::to_owned)
                .ok_or_else(|| {
                    HostError::Protocol("browser extension capability is invalid".into())
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if capabilities
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len()
        != capabilities.len()
    {
        return Err(HostError::Protocol(
            "browser extension capabilities must not contain duplicates".into(),
        ));
    }
    Ok((
        extension_id,
        extension_version,
        capabilities,
        ExtensionPairing {
            session_nonce: bounded_string(pairing, "session_nonce", 64)?.to_owned(),
            tab_id: positive_id(pairing, "tab_id")?,
            window_id: positive_id(pairing, "window_id")?,
            origin: bounded_string(pairing, "origin", 2048)?.to_owned(),
            document_id: bounded_string(pairing, "document_id", 128)?.to_owned(),
        },
    ))
}

pub(super) fn validate_extension_response(response: &Value) -> Result<(), HostError> {
    if response["schema"] != EXTENSION_SCHEMA || response["type"] != "response" {
        return Err(HostError::Protocol(
            "browser extension returned an invalid response envelope".into(),
        ));
    }
    bounded_string(response, "request_id", MAX_REQUEST_ID_CHARS)?;
    match response["ok"].as_bool() {
        Some(true) => {
            ensure_exact_fields(
                response,
                &["schema", "type", "request_id", "ok", "result"],
                "success response",
            )?;
        }
        Some(false) => {
            ensure_exact_fields(
                response,
                &["schema", "type", "request_id", "ok", "error"],
                "error response",
            )?;
            ensure_exact_fields(&response["error"], &["code", "message"], "response error")?;
            bounded_string(&response["error"], "code", 64)?;
            bounded_string(&response["error"], "message", 256)?;
        }
        None => {
            return Err(HostError::Protocol(
                "browser extension response ok field must be boolean".into(),
            ));
        }
    }
    Ok(())
}

fn ensure_exact_fields(
    value: &Value,
    expected: &[&str],
    object_name: &str,
) -> Result<(), HostError> {
    let object = value.as_object().ok_or_else(|| {
        HostError::Protocol(format!("browser extension {object_name} must be an object"))
    })?;
    if object.len() != expected.len() || !expected.iter().all(|field| object.contains_key(*field)) {
        return Err(HostError::Protocol(format!(
            "browser extension {object_name} fields do not match protocol v1"
        )));
    }
    Ok(())
}

fn bounded_string<'a>(value: &'a Value, field: &str, maximum: usize) -> Result<&'a str, HostError> {
    value[field]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= maximum)
        .ok_or_else(|| {
            HostError::Protocol(format!(
                "browser extension {field} must contain 1..{maximum} characters"
            ))
        })
}

fn positive_id(value: &Value, field: &str) -> Result<u64, HostError> {
    value[field]
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(|| HostError::Protocol(format!("browser extension {field} must be positive")))
}

fn invocation_matches_extension(invocation_origin: &str, extension_id: &str) -> bool {
    invocation_origin == extension_id
        || invocation_origin == format!("chrome-extension://{extension_id}/")
        || invocation_origin == format!("moz-extension://{extension_id}/")
}
