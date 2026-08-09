use std::time::Duration;

use dcc_cua_semantic_profiles::StateSource;
use reqwest::header::{ACCEPT, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use super::{flag_value, load_semantic_profile};

#[derive(Debug, Serialize)]
pub(crate) struct StateObservation {
    pub source_id: String,
    pub schema_version: String,
    pub tick: Value,
    pub etag: Option<String>,
    pub state: Value,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum StateRead {
    Changed(StateObservation),
    NotModified {
        source_id: String,
        etag: Option<String>,
    },
}

pub(crate) struct StateWatcher<'a> {
    source: &'a StateSource,
    etag: Option<String>,
}

impl<'a> StateWatcher<'a> {
    pub(crate) fn new(source: &'a StateSource, etag: Option<String>) -> Self {
        Self { source, etag }
    }

    pub(crate) fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    pub(crate) async fn poll(&mut self) -> Result<StateRead, ProfileStateError> {
        let read = observe_source(self.source, self.etag()).await?;
        match &read {
            StateRead::Changed(observation) => {
                if observation.etag.is_some() {
                    self.etag.clone_from(&observation.etag);
                }
            }
            StateRead::NotModified { etag, .. } => {
                if etag.is_some() {
                    self.etag.clone_from(etag);
                }
            }
        }
        Ok(read)
    }
}

#[derive(Debug, Error)]
pub(crate) enum ProfileStateError {
    #[error("state source request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("state source returned HTTP {0}")]
    HttpStatus(u16),
    #[error("state source did not return application/json")]
    InvalidContentType,
    #[error("state source response exceeds {0} bytes")]
    ResponseTooLarge(u64),
    #[error("state source returned invalid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("state source is missing {0}")]
    MissingContractField(&'static str),
    #[error("state source schema version {actual:?} does not match expected {expected:?}")]
    SchemaMismatch { expected: String, actual: String },
    #[error("state source ETag is not a valid HTTP header: {0}")]
    InvalidEtag(String),
}

pub(crate) async fn observe_source(
    source: &StateSource,
    previous_etag: Option<&str>,
) -> Result<StateRead, ProfileStateError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(source.timeout_ms))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut request = client.get(&source.url).header(ACCEPT, "application/json");
    if source.use_etag
        && let Some(etag) = previous_etag
    {
        let value = reqwest::header::HeaderValue::from_str(etag)
            .map_err(|_| ProfileStateError::InvalidEtag(etag.to_owned()))?;
        request = request.header(IF_NONE_MATCH, value);
    }
    let mut response = request.send().await?;
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(StateRead::NotModified {
            source_id: source.id.clone(),
            etag,
        });
    }
    if response.status() != reqwest::StatusCode::OK {
        return Err(ProfileStateError::HttpStatus(response.status().as_u16()));
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !content_type
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
    {
        return Err(ProfileStateError::InvalidContentType);
    }
    if response
        .content_length()
        .is_some_and(|length| length > source.max_response_bytes)
    {
        return Err(ProfileStateError::ResponseTooLarge(
            source.max_response_bytes,
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > source.max_response_bytes as usize {
            return Err(ProfileStateError::ResponseTooLarge(
                source.max_response_bytes,
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let state = serde_json::from_slice::<Value>(&bytes)?;
    let schema_version = state
        .pointer(&source.schema_version_pointer)
        .and_then(Value::as_str)
        .ok_or(ProfileStateError::MissingContractField("schema version"))?
        .to_owned();
    if schema_version != source.expected_schema_version {
        return Err(ProfileStateError::SchemaMismatch {
            expected: source.expected_schema_version.clone(),
            actual: schema_version,
        });
    }
    let tick = state
        .pointer(&source.tick_pointer)
        .filter(|value| !value.is_null())
        .cloned()
        .ok_or(ProfileStateError::MissingContractField("tick"))?;
    Ok(StateRead::Changed(StateObservation {
        source_id: source.id.clone(),
        schema_version: source.expected_schema_version.clone(),
        tick,
        etag,
        state,
    }))
}

pub(crate) async fn execute(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let profile = load_semantic_profile(flags)?;
    let source = if let Some(id) = flag_value(flags, "--source") {
        profile
            .state_source(&id)
            .ok_or_else(|| format!("profile {:?} has no state source {id:?}", profile.id))?
    } else if profile.state_sources.len() == 1 {
        &profile.state_sources[0]
    } else {
        return Err(format!(
            "profile {:?} has {} state sources; select one with --source ID",
            profile.id,
            profile.state_sources.len()
        )
        .into());
    };
    if flags.iter().any(|flag| flag == "--watch") {
        return watch_source(&profile.id, source, flags).await;
    }
    match observe_source(source, flag_value(flags, "--etag").as_deref()).await {
        Ok(observation) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "success": true,
                    "profile_id": profile.id,
                    "source": observation,
                }))?
            );
            Ok(())
        }
        Err(error) if source.optional => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "success": false,
                    "degraded": true,
                    "profile_id": profile.id,
                    "source_id": source.id,
                    "fallback": "visual_cua",
                    "error": error.to_string(),
                }))?
            );
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

async fn watch_source(
    profile_id: &str,
    source: &StateSource,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let poll_ms = flag_value(flags, "--poll-ms")
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(250);
    if !(50..=60_000).contains(&poll_ms) {
        return Err("--poll-ms must be between 50 and 60000".into());
    }
    let max_updates = flag_value(flags, "--max-updates")
        .map(|value| value.parse::<u64>())
        .transpose()?;
    if max_updates == Some(0) {
        return Err("--max-updates must be greater than zero".into());
    }

    let mut watcher = StateWatcher::new(source, flag_value(flags, "--etag"));
    let mut emitted = 0_u64;
    let mut last_degraded_error = None::<String>;
    loop {
        match watcher.poll().await {
            Ok(StateRead::Changed(observation)) => {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "success": true,
                        "profile_id": profile_id,
                        "source": StateRead::Changed(observation),
                    }))?
                );
                last_degraded_error = None;
                emitted += 1;
                if max_updates.is_some_and(|maximum| emitted >= maximum) {
                    return Ok(());
                }
            }
            Ok(StateRead::NotModified { .. }) => {}
            Err(error) if source.optional => {
                let error = error.to_string();
                if last_degraded_error.as_deref() != Some(error.as_str()) {
                    println!(
                        "{}",
                        serde_json::to_string(&serde_json::json!({
                            "success": false,
                            "degraded": true,
                            "profile_id": profile_id,
                            "source_id": source.id,
                            "fallback": "visual_cua",
                            "error": error,
                        }))?
                    );
                    last_degraded_error = Some(error);
                }
            }
            Err(error) => return Err(error.into()),
        }
        tokio::time::sleep(Duration::from_millis(poll_ms)).await;
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use dcc_cua_semantic_profiles::parse_profile;
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn observes_versioned_tick_state_with_etag_from_a_loopback_source() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback server");
        let port = listener.local_addr().expect("listener address").port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0_u8; 2048];
            let read = stream.read(&mut request).expect("read request");
            assert!(String::from_utf8_lossy(&request[..read]).starts_with("GET /v1/context "));
            let body = r#"{"schemaVersion":"2.2.0","tickId":42,"run":{"day":4}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"tick-42\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write response");
        });
        let profile = parse_profile(&format!(
            r#"{{
                "schema_version": 3,
                "id": "the-bazaar",
                "profile_version": "1.0.0",
                "application": {{"family": "the-bazaar", "versions": []}},
                "display_name": "The Bazaar",
                "selectors": [{{"application_names": ["TheBazaar.exe"]}}],
                "surfaces": [],
                "state_sources": [{{
                    "id": "bazaar-agent",
                    "type": "loopback_http_json",
                    "mode": "read_only",
                    "url": "http://127.0.0.1:{port}/v1/context",
                    "expected_schema_version": "2.2.0",
                    "schema_version_pointer": "/schemaVersion",
                    "tick_pointer": "/tickId",
                    "use_etag": true,
                    "timeout_ms": 1000,
                    "max_response_bytes": 1048576,
                    "optional": true
                }}],
                "settings": {{"dialog_style": "application_rendered", "preferred_route": "visual_fallback"}}
            }}"#
        ))
        .expect("profile");

        let observation = observe_source(
            profile.state_source("bazaar-agent").expect("state source"),
            None,
        )
        .await
        .expect("state observation");

        let StateRead::Changed(observation) = observation else {
            panic!("expected changed state");
        };

        assert_eq!(observation.schema_version, "2.2.0");
        assert_eq!(observation.tick, json!(42));
        assert_eq!(observation.etag.as_deref(), Some("\"tick-42\""));
        assert_eq!(observation.state["run"]["day"], 4);
        server.join().expect("loopback server thread");
    }

    #[tokio::test]
    async fn reports_an_unchanged_state_without_requiring_a_json_body() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback server");
        let port = listener.local_addr().expect("listener address").port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0_u8; 2048];
            let read = stream.read(&mut request).expect("read request");
            let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
            assert!(request.contains("if-none-match: \"tick-42\""));
            write!(
                stream,
                "HTTP/1.1 304 Not Modified\r\nETag: \"tick-42\"\r\nConnection: close\r\n\r\n"
            )
            .expect("write response");
        });
        let profile = parse_profile(&format!(
            r#"{{
                "schema_version": 3,
                "id": "the-bazaar",
                "profile_version": "1.0.0",
                "application": {{"family": "the-bazaar", "versions": []}},
                "display_name": "The Bazaar",
                "selectors": [{{"application_names": ["TheBazaar.exe"]}}],
                "surfaces": [],
                "state_sources": [{{
                    "id": "bazaar-agent",
                    "type": "loopback_http_json",
                    "mode": "read_only",
                    "url": "http://127.0.0.1:{port}/v1/context",
                    "expected_schema_version": "2.2.0",
                    "schema_version_pointer": "/schemaVersion",
                    "tick_pointer": "/tickId",
                    "use_etag": true,
                    "timeout_ms": 1000,
                    "max_response_bytes": 1048576,
                    "optional": true
                }}],
                "settings": {{"dialog_style": "application_rendered", "preferred_route": "visual_fallback"}}
            }}"#
        ))
        .expect("profile");

        let observation = observe_source(
            profile.state_source("bazaar-agent").expect("state source"),
            Some("\"tick-42\""),
        )
        .await
        .expect("unchanged state");

        assert!(matches!(
            observation,
            StateRead::NotModified {
                source_id,
                etag: Some(etag)
            } if source_id == "bazaar-agent" && etag == "\"tick-42\""
        ));
        server.join().expect("loopback server thread");
    }

    #[tokio::test]
    async fn watcher_reuses_the_latest_etag_across_polls() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback server");
        let port = listener.local_addr().expect("listener address").port();
        let server = thread::spawn(move || {
            for request_number in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut request = [0_u8; 2048];
                let read = stream.read(&mut request).expect("read request");
                let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
                if request_number == 0 {
                    assert!(!request.contains("if-none-match:"));
                    let body = r#"{"schemaVersion":"2.2.0","tickId":1}"#;
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"tick-1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .expect("write changed response");
                } else {
                    assert!(request.contains("if-none-match: \"tick-1\""));
                    write!(
                        stream,
                        "HTTP/1.1 304 Not Modified\r\nETag: \"tick-1\"\r\nConnection: close\r\n\r\n"
                    )
                    .expect("write unchanged response");
                }
            }
        });
        let profile = parse_profile(&format!(
            r#"{{
                "schema_version": 3,
                "id": "the-bazaar",
                "profile_version": "1.0.0",
                "application": {{"family": "the-bazaar", "versions": []}},
                "display_name": "The Bazaar",
                "selectors": [{{"application_names": ["TheBazaar.exe"]}}],
                "surfaces": [],
                "state_sources": [{{
                    "id": "bazaar-agent",
                    "type": "loopback_http_json",
                    "mode": "read_only",
                    "url": "http://127.0.0.1:{port}/v1/context",
                    "expected_schema_version": "2.2.0",
                    "schema_version_pointer": "/schemaVersion",
                    "tick_pointer": "/tickId",
                    "use_etag": true,
                    "timeout_ms": 1000,
                    "max_response_bytes": 1048576,
                    "optional": true
                }}],
                "settings": {{"dialog_style": "application_rendered", "preferred_route": "visual_fallback"}}
            }}"#
        ))
        .expect("profile");
        let source = profile.state_source("bazaar-agent").expect("state source");
        let mut watcher = StateWatcher::new(source, None);

        assert!(matches!(
            watcher.poll().await.expect("first poll"),
            StateRead::Changed(_)
        ));
        assert!(matches!(
            watcher.poll().await.expect("second poll"),
            StateRead::NotModified { .. }
        ));
        assert_eq!(watcher.etag(), Some("\"tick-1\""));
        server.join().expect("loopback server thread");
    }
}
