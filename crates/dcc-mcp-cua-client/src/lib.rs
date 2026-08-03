//! Small client for the versioned local Computer Use Host protocol.
//!
//! The client deliberately exposes the protocol as JSON values so DCC-MCP Core
//! can own its higher-level task contracts without duplicating framing,
//! request correlation, or binary image handling.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    future::Future,
    path::Path,
    pin::Pin,
    process::{ExitStatus, Stdio},
    task::{Context, Poll},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, ReadHalf, WriteHalf};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

pub use dcc_mcp_cua_protocol::{
    HOST_PROTOCOL_VERSION, MAX_BINARY_FRAME_BYTES, MAX_JSON_FRAME_BYTES,
    MAX_PARALLEL_DISCOVERY_REQUESTS, MAX_REQUEST_ID_CHARS,
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
    #[error("host returned {code}: {message}")]
    Remote {
        code: String,
        message: String,
        response: Value,
    },
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

/// One multiplexed client connection to a DCC-MCP Computer Use Host.
pub struct HostClient {
    reader: ReadHalf<BoxedHostStream>,
    writer: WriteHalf<BoxedHostStream>,
    pending_responses: HashMap<String, ReceivedResponse>,
    next_request_id: u64,
    hello_complete: bool,
    snapshot_transport: SnapshotTransport,
    capabilities: Vec<String>,
}

/// A CUA Host child and its already-negotiated stdio client.
///
/// Core can use this when it owns the Host lifecycle. Endpoint connections
/// remain available for supervisors that launch the CLI separately.
pub struct HostProcess {
    client: Option<HostClient>,
    child: Option<Child>,
}

impl fmt::Debug for HostProcess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostProcess")
            .field("pid", &self.child.as_ref().and_then(Child::id))
            .field("client", &self.client)
            .finish()
    }
}

impl HostProcess {
    /// Spawn `dcc-mcp-cua host --stdio` and complete the Host handshake.
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
        let mut child = Command::new(binary_path.as_ref())
            .arg("host")
            .arg("--stdio")
            .args(host_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                let _ = child.start_kill();
                return Err(HostClientError::Protocol(
                    "spawned Host did not expose stdin".into(),
                ));
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                let _ = child.start_kill();
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
            return Err(error);
        }
        Ok(Self {
            client: Some(client),
            child: Some(child),
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
        match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
            Ok(status) => Ok(status?),
            Err(_) => {
                child.kill().await?;
                Ok(child.wait().await?)
            }
        }
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
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
            .finish_non_exhaustive()
    }
}

impl HostClient {
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
            next_request_id: 1,
            hello_complete: false,
            snapshot_transport,
            capabilities: Vec::new(),
        }
    }

    #[must_use]
    pub fn default_endpoint() -> String {
        dcc_mcp_cua_protocol::default_endpoint()
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

    async fn request_inner(
        &mut self,
        method: &str,
        params: Value,
    ) -> HostClientResult<HostResponse> {
        let request_id = self.send_request(method, params).await?;
        self.receive_for_request(&request_id).await
    }

    async fn request_inner_with_id(
        &mut self,
        request_id: &str,
        method: &str,
        params: Value,
    ) -> HostClientResult<HostResponse> {
        let request_id = self
            .send_request_with_id(request_id, method, params)
            .await?;
        self.receive_for_request(&request_id).await
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
            return Ok(response);
        }
        loop {
            let response = self.receive_response().await?;
            if response.request_id == request_id {
                return Ok(response);
            }
            self.pending_responses
                .insert(response.request_id.clone(), response);
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

fn is_pipeline_safe_method(method: &str) -> bool {
    matches!(
        method,
        "ping"
            | "list_apps"
            | "list_tools"
            | "list_windows"
            | "desktop_snapshot"
            | "screen_size"
            | "cursor_position"
            | "get_window_state"
            | "snapshot"
            | "accessibility_snapshot"
            | "verify_state"
            | "get_session_state"
            | "find"
            | "browser_snapshot"
            | "recording_state"
            | "clipboard_read"
            | "desktop_session_snapshot"
    )
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
    matches!(
        method,
        "ping" | "list_apps" | "list_tools" | "list_windows" | "screen_size" | "cursor_position"
    )
}

fn validate_request_id(request_id: &str) -> HostClientResult<()> {
    if request_id.is_empty() || request_id.chars().count() > MAX_REQUEST_ID_CHARS {
        return Err(HostClientError::Protocol(format!(
            "request id must contain 1..{MAX_REQUEST_ID_CHARS} characters"
        )));
    }
    Ok(())
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
    let mut prefix = [0_u8; 4];
    match reader.read_exact(&mut prefix).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > max {
        return Err(HostClientError::Protocol(format!(
            "frame length {length} exceeds the host limit"
        )));
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).await?;
    Ok(Some(body))
}

async fn write_frame_unflushed<W: AsyncWrite + Unpin>(
    writer: &mut W,
    body: &[u8],
    max: usize,
) -> HostClientResult<()> {
    if body.is_empty() || body.len() > max || body.len() > u32::MAX as usize {
        return Err(HostClientError::Protocol(
            "frame payload is outside the host limit".into(),
        ));
    }
    writer.write_all(&(body.len() as u32).to_be_bytes()).await?;
    writer.write_all(body).await?;
    Ok(())
}

async fn connect_endpoint(endpoint: &str) -> HostClientResult<BoxedHostStream> {
    #[cfg(windows)]
    {
        let stream = tokio::net::windows::named_pipe::ClientOptions::new().open(endpoint)?;
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
