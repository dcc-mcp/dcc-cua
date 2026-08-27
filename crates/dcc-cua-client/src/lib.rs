//! Small client for the versioned local Computer Use Host protocol.
//!
//! The client deliberately exposes the protocol as JSON values so callers
//! can own its higher-level task contracts without duplicating framing,
//! request correlation, or binary image handling.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    future::Future,
    path::Path,
    pin::Pin,
    process::{ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, ReadHalf, WriteHalf};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;

pub use dcc_cua_protocol::{
    DEFAULT_SESSION_IDLE_TIMEOUT_MS, HOST_PROTOCOL_VERSION, MAX_BINARY_FRAME_BYTES,
    MAX_JSON_FRAME_BYTES, MAX_PARALLEL_DISCOVERY_REQUESTS, MAX_REQUEST_ID_CHARS,
    MAX_SESSION_IDLE_TIMEOUT_MS, MIN_SESSION_IDLE_TIMEOUT_MS,
};
use dcc_cua_protocol::{
    FrameError, host_method_traits, validate_request_id as validate_protocol_request_id,
};

trait HostStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T> HostStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

type BoxedHostStream = Box<dyn HostStream>;

#[derive(Debug)]
pub struct HostResponse {
    pub value: Value,
    pub binary_attachment: Option<Vec<u8>>,
}

#[derive(Debug, thiserror::Error)]
pub enum HostClientError {
    #[error("host transport failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("host protocol failed: {0}")]
    Protocol(String),
    #[error(
        "host request timed out after {timeout_ms} ms; reconnect before sending another request"
    )]
    Timeout { timeout_ms: u128 },
    #[error("host returned {code}: {message}")]
    Remote {
        code: String,
        message: String,
        response: Value,
    },
}

const CONNECTION_READY: u8 = 0;
const CONNECTION_IN_FLIGHT: u8 = 1;
const CONNECTION_UNUSABLE: u8 = 2;

struct RequestOperationGuard {
    state: Arc<AtomicU8>,
    complete: bool,
}

impl RequestOperationGuard {
    fn complete(mut self) {
        self.state.store(CONNECTION_READY, Ordering::Release);
        self.complete = true;
    }
}

impl Drop for RequestOperationGuard {
    fn drop(&mut self) {
        if !self.complete {
            self.state.store(CONNECTION_UNUSABLE, Ordering::Release);
        }
    }
}

pub type HostClientResult<T> = Result<T, HostClientError>;

struct ReceivedResponse {
    request_id: String,
    value: Value,
    binary_attachment: Option<Vec<u8>>,
}

impl ReceivedResponse {
    fn into_result(self) -> HostClientResult<HostResponse> {
        if self.value["type"] == "error" {
            return Err(HostClientError::Remote {
                code: self.value["code"]
                    .as_str()
                    .unwrap_or("host_error")
                    .to_owned(),
                message: self.value["message"]
                    .as_str()
                    .unwrap_or("Host returned an error")
                    .to_owned(),
                response: self.value,
            });
        }
        Ok(HostResponse {
            value: self.value,
            binary_attachment: self.binary_attachment,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotTransport {
    BinaryFrame,
    SharedMemory,
}

impl SnapshotTransport {
    fn as_wire_name(self) -> &'static str {
        match self {
            Self::BinaryFrame => "binary_frame",
            Self::SharedMemory => "shared_memory",
        }
    }
}

/// One multiplexed client connection to a dcc-cua Host.
pub struct HostClient {
    reader: ReadHalf<BoxedHostStream>,
    writer: WriteHalf<BoxedHostStream>,
    pending_responses: HashMap<String, ReceivedResponse>,
    outstanding_request_ids: HashSet<String>,
    next_request_id: u64,
    hello_complete: bool,
    snapshot_transport: SnapshotTransport,
    capabilities: Vec<String>,
    connection_state: Arc<AtomicU8>,
}

/// One logical agent task bound to one persistent Host connection and window session.
///
/// Scoped requests automatically receive the exact session id, task grant id,
/// and window capability returned by `open_session`, preventing callers from
/// accidentally mixing credentials from another task or Host connection.
pub struct LogicalTaskSession {
    client: HostClient,
    session_id: String,
    task_grant_id: String,
    window_capability: String,
    target: Value,
    idle_timeout_ms: u64,
}

impl fmt::Debug for LogicalTaskSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LogicalTaskSession")
            .field("session_id", &self.session_id)
            .field("task_grant_id", &self.task_grant_id)
            .field("target", &self.target)
            .field("idle_timeout_ms", &self.idle_timeout_ms)
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

impl LogicalTaskSession {
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    #[must_use]
    pub fn task_grant_id(&self) -> &str {
        &self.task_grant_id
    }

    #[must_use]
    pub fn idle_timeout_ms(&self) -> u64 {
        self.idle_timeout_ms
    }

    /// Exact target identity returned by Host after it opens the task.
    #[must_use]
    pub fn target(&self) -> &Value {
        &self.target
    }

    /// Send a request through this task's existing Host session.
    ///
    /// The caller supplies only method-specific fields. Exact session
    /// credentials are inserted here and conflicting values are rejected.
    pub async fn request(
        &mut self,
        method: impl Into<String>,
        params: Value,
    ) -> HostClientResult<HostResponse> {
        let method = method.into();
        if matches!(method.as_str(), "hello" | "open_session" | "stop_session") {
            return Err(HostClientError::Protocol(format!(
                "{method} is managed by LogicalTaskSession"
            )));
        }
        let params = bind_task_credentials(
            params,
            &self.session_id,
            &self.task_grant_id,
            &self.window_capability,
        )?;
        self.client.request(method, params).await
    }

    /// Send one bounded request through this task's existing Host session.
    pub async fn request_with_timeout(
        &mut self,
        method: impl Into<String>,
        params: Value,
        timeout: Duration,
    ) -> HostClientResult<HostResponse> {
        let method = method.into();
        if matches!(method.as_str(), "hello" | "open_session" | "stop_session") {
            return Err(HostClientError::Protocol(format!(
                "{method} is managed by LogicalTaskSession"
            )));
        }
        let params = bind_task_credentials(
            params,
            &self.session_id,
            &self.task_grant_id,
            &self.window_capability,
        )?;
        self.client
            .request_with_timeout(method, params, timeout)
            .await
    }

    /// Stop the task session and return the still-negotiated Host connection.
    pub async fn close(mut self) -> HostClientResult<HostClient> {
        self.client
            .request(
                "stop_session",
                json!({
                    "session_id": self.session_id,
                }),
            )
            .await?;
        Ok(self.client)
    }

    /// Return the negotiated Host connection without stopping the task.
    ///
    /// This is intended for ownership hand-off. Dropping the connection still
    /// stops every connection-scoped session in the Host.
    #[must_use]
    pub fn into_client(self) -> HostClient {
        self.client
    }
}

/// A CUA Host child and its already-negotiated stdio client.
///
/// Core can use this when it owns the Host lifecycle. Endpoint connections
/// remain available for supervisors that launch the CLI separately.
pub struct HostProcess {
    client: Option<HostClient>,
    child: Option<Child>,
    stderr_capture: Option<ChildStderrCapture>,
}

const MAX_CAPTURED_CHILD_STDERR_BYTES: usize = 16 * 1024;
const CHILD_STDERR_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Default)]
struct ChildStderrState {
    retained: Vec<u8>,
    total_bytes: usize,
    read_failed: bool,
}

impl ChildStderrState {
    fn record(&mut self, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        let remaining = MAX_CAPTURED_CHILD_STDERR_BYTES.saturating_sub(self.retained.len());
        self.retained
            .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
    }

    fn summary(&self) -> ChildStderrSummary {
        ChildStderrSummary {
            retained_bytes: self.retained.len(),
            total_bytes: self.total_bytes,
            truncated: self.total_bytes > self.retained.len(),
            read_failed: self.read_failed,
        }
    }
}

struct ChildStderrSummary {
    retained_bytes: usize,
    total_bytes: usize,
    truncated: bool,
    read_failed: bool,
}

impl fmt::Debug for ChildStderrSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChildStderrSummary")
            .field("retained_bytes", &self.retained_bytes)
            .field("total_bytes", &self.total_bytes)
            .field("truncated", &self.truncated)
            .field("read_failed", &self.read_failed)
            .finish()
    }
}

struct ChildStderrCapture {
    state: Arc<Mutex<ChildStderrState>>,
    task: JoinHandle<()>,
}

impl ChildStderrCapture {
    fn start(mut stderr: ChildStderr) -> Self {
        let state = Arc::new(Mutex::new(ChildStderrState::default()));
        let task_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let mut chunk = [0_u8; 4096];
            loop {
                let read = match stderr.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(_) => {
                        task_state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .read_failed = true;
                        break;
                    }
                };
                let mut state = task_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.record(&chunk[..read]);
            }
        });
        Self { state, task }
    }

    fn summary(&self) -> ChildStderrSummary {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .summary()
    }

    async fn finish(self) {
        let mut task = self.task;
        if tokio::time::timeout(CHILD_STDERR_DRAIN_TIMEOUT, &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

impl fmt::Debug for HostProcess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stderr = self
            .stderr_capture
            .as_ref()
            .map(ChildStderrCapture::summary);
        formatter
            .debug_struct("HostProcess")
            .field("pid", &self.child.as_ref().and_then(Child::id))
            .field("client", &self.client)
            .field("stderr_capture", &stderr)
            .finish()
    }
}

impl HostProcess {
    /// Spawn `dcc-cua host --stdio` and complete the Host handshake.
    pub async fn spawn(
        binary_path: impl AsRef<Path>,
        client_name: impl Into<String>,
        snapshot_transport: SnapshotTransport,
    ) -> HostClientResult<Self> {
        Self::spawn_with_host_args(binary_path, client_name, snapshot_transport, &[]).await
    }

    /// Spawn a Host with explicit user-approved startup arguments.
    pub async fn spawn_with_host_args(
        binary_path: impl AsRef<Path>,
        client_name: impl Into<String>,
        snapshot_transport: SnapshotTransport,
        host_args: &[&str],
    ) -> HostClientResult<Self> {
        let mut command = Command::new(binary_path.as_ref());
        command
            .arg("host")
            .arg("--stdio")
            .args(host_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_host_process(&mut command);
        let mut child = command.spawn()?;
        let stderr_capture = child.stderr.take().map(ChildStderrCapture::start);
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                if let Some(capture) = stderr_capture {
                    capture.finish().await;
                }
                return Err(HostClientError::Protocol(
                    "spawned Host did not expose stdin".into(),
                ));
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                if let Some(capture) = stderr_capture {
                    capture.finish().await;
                }
                return Err(HostClientError::Protocol(
                    "spawned Host did not expose stdout".into(),
                ));
            }
        };
        let mut client = HostClient::from_stream_with_transport(
            ChildStdio {
                reader: stdout,
                writer: stdin,
            },
            snapshot_transport,
        );
        if let Err(error) = client.hello(client_name).await {
            let _ = child.start_kill();
            let _ = child.wait().await;
            if let Some(capture) = stderr_capture {
                capture.finish().await;
            }
            return Err(error);
        }
        Ok(Self {
            client: Some(client),
            child: Some(child),
            stderr_capture,
        })
    }

    /// Access the negotiated client for Host requests.
    pub fn client_mut(&mut self) -> &mut HostClient {
        self.client
            .as_mut()
            .expect("HostProcess client is available until shutdown")
    }

    /// Return the spawned Host process id while it is still running.
    #[must_use]
    pub fn id(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::id)
    }

    /// Report whether the owned Host child is still running.
    pub fn is_running(&mut self) -> HostClientResult<bool> {
        let Some(child) = self.child.as_mut() else {
            return Ok(false);
        };
        Ok(child.try_wait()?.is_none())
    }

    /// Stop this Host and start a fresh negotiated process.
    ///
    /// Requests are never replayed. Callers must establish fresh sessions and
    /// observations after a restart because the previous process state is gone.
    pub async fn restart(
        self,
        binary_path: impl AsRef<Path>,
        client_name: impl Into<String>,
        snapshot_transport: SnapshotTransport,
    ) -> HostClientResult<Self> {
        let _ = self.shutdown().await?;
        Self::spawn(binary_path, client_name, snapshot_transport).await
    }

    /// Close stdio gracefully, then force-stop a Host that does not exit.
    pub async fn shutdown(mut self) -> HostClientResult<ExitStatus> {
        drop(self.client.take());
        let mut child = self.child.take().ok_or_else(|| {
            HostClientError::Protocol("Host process was already shut down".into())
        })?;
        let status = match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
            Ok(status) => status.map_err(HostClientError::from),
            Err(_) => match child.kill().await {
                Ok(()) => child.wait().await.map_err(HostClientError::from),
                Err(error) => Err(HostClientError::from(error)),
            },
        };
        if let Some(capture) = self.stderr_capture.take() {
            capture.finish().await;
        }
        status
    }
}

fn configure_host_process(_command: &mut Command) {
    #[cfg(windows)]
    {
        _command.creation_flags(HOST_CREATE_NO_WINDOW);
    }
}

#[cfg(windows)]
const HOST_CREATE_NO_WINDOW: u32 = 0x0800_0000;

impl Drop for HostProcess {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
        if let Some(capture) = self.stderr_capture.take() {
            capture.task.abort();
        }
    }
}

impl fmt::Debug for HostClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostClient")
            .field("next_request_id", &self.next_request_id)
            .field("hello_complete", &self.hello_complete)
            .field("snapshot_transport", &self.snapshot_transport)
            .field("capability_count", &self.capabilities.len())
            .field(
                "connection_usable",
                &(self.connection_state.load(Ordering::Acquire) == CONNECTION_READY),
            )
            .finish_non_exhaustive()
    }
}

impl HostClient {
    fn begin_request_operation(&self) -> HostClientResult<RequestOperationGuard> {
        match self.connection_state.compare_exchange(
            CONNECTION_READY,
            CONNECTION_IN_FLIGHT,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(RequestOperationGuard {
                state: Arc::clone(&self.connection_state),
                complete: false,
            }),
            Err(CONNECTION_UNUSABLE) => Err(HostClientError::Protocol(
                "Host connection is unusable after an interrupted or incomplete request; reconnect before sending another request".into(),
            )),
            Err(_) => Err(HostClientError::Protocol(
                "another Host request is already in flight on this connection".into(),
            )),
        }
    }

    fn finish_request_operation<T>(
        guard: RequestOperationGuard,
        result: HostClientResult<T>,
    ) -> HostClientResult<T> {
        if result.is_ok() || matches!(&result, Err(HostClientError::Remote { .. })) {
            guard.complete();
        }
        result
    }

    /// Connect to the platform-default per-user Host endpoint and negotiate it.
    pub async fn connect_default(client_name: impl Into<String>) -> HostClientResult<Self> {
        Self::connect(Self::default_endpoint(), client_name).await
    }

    /// Connect to the platform-default endpoint with an explicit image transport.
    pub async fn connect_default_with_transport(
        client_name: impl Into<String>,
        snapshot_transport: SnapshotTransport,
    ) -> HostClientResult<Self> {
        Self::connect_with_transport(Self::default_endpoint(), client_name, snapshot_transport)
            .await
    }

    /// Connect to an endpoint and complete the mandatory Host handshake.
    pub async fn connect(
        endpoint: impl Into<String>,
        client_name: impl Into<String>,
    ) -> HostClientResult<Self> {
        Self::connect_with_transport(endpoint, client_name, SnapshotTransport::BinaryFrame).await
    }

    /// Connect to an endpoint and select binary frames or shared memory for images.
    pub async fn connect_with_transport(
        endpoint: impl Into<String>,
        client_name: impl Into<String>,
        snapshot_transport: SnapshotTransport,
    ) -> HostClientResult<Self> {
        let stream = connect_endpoint(&endpoint.into()).await?;
        let mut client = Self::from_stream_with_transport(stream, snapshot_transport);
        client.hello(client_name).await?;
        Ok(client)
    }

    /// Wrap an already-connected stream. This is useful for stdio bridges and
    /// tests; the caller must call [`Self::hello`] before other requests.
    pub fn from_stream<S>(stream: S) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Self::from_stream_with_transport(stream, SnapshotTransport::BinaryFrame)
    }

    pub fn from_stream_with_transport<S>(stream: S, snapshot_transport: SnapshotTransport) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (reader, writer) = tokio::io::split(Box::new(stream) as BoxedHostStream);
        Self {
            reader,
            writer,
            pending_responses: HashMap::new(),
            outstanding_request_ids: HashSet::new(),
            next_request_id: 1,
            hello_complete: false,
            snapshot_transport,
            capabilities: Vec::new(),
            connection_state: Arc::new(AtomicU8::new(CONNECTION_READY)),
        }
    }

    #[must_use]
    pub fn default_endpoint() -> String {
        dcc_cua_protocol::default_endpoint()
    }

    /// Negotiate the protocol and preferred snapshot transport.
    pub async fn hello(
        &mut self,
        client_name: impl Into<String>,
    ) -> HostClientResult<HostResponse> {
        if self.hello_complete {
            return Err(HostClientError::Protocol(
                "hello has already completed on this connection".into(),
            ));
        }
        let response = self
            .request_inner(
                "hello",
                json!({
                "protocol_version": HOST_PROTOCOL_VERSION,
                    "client_name": client_name.into(),
                "snapshot_transport": self.snapshot_transport.as_wire_name(),
                }),
            )
            .await?;
        if response.value["type"] != "hello" {
            return Err(HostClientError::Protocol(
                "Host hello response has an unexpected type".into(),
            ));
        }
        self.capabilities = response_capabilities(&response.value)?;
        self.hello_complete = true;
        Ok(response)
    }

    /// Return the capabilities advertised by the negotiated Host.
    #[must_use]
    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    /// Check one capability without coupling Core to the wire JSON shape.
    #[must_use]
    pub fn supports_capability(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|value| value == capability)
    }

    /// Cheap Host liveness check that does not call into the native CUA backend.
    pub async fn ping(&mut self) -> HostClientResult<HostResponse> {
        self.request("ping", json!({})).await
    }

    /// Probe the readiness of this negotiated Host and its embedded CUA runtime.
    pub async fn doctor(&mut self) -> HostClientResult<HostResponse> {
        self.request("doctor", json!({})).await
    }

    /// Cooperatively stop every active session owned by this Host process.
    pub async fn interrupt_all(&mut self) -> HostClientResult<HostResponse> {
        self.request("interrupt_all", json!({})).await
    }

    /// Open one persistent window session for one logical task.
    ///
    /// This consumes the negotiated client so the returned task object is the
    /// only owner of the connection and its connection-scoped capability.
    pub async fn open_logical_task_session(
        mut self,
        session_id: impl Into<String>,
        grant: Value,
        idle_timeout_ms: u64,
    ) -> HostClientResult<LogicalTaskSession> {
        if !(MIN_SESSION_IDLE_TIMEOUT_MS..=MAX_SESSION_IDLE_TIMEOUT_MS).contains(&idle_timeout_ms) {
            return Err(HostClientError::Protocol(format!(
                "idle_timeout_ms must be between {MIN_SESSION_IDLE_TIMEOUT_MS} and {MAX_SESSION_IDLE_TIMEOUT_MS}"
            )));
        }
        let session_id = session_id.into();
        let task_grant_id = grant
            .get("task_grant_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                HostClientError::Protocol(
                    "logical task grant requires a non-empty task_grant_id".into(),
                )
            })?
            .to_owned();
        let response = self
            .request(
                "open_session",
                json!({
                    "session_id": session_id,
                    "grant": grant,
                    "idle_timeout_ms": idle_timeout_ms,
                }),
            )
            .await?;
        let returned_session_id = response.value["session_id"].as_str().ok_or_else(|| {
            HostClientError::Protocol("open_session response omitted session_id".into())
        })?;
        if returned_session_id != session_id {
            return Err(HostClientError::Protocol(
                "open_session response changed the logical task session id".into(),
            ));
        }
        let window_capability = response.value["window_capability"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                HostClientError::Protocol("open_session response omitted window_capability".into())
            })?
            .to_owned();
        let target = response
            .value
            .get("target")
            .filter(|value| value.is_object())
            .cloned()
            .ok_or_else(|| {
                HostClientError::Protocol("open_session response omitted target identity".into())
            })?;
        if target
            .get("process_id")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
            .is_none()
            || target
                .get("window_handle")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0)
                .is_none()
        {
            return Err(HostClientError::Protocol(
                "open_session response target identity is not an exact PID/HWND".into(),
            ));
        }
        Ok(LogicalTaskSession {
            client: self,
            session_id,
            task_grant_id,
            window_capability,
            target,
            idle_timeout_ms,
        })
    }

    /// Send one request and read its JSON response plus an optional image frame.
    pub async fn request(
        &mut self,
        method: impl Into<String>,
        params: Value,
    ) -> HostClientResult<HostResponse> {
        if !self.hello_complete {
            return Err(HostClientError::Protocol(
                "hello must complete before stateful requests".into(),
            ));
        }
        self.request_inner(&method.into(), params).await
    }

    /// Send one bounded request.
    ///
    /// A timeout can occur after part of a frame was transferred, so the
    /// connection fails closed and must be replaced instead of risking frame
    /// or request-correlation reuse.
    pub async fn request_with_timeout(
        &mut self,
        method: impl Into<String>,
        params: Value,
        timeout: Duration,
    ) -> HostClientResult<HostResponse> {
        if !self.hello_complete {
            return Err(HostClientError::Protocol(
                "hello must complete before stateful requests".into(),
            ));
        }
        match tokio::time::timeout(timeout, self.request_inner(&method.into(), params)).await {
            Ok(result) => result,
            Err(_) => Err(HostClientError::Timeout {
                timeout_ms: timeout.as_millis(),
            }),
        }
    }

    /// Send one request with a caller-owned correlation id.
    ///
    /// The id is echoed by the Host on both success and error responses. This
    /// is the Core-facing path for preserving task/turn tracing across a
    /// long-lived connection; [`Self::request`] remains convenient for
    /// callers that do not need an external id.
    pub async fn request_with_id(
        &mut self,
        request_id: impl Into<String>,
        method: impl Into<String>,
        params: Value,
    ) -> HostClientResult<HostResponse> {
        if !self.hello_complete {
            return Err(HostClientError::Protocol(
                "hello must complete before stateful requests".into(),
            ));
        }
        let request_id = request_id.into();
        validate_request_id(&request_id)?;
        self.request_inner_with_id(&request_id, &method.into(), params)
            .await
    }

    /// Send one bounded request with a caller-owned correlation id.
    pub async fn request_with_id_and_timeout(
        &mut self,
        request_id: impl Into<String>,
        method: impl Into<String>,
        params: Value,
        timeout: Duration,
    ) -> HostClientResult<HostResponse> {
        if !self.hello_complete {
            return Err(HostClientError::Protocol(
                "hello must complete before stateful requests".into(),
            ));
        }
        let request_id = request_id.into();
        validate_request_id(&request_id)?;
        match tokio::time::timeout(
            timeout,
            self.request_inner_with_id(&request_id, &method.into(), params),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(HostClientError::Timeout {
                timeout_ms: timeout.as_millis(),
            }),
        }
    }

    /// Send a sequence of read-only requests in one flushed write and return
    /// responses in the same order. Stateful mutations intentionally remain
    /// single-request so a failed operation cannot hide later side effects.
    pub async fn request_batch(
        &mut self,
        requests: impl IntoIterator<Item = (String, Value)>,
    ) -> HostClientResult<Vec<HostResponse>> {
        if !self.hello_complete {
            return Err(HostClientError::Protocol(
                "hello must complete before stateful requests".into(),
            ));
        }
        let requests = requests
            .into_iter()
            .map(|(method, params)| (self.next_request_id(), method, params))
            .collect::<Vec<_>>();
        self.request_batch_with_ids(requests).await
    }

    /// Send a sequence of read-only requests with caller-owned correlation ids
    /// in one flushed write and return responses in the same order.
    pub async fn request_batch_with_ids(
        &mut self,
        requests: impl IntoIterator<Item = (String, String, Value)>,
    ) -> HostClientResult<Vec<HostResponse>> {
        let responses = self.request_batch_with_ids_all(requests).await?;
        responses.into_iter().collect()
    }

    /// Send read-only requests in one flushed write and drain every response,
    /// including remote errors, in caller order.
    pub async fn request_batch_with_ids_all(
        &mut self,
        requests: impl IntoIterator<Item = (String, String, Value)>,
    ) -> HostClientResult<Vec<HostClientResult<HostResponse>>> {
        if !self.hello_complete {
            return Err(HostClientError::Protocol(
                "hello must complete before stateful requests".into(),
            ));
        }
        let requests = requests.into_iter().collect::<Vec<_>>();
        if requests.len() > MAX_PARALLEL_DISCOVERY_REQUESTS {
            return Err(HostClientError::Protocol(format!(
                "request_batch accepts at most {MAX_PARALLEL_DISCOVERY_REQUESTS} requests"
            )));
        }
        if requests
            .iter()
            .any(|(_, method, _)| !is_pipeline_safe_method(method))
        {
            return Err(HostClientError::Protocol(
                "request_batch accepts read-only Host methods only".into(),
            ));
        }
        validate_shared_memory_batch_handoffs(self.snapshot_transport, &requests)?;
        for (request_id, _, _) in &requests {
            validate_request_id(request_id)?;
        }
        let mut seen = HashSet::with_capacity(requests.len());
        if requests
            .iter()
            .any(|(request_id, _, _)| !seen.insert(request_id.as_str()))
        {
            return Err(HostClientError::Protocol(
                "request_batch does not allow duplicate request ids".into(),
            ));
        }
        let guard = self.begin_request_operation()?;
        let result = async {
            let mut request_ids = Vec::with_capacity(requests.len());
            for (request_id, method, params) in requests {
                request_ids.push(
                    self.send_request_unflushed_with_id(&request_id, &method, params)
                        .await?,
                );
            }
            if !request_ids.is_empty() {
                self.writer.flush().await?;
            }
            let mut received = Vec::with_capacity(request_ids.len());
            for request_id in request_ids {
                let response = self.receive_for_request_raw(&request_id).await?;
                received.push(response.into_result());
            }
            Ok(received)
        }
        .await;
        Self::finish_request_operation(guard, result)
    }

    /// Send a request while allowing the Host's same-connection cancellation
    /// route to terminate it. The cancellation parameters must contain the
    /// exact credentials required by the target request, such as `wait_for`.
    pub async fn request_with_cancel<C>(
        &mut self,
        method: impl Into<String>,
        params: Value,
        cancel_params: Value,
        cancel: C,
    ) -> HostClientResult<HostResponse>
    where
        C: Future<Output = ()>,
    {
        if !self.hello_complete {
            return Err(HostClientError::Protocol(
                "hello must complete before stateful requests".into(),
            ));
        }
        let guard = self.begin_request_operation()?;
        let result = async {
            let request_id = self.send_request(&method.into(), params).await?;
            tokio::pin!(cancel);
            tokio::select! {
                received = self.receive_for_request(&request_id) => {
                    received
                }
                _ = &mut cancel => {
                    let cancel_id = self.send_request("cancel", cancel_params).await?;
                    let cancel_result = self.receive_for_request(&cancel_id).await;
                    let request_result = self.receive_for_request(&request_id).await;
                    cancel_result?;
                    request_result
                }
            }
        }
        .await;
        Self::finish_request_operation(guard, result)
    }

    /// Wait for a native window while allowing a same-connection cancellation.
    /// The generated request id is used as the cancellation handle.
    pub async fn wait_for_window_with_cancel<C>(
        &mut self,
        params: Value,
        cancel: C,
    ) -> HostClientResult<HostResponse>
    where
        C: Future<Output = ()>,
    {
        if !self.hello_complete {
            return Err(HostClientError::Protocol(
                "hello must complete before stateful requests".into(),
            ));
        }
        let guard = self.begin_request_operation()?;
        let result = async {
            let request_id = self.send_request("wait_for_window", params).await?;
            tokio::pin!(cancel);
            tokio::select! {
                received = self.receive_for_request(&request_id) => received,
                _ = &mut cancel => {
                    let cancel_id = self
                        .send_request("cancel_window_wait", json!({"wait_id": request_id}))
                        .await?;
                    self.receive_for_request(&cancel_id).await?;
                    self.receive_for_request(&request_id).await
                }
            }
        }
        .await;
        Self::finish_request_operation(guard, result)
    }

    async fn request_inner(
        &mut self,
        method: &str,
        params: Value,
    ) -> HostClientResult<HostResponse> {
        let guard = self.begin_request_operation()?;
        let result = async {
            let request_id = self.send_request(method, params).await?;
            self.receive_for_request(&request_id).await
        }
        .await;
        Self::finish_request_operation(guard, result)
    }

    async fn request_inner_with_id(
        &mut self,
        request_id: &str,
        method: &str,
        params: Value,
    ) -> HostClientResult<HostResponse> {
        let guard = self.begin_request_operation()?;
        let result = async {
            let request_id = self
                .send_request_with_id(request_id, method, params)
                .await?;
            self.receive_for_request(&request_id).await
        }
        .await;
        Self::finish_request_operation(guard, result)
    }

    async fn receive_for_request(&mut self, request_id: &str) -> HostClientResult<HostResponse> {
        self.receive_for_request_raw(request_id)
            .await
            .and_then(ReceivedResponse::into_result)
    }

    async fn receive_for_request_raw(
        &mut self,
        request_id: &str,
    ) -> HostClientResult<ReceivedResponse> {
        if let Some(response) = self.pending_responses.remove(request_id) {
            self.outstanding_request_ids.remove(request_id);
            return Ok(response);
        }
        loop {
            let response = self.receive_response().await?;
            if !self
                .outstanding_request_ids
                .contains(response.request_id.as_str())
            {
                return Err(HostClientError::Protocol(format!(
                    "Host returned an untracked request_id: {}",
                    response.request_id
                )));
            }
            if response.request_id == request_id {
                self.outstanding_request_ids.remove(request_id);
                return Ok(response);
            }
            if self
                .pending_responses
                .insert(response.request_id.clone(), response)
                .is_some()
            {
                return Err(HostClientError::Protocol(
                    "Host returned a duplicate response for one request_id".into(),
                ));
            }
        }
    }

    async fn send_request(&mut self, method: &str, params: Value) -> HostClientResult<String> {
        let request_id = self.next_request_id();
        self.send_request_with_id(&request_id, method, params).await
    }

    fn next_request_id(&mut self) -> String {
        let request_id = format!("cua-client-{}", self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        request_id
    }

    async fn send_request_with_id(
        &mut self,
        request_id: &str,
        method: &str,
        params: Value,
    ) -> HostClientResult<String> {
        validate_request_id(request_id)?;
        let request_id = request_id.to_owned();
        self.register_outstanding_request(&request_id)?;
        let request = json!({
            "request_id": request_id,
            "method": method,
            "params": params,
        });
        let body = serde_json::to_vec(&request)
            .map_err(|error| HostClientError::Protocol(error.to_string()))?;
        write_frame_unflushed(&mut self.writer, &body, MAX_JSON_FRAME_BYTES).await?;
        self.writer.flush().await?;
        Ok(request_id)
    }

    async fn send_request_unflushed_with_id(
        &mut self,
        request_id: &str,
        method: &str,
        params: Value,
    ) -> HostClientResult<String> {
        validate_request_id(request_id)?;
        let request_id = request_id.to_owned();
        self.register_outstanding_request(&request_id)?;
        let request = json!({
            "request_id": request_id,
            "method": method,
            "params": params,
        });
        let body = serde_json::to_vec(&request)
            .map_err(|error| HostClientError::Protocol(error.to_string()))?;
        write_frame_unflushed(&mut self.writer, &body, MAX_JSON_FRAME_BYTES).await?;
        Ok(request_id)
    }

    fn register_outstanding_request(&mut self, request_id: &str) -> HostClientResult<()> {
        if self.outstanding_request_ids.len() >= MAX_PARALLEL_DISCOVERY_REQUESTS {
            return Err(HostClientError::Protocol(format!(
                "one client operation tracks at most {MAX_PARALLEL_DISCOVERY_REQUESTS} request ids"
            )));
        }
        if !self.outstanding_request_ids.insert(request_id.to_owned())
            || self.pending_responses.contains_key(request_id)
        {
            return Err(HostClientError::Protocol(format!(
                "request_id {request_id} is already in flight"
            )));
        }
        Ok(())
    }

    async fn receive_response(&mut self) -> HostClientResult<ReceivedResponse> {
        let response_body = read_frame(&mut self.reader, MAX_JSON_FRAME_BYTES)
            .await?
            .ok_or_else(|| HostClientError::Protocol("Host closed the connection".into()))?;
        let response: Value = serde_json::from_slice(&response_body)
            .map_err(|error| HostClientError::Protocol(error.to_string()))?;
        let binary_attachment = if let Some(expected_length) = binary_attachment_length(&response) {
            let attachment = read_frame(&mut self.reader, MAX_BINARY_FRAME_BYTES)
                .await?
                .ok_or_else(|| {
                    HostClientError::Protocol(
                        "Host advertised a binary attachment but closed the connection".into(),
                    )
                })?;
            if attachment.len() != expected_length {
                return Err(HostClientError::Protocol(format!(
                    "binary attachment length {} does not match advertised length {expected_length}",
                    attachment.len()
                )));
            }
            Some(attachment)
        } else {
            None
        };
        let request_id = response["request_id"]
            .as_str()
            .ok_or_else(|| HostClientError::Protocol("Host response omitted request_id".into()))?
            .to_owned();
        Ok(ReceivedResponse {
            request_id,
            value: response,
            binary_attachment,
        })
    }
}

fn bind_task_credentials(
    params: Value,
    session_id: &str,
    task_grant_id: &str,
    window_capability: &str,
) -> HostClientResult<Value> {
    let mut params = params.as_object().cloned().ok_or_else(|| {
        HostClientError::Protocol("logical task request params must be a JSON object".into())
    })?;
    for (key, expected) in [
        ("session_id", session_id),
        ("task_grant_id", task_grant_id),
        ("window_capability", window_capability),
    ] {
        if let Some(actual) = params.get(key) {
            if actual.as_str() != Some(expected) {
                return Err(HostClientError::Protocol(format!(
                    "logical task request cannot override {key}"
                )));
            }
        } else {
            params.insert(key.into(), Value::String(expected.to_owned()));
        }
    }
    Ok(Value::Object(params))
}

fn is_pipeline_safe_method(method: &str) -> bool {
    host_method_traits(method).pipeline_safe
}

fn validate_shared_memory_batch_handoffs(
    snapshot_transport: SnapshotTransport,
    requests: &[(String, String, Value)],
) -> HostClientResult<()> {
    if snapshot_transport != SnapshotTransport::SharedMemory {
        return Ok(());
    }
    let mut publishers = HashSet::new();
    for (_, method, params) in requests {
        let Some(slot) = shared_memory_handoff_slot(method, params) else {
            continue;
        };
        if !publishers.insert(slot) {
            return Err(HostClientError::Protocol(
                "request_batch allows at most one shared-memory image publisher per session".into(),
            ));
        }
    }
    Ok(())
}

fn shared_memory_handoff_slot(method: &str, params: &Value) -> Option<String> {
    let namespace = match method {
        "desktop_snapshot" => return Some("desktop:global".into()),
        "desktop_session_snapshot" => "desktop",
        "snapshot" | "verify_state" | "browser_snapshot" => "window",
        _ => return None,
    };
    params["session_id"]
        .as_str()
        .map(|session_id| format!("{namespace}:{session_id}"))
}

fn response_capabilities(response: &Value) -> HostClientResult<Vec<String>> {
    let Some(capabilities) = response.get("capabilities") else {
        return Ok(Vec::new());
    };
    capabilities
        .as_array()
        .ok_or_else(|| HostClientError::Protocol("Host capabilities must be an array".into()))?
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                HostClientError::Protocol("Host capability names must be strings".into())
            })
        })
        .collect()
}

/// Host methods that can execute concurrently without session or observation
/// state. Callers may batch the broader read-only set, but only these methods
/// use the Host's parallel dispatch path.
#[must_use]
pub fn is_parallel_discovery_method(method: &str) -> bool {
    host_method_traits(method).parallel_discovery
}

fn validate_request_id(request_id: &str) -> HostClientResult<()> {
    validate_protocol_request_id(request_id)
        .map_err(|error| HostClientError::Protocol(error.to_string()))
}

fn binary_attachment_length(response: &Value) -> Option<usize> {
    if let Some(attachments) = response["attachments"].as_array() {
        let mut total = 0;
        for attachment in attachments {
            if attachment["encoding"] != "binary_frame" {
                continue;
            }
            let offset = attachment["offset"].as_u64()? as usize;
            let length = attachment["length"].as_u64()? as usize;
            total = total.max(offset.checked_add(length)?);
        }
        return (total > 0).then_some(total);
    }
    (response["image"]["encoding"] == "binary_frame")
        .then(|| {
            response["image"]["length"]
                .as_u64()
                .map(|length| length as usize)
        })
        .flatten()
}

async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> HostClientResult<Option<Vec<u8>>> {
    dcc_cua_protocol::read_frame(reader, max)
        .await
        .map_err(frame_error)
}

async fn write_frame_unflushed<W: AsyncWrite + Unpin>(
    writer: &mut W,
    body: &[u8],
    max: usize,
) -> HostClientResult<()> {
    dcc_cua_protocol::write_frame_unflushed(writer, body, max)
        .await
        .map_err(frame_error)
}

fn frame_error(error: FrameError) -> HostClientError {
    match error {
        FrameError::Io(error) => HostClientError::Io(error),
        FrameError::Protocol(message) => HostClientError::Protocol(message),
    }
}

async fn connect_endpoint(endpoint: &str) -> HostClientResult<BoxedHostStream> {
    #[cfg(windows)]
    {
        let stream = tokio::net::windows::named_pipe::ClientOptions::new().open(endpoint)?;
        verify_named_pipe_server_identity(&stream)?;
        Ok(Box::new(stream))
    }
    #[cfg(unix)]
    {
        Ok(Box::new(tokio::net::UnixStream::connect(endpoint).await?))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = endpoint;
        Err(HostClientError::Protocol(
            "local endpoint transport is unsupported on this platform".into(),
        ))
    }
}

#[cfg(windows)]
fn verify_named_pipe_server_identity(
    stream: &tokio::net::windows::named_pipe::NamedPipeClient,
) -> HostClientResult<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::HANDLE,
        Security::{EqualSid, TOKEN_USER},
        System::{
            Pipes::GetNamedPipeServerProcessId,
            Threading::{GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        },
    };

    let mut server_process_id = 0_u32;
    if unsafe {
        GetNamedPipeServerProcessId(stream.as_raw_handle() as HANDLE, &mut server_process_id)
    } == 0
        || server_process_id == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let raw_server_process =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, server_process_id) };
    if raw_server_process.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    let server_process = unsafe { OwnedHandle::from_raw_handle(raw_server_process) };
    let server_user = process_user_token_buffer(server_process.as_raw_handle() as HANDLE)?;
    let current_user = process_user_token_buffer(unsafe { GetCurrentProcess() })?;
    let server_sid = unsafe { &*server_user.as_ptr().cast::<TOKEN_USER>() }
        .User
        .Sid;
    let current_sid = unsafe { &*current_user.as_ptr().cast::<TOKEN_USER>() }
        .User
        .Sid;
    if unsafe { EqualSid(server_sid, current_sid) } == 0 {
        return Err(HostClientError::Protocol(
            "named-pipe server owner does not match the current user".into(),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn process_user_token_buffer(
    process: windows_sys::Win32::Foundation::HANDLE,
) -> std::io::Result<Vec<usize>> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Security::{GetTokenInformation, IsValidSid, TOKEN_QUERY, TOKEN_USER, TokenUser},
        System::Threading::OpenProcessToken,
    };

    let mut raw_token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut raw_token) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(raw_token) };
    let mut required_bytes = 0_u32;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut required_bytes,
        )
    };
    if (required_bytes as usize) < std::mem::size_of::<TOKEN_USER>() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "process token returned no user SID",
        ));
    }
    let words = (required_bytes as usize).div_ceil(std::mem::size_of::<usize>());
    let mut buffer = vec![0_usize; words];
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required_bytes,
            &mut required_bytes,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let sid = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() }.User.Sid;
    if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "process token returned an invalid user SID",
        ));
    }
    Ok(buffer)
}

struct ChildStdio {
    reader: ChildStdout,
    writer: ChildStdin,
}

impl AsyncRead for ChildStdio {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.reader).poll_read(context, buffer)
    }
}

impl AsyncWrite for ChildStdio {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.writer).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.writer).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.writer).poll_shutdown(context)
    }
}

#[cfg(test)]
mod tests;
