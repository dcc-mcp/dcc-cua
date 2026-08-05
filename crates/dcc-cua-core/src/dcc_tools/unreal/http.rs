//! Minimal loopback HTTP/1.1 exchange for the Unreal Remote Control tool.
//! Deliberately not a general HTTP client: one request per connection
//! (`Connection: close`), Content-Length or read-to-EOF bodies only.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub(crate) struct HttpResponse {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
}

pub(crate) async fn roundtrip(
    mut stream: TcpStream,
    method: &str,
    path: &str,
    body: &[u8],
    max_reply_bytes: usize,
) -> std::io::Result<HttpResponse> {
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(request.as_bytes()).await?;
    stream.write_all(body).await?;

    let mut raw = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read]);
        if raw.len() > max_reply_bytes {
            return Err(std::io::Error::other(format!(
                "reply exceeds {max_reply_bytes} bytes"
            )));
        }
        if response_complete(&raw) {
            break;
        }
    }
    parse(&raw)
}

/// True once the headers arrived and Content-Length (when present) is met.
fn response_complete(raw: &[u8]) -> bool {
    let Some(body_start) = header_end(raw) else {
        return false;
    };
    match content_length(&raw[..body_start]) {
        Some(length) => raw.len() - body_start >= length,
        None => false,
    }
}

fn header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

fn content_length(headers: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(headers);
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            value.trim().parse().ok()
        } else {
            None
        }
    })
}

fn parse(raw: &[u8]) -> std::io::Result<HttpResponse> {
    let body_start = header_end(raw)
        .ok_or_else(|| std::io::Error::other("response headers were never terminated"))?;
    let headers = String::from_utf8_lossy(&raw[..body_start]);
    let status_line = headers
        .lines()
        .next()
        .ok_or_else(|| std::io::Error::other("empty response"))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| std::io::Error::other(format!("malformed status line: {status_line}")))?;
    let body = match content_length(&raw[..body_start]) {
        Some(length) => raw[body_start..]
            .get(..length)
            .unwrap_or(&raw[body_start..]),
        None => &raw[body_start..],
    };
    Ok(HttpResponse {
        status,
        body: body.to_vec(),
    })
}
