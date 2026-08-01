//! Small client for the versioned local Computer Use Host protocol.
//!
//! The client deliberately exposes the protocol as JSON values so DCC-MCP Core
//! can own its higher-level task contracts without duplicating framing,
//! request correlation, or binary image handling.

use std::{fmt, future::Future};

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};

pub const HOST_PROTOCOL_VERSION: u32 = 1;
pub const MAX_JSON_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_BINARY_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_REQUEST_ID_CHARS: usize = 128;

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

/// One ordered client connection to a DCC-MCP Computer Use Host.
pub struct HostClient {
    reader: ReadHalf<BoxedHostStream>,
    writer: WriteHalf<BoxedHostStream>,
    next_request_id: u64,
    hello_complete: bool,
    snapshot_transport: SnapshotTransport,
}

impl fmt::Debug for HostClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostClient")
            .field("next_request_id", &self.next_request_id)
            .field("hello_complete", &self.hello_complete)
            .field("snapshot_transport", &self.snapshot_transport)
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
            next_request_id: 1,
            hello_complete: false,
            snapshot_transport,
        }
    }

    #[must_use]
    pub fn default_endpoint() -> String {
        #[cfg(windows)]
        {
            let mut session_id = 0;
            let resolved = unsafe {
                windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(
                    windows_sys::Win32::System::Threading::GetCurrentProcessId(),
                    &mut session_id,
                ) != 0
            };
            if resolved {
                return format!(r"\\.\pipe\dcc-mcp-cua-v1-session-{session_id}");
            }
            return r"\\.\pipe\dcc-mcp-cua-v1".to_owned();
        }
        #[cfg(unix)]
        {
            return std::env::temp_dir()
                .join("dcc-mcp-cua-v1.sock")
                .to_string_lossy()
                .into_owned();
        }
        #[cfg(not(any(windows, unix)))]
        {
            "dcc-mcp-cua-v1".to_owned()
        }
    }

    /// Negotiate the protocol and preferred snapshot transport.
    pub async fn hello(
        &mut self,
        client_name: impl Into<String>,
    ) -> HostClientResult<HostResponse> {
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
        self.hello_complete = true;
        Ok(response)
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
        let requests = requests.into_iter().collect::<Vec<_>>();
        if requests
            .iter()
            .any(|(method, _)| !is_pipeline_safe_method(method))
        {
            return Err(HostClientError::Protocol(
                "request_batch accepts read-only Host methods only".into(),
            ));
        }
        let mut request_ids = Vec::with_capacity(requests.len());
        for (method, params) in requests {
            request_ids.push(self.send_request_unflushed(&method, params).await?);
        }
        if !request_ids.is_empty() {
            self.writer.flush().await?;
        }
        let mut received = Vec::with_capacity(request_ids.len());
        for request_id in request_ids {
            let response = self.receive_response().await?;
            ensure_request_id(&response, &request_id)?;
            received.push(response);
        }
        received
            .into_iter()
            .map(ReceivedResponse::into_result)
            .collect()
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
            received = self.receive_response() => {
                let received = received?;
                ensure_request_id(&received, &request_id)?;
                received.into_result()
            }
            _ = &mut cancel => {
                let cancel_id = self.send_request("cancel", cancel_params).await?;
                let first = self.receive_response().await?;
                if first.request_id == request_id {
                    let result = first.into_result()?;
                    let cancel_response = self.receive_response().await?;
                    ensure_request_id(&cancel_response, &cancel_id)?;
                    return Ok(result);
                }
                ensure_request_id(&first, &cancel_id)?;
                let cancel_result = first.into_result();
                let terminal = self.receive_response().await?;
                ensure_request_id(&terminal, &request_id)?;
                let result = terminal.into_result()?;
                cancel_result?;
                Ok(result)
            }
        }
    }

    async fn request_inner(
        &mut self,
        method: &str,
        params: Value,
    ) -> HostClientResult<HostResponse> {
        let request_id = self.send_request(method, params).await?;
        let response = self.receive_response().await?;
        ensure_request_id(&response, &request_id)?;
        response.into_result()
    }

    async fn send_request(&mut self, method: &str, params: Value) -> HostClientResult<String> {
        let request_id = self.send_request_unflushed(method, params).await?;
        self.writer.flush().await?;
        Ok(request_id)
    }

    async fn send_request_unflushed(
        &mut self,
        method: &str,
        params: Value,
    ) -> HostClientResult<String> {
        let request_id = format!("cua-client-{}", self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        if request_id.chars().count() > MAX_REQUEST_ID_CHARS {
            return Err(HostClientError::Protocol(
                "request id exceeds host limit".into(),
            ));
        }
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
        "list_apps"
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

fn ensure_request_id(response: &ReceivedResponse, expected: &str) -> HostClientResult<()> {
    if response.request_id != expected {
        return Err(HostClientError::Protocol(
            "Host response request_id does not match the request".into(),
        ));
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

#[cfg(test)]
async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    body: &[u8],
    max: usize,
) -> HostClientResult<()> {
    write_frame_unflushed(writer, body, max).await?;
    writer.flush().await?;
    Ok(())
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
        return Ok(Box::new(stream));
    }
    #[cfg(unix)]
    {
        return Ok(Box::new(tokio::net::UnixStream::connect(endpoint).await?));
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = endpoint;
        Err(HostClientError::Protocol(
            "local endpoint transport is unsupported on this platform".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::DuplexStream;

    #[tokio::test]
    async fn client_negotiates_and_reads_binary_attachment() {
        let (client_stream, server_stream) = tokio::io::duplex(4096);
        let server = tokio::spawn(fake_server(server_stream));
        let mut client = HostClient::from_stream(client_stream);

        let hello = client.hello("test-client").await.unwrap();
        assert_eq!(hello.value["type"], "hello");
        let response = client.request("desktop_snapshot", json!({})).await.unwrap();
        assert_eq!(response.value["type"], "desktop_snapshot");
        assert_eq!(
            response.binary_attachment.as_deref(),
            Some(b"png".as_slice())
        );
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn client_rejects_requests_before_hello() {
        let (client_stream, _server_stream) = tokio::io::duplex(64);
        let mut client = HostClient::from_stream(client_stream);
        assert!(matches!(
            client.request("list_apps", json!({})).await,
            Err(HostClientError::Protocol(message)) if message.contains("hello")
        ));
    }

    #[tokio::test]
    async fn client_can_negotiate_shared_memory_images() {
        let (client_stream, server_stream) = tokio::io::duplex(4096);
        let server = tokio::spawn(fake_hello_only_server(server_stream));
        let mut client =
            HostClient::from_stream_with_transport(client_stream, SnapshotTransport::SharedMemory);
        let hello = client.hello("shared-memory-client").await.unwrap();
        assert_eq!(hello.value["snapshot_transport"], "shared_memory");
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn client_pipelines_read_only_requests_in_order() {
        let (client_stream, server_stream) = tokio::io::duplex(4096);
        let server = tokio::spawn(fake_batch_server(server_stream));
        let mut client = HostClient::from_stream(client_stream);
        client.hello("batch-client").await.unwrap();

        let responses = client
            .request_batch(vec![
                ("screen_size".into(), json!({})),
                ("cursor_position".into(), json!({})),
            ])
            .await
            .unwrap();

        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0].value["type"], "screen_size");
        assert_eq!(responses[1].value["type"], "cursor_position");
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn client_rejects_mutations_from_request_batch() {
        let (client_stream, server_stream) = tokio::io::duplex(4096);
        let server = tokio::spawn(fake_hello_only_server(server_stream));
        let mut client =
            HostClient::from_stream_with_transport(client_stream, SnapshotTransport::SharedMemory);
        client.hello("batch-client").await.unwrap();

        assert!(matches!(
            client
                .request_batch(vec![("execute_action".into(), json!({}))])
                .await,
            Err(HostClientError::Protocol(message))
                if message.contains("read-only")
        ));
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn client_can_cancel_wait_on_the_same_connection() {
        let (client_stream, server_stream) = tokio::io::duplex(4096);
        let server = tokio::spawn(fake_cancel_server(server_stream));
        let mut client = HostClient::from_stream(client_stream);
        client.hello("cancel-client").await.unwrap();
        let response = client
            .request_with_cancel(
                "wait_for",
                json!({"session_id":"s"}),
                json!({
                    "session_id":"s",
                    "task_grant_id":"grant",
                    "window_capability":"cap"
                }),
                async {},
            )
            .await
            .unwrap();
        assert_eq!(response.value["type"], "wait_cancelled");
        server.await.unwrap().unwrap();
    }

    async fn fake_server(mut stream: DuplexStream) -> HostClientResult<()> {
        let hello = read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
            .await?
            .unwrap();
        let hello: Value = serde_json::from_slice(&hello).unwrap();
        write_json_response(
            &mut stream,
            hello["request_id"].as_str().unwrap(),
            json!({"type":"hello"}),
        )
        .await?;

        let snapshot = read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
            .await?
            .unwrap();
        let snapshot: Value = serde_json::from_slice(&snapshot).unwrap();
        write_json_response(
            &mut stream,
            snapshot["request_id"].as_str().unwrap(),
            json!({
                "type":"desktop_snapshot",
                "image":{"encoding":"binary_frame","length":3}
            }),
        )
        .await?;
        write_frame(&mut stream, b"png", MAX_BINARY_FRAME_BYTES).await
    }

    async fn fake_hello_only_server(mut stream: DuplexStream) -> HostClientResult<()> {
        let hello = read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
            .await?
            .unwrap();
        let hello: Value = serde_json::from_slice(&hello).unwrap();
        assert_eq!(hello["params"]["snapshot_transport"], "shared_memory");
        write_json_response(
            &mut stream,
            hello["request_id"].as_str().unwrap(),
            json!({"type":"hello","snapshot_transport":"shared_memory"}),
        )
        .await
    }

    async fn fake_batch_server(mut stream: DuplexStream) -> HostClientResult<()> {
        let hello = read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
            .await?
            .unwrap();
        let hello: Value = serde_json::from_slice(&hello).unwrap();
        write_json_response(
            &mut stream,
            hello["request_id"].as_str().unwrap(),
            json!({"type":"hello"}),
        )
        .await?;

        let first: Value = serde_json::from_slice(
            &read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
                .await?
                .unwrap(),
        )
        .unwrap();
        let second: Value = serde_json::from_slice(
            &read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
                .await?
                .unwrap(),
        )
        .unwrap();
        assert_eq!(first["method"], "screen_size");
        assert_eq!(second["method"], "cursor_position");
        write_json_response(
            &mut stream,
            first["request_id"].as_str().unwrap(),
            json!({"type":"screen_size"}),
        )
        .await?;
        write_json_response(
            &mut stream,
            second["request_id"].as_str().unwrap(),
            json!({"type":"cursor_position"}),
        )
        .await
    }

    async fn fake_cancel_server(mut stream: DuplexStream) -> HostClientResult<()> {
        let hello = read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
            .await?
            .unwrap();
        let hello: Value = serde_json::from_slice(&hello).unwrap();
        write_json_response(
            &mut stream,
            hello["request_id"].as_str().unwrap(),
            json!({"type":"hello"}),
        )
        .await?;

        let first = read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
            .await?
            .unwrap();
        let second = read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
            .await?
            .unwrap();
        let first: Value = serde_json::from_slice(&first).unwrap();
        let second: Value = serde_json::from_slice(&second).unwrap();
        let requests = [first, second];
        let wait_id = requests
            .iter()
            .find(|request| request["method"] == "wait_for")
            .and_then(|request| request["request_id"].as_str())
            .unwrap();
        let cancel_id = requests
            .iter()
            .find(|request| request["method"] == "cancel")
            .and_then(|request| request["request_id"].as_str())
            .unwrap();
        write_json_response(
            &mut stream,
            cancel_id,
            json!({"type":"wait_cancel_requested"}),
        )
        .await?;
        write_json_response(&mut stream, wait_id, json!({"type":"wait_cancelled"})).await
    }

    async fn write_json_response(
        stream: &mut DuplexStream,
        request_id: &str,
        mut value: Value,
    ) -> HostClientResult<()> {
        value["request_id"] = Value::String(request_id.to_owned());
        let body = serde_json::to_vec(&value).unwrap();
        write_frame(stream, &body, MAX_JSON_FRAME_BYTES).await
    }
}
