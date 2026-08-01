//! Small client for the versioned local Computer Use Host protocol.
//!
//! The client deliberately exposes the protocol as JSON values so DCC-MCP Core
//! can own its higher-level task contracts without duplicating framing,
//! request correlation, or binary image handling.

use std::fmt;

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

/// One ordered client connection to a DCC-MCP Computer Use Host.
pub struct HostClient {
    reader: ReadHalf<BoxedHostStream>,
    writer: WriteHalf<BoxedHostStream>,
    next_request_id: u64,
    hello_complete: bool,
}

impl fmt::Debug for HostClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostClient")
            .field("next_request_id", &self.next_request_id)
            .field("hello_complete", &self.hello_complete)
            .finish_non_exhaustive()
    }
}

impl HostClient {
    /// Connect to the platform-default per-user Host endpoint and negotiate it.
    pub async fn connect_default(client_name: impl Into<String>) -> HostClientResult<Self> {
        Self::connect(Self::default_endpoint(), client_name).await
    }

    /// Connect to an endpoint and complete the mandatory Host handshake.
    pub async fn connect(
        endpoint: impl Into<String>,
        client_name: impl Into<String>,
    ) -> HostClientResult<Self> {
        let stream = connect_endpoint(&endpoint.into()).await?;
        let mut client = Self::from_stream(stream);
        client.hello(client_name).await?;
        Ok(client)
    }

    /// Wrap an already-connected stream. This is useful for stdio bridges and
    /// tests; the caller must call [`Self::hello`] before other requests.
    pub fn from_stream<S>(stream: S) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (reader, writer) = tokio::io::split(Box::new(stream) as BoxedHostStream);
        Self {
            reader,
            writer,
            next_request_id: 1,
            hello_complete: false,
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
                    "snapshot_transport": "binary_frame",
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

    async fn request_inner(
        &mut self,
        method: &str,
        params: Value,
    ) -> HostClientResult<HostResponse> {
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
        write_frame(&mut self.writer, &body, MAX_JSON_FRAME_BYTES).await?;

        let response_body = read_frame(&mut self.reader, MAX_JSON_FRAME_BYTES)
            .await?
            .ok_or_else(|| HostClientError::Protocol("Host closed the connection".into()))?;
        let response: Value = serde_json::from_slice(&response_body)
            .map_err(|error| HostClientError::Protocol(error.to_string()))?;
        if response["request_id"] != request_id {
            return Err(HostClientError::Protocol(
                "Host response request_id does not match the request".into(),
            ));
        }
        if response["type"] == "error" {
            return Err(HostClientError::Remote {
                code: response["code"].as_str().unwrap_or("host_error").to_owned(),
                message: response["message"]
                    .as_str()
                    .unwrap_or("Host returned an error")
                    .to_owned(),
                response,
            });
        }
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
        Ok(HostResponse {
            value: response,
            binary_attachment,
        })
    }
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

async fn write_frame<W: AsyncWrite + Unpin>(
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
    writer.flush().await?;
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
