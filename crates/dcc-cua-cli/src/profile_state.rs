use std::time::Duration;

use dcc_cua_semantic_profiles::StateSource;
use reqwest::header::{ACCEPT, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use super::{flag_value, has_flag, load_semantic_profile};

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
    client: reqwest::Client,
    source: &'a StateSource,
    etag: Option<String>,
    last_tick: Option<u64>,
}

impl<'a> StateWatcher<'a> {
    pub(crate) fn new(
        source: &'a StateSource,
        etag: Option<String>,
    ) -> Result<Self, ProfileStateError> {
        Ok(Self {
            client: state_client(source)?,
            source,
            etag,
            last_tick: None,
        })
    }

    pub(crate) fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    pub(crate) async fn poll(&mut self) -> Result<StateRead, ProfileStateError> {
        let read = observe_source_with_client(&self.client, self.source, self.etag()).await?;
        match &read {
            StateRead::Changed(observation) => {
                let actual = observation
                    .tick
                    .as_u64()
                    .ok_or_else(|| ProfileStateError::InvalidTick(observation.tick.clone()))?;
                if let Some(previous) = self.last_tick
                    && actual <= previous
                {
                    return Err(ProfileStateError::NonMonotonicTick { previous, actual });
                }
                self.last_tick = Some(actual);
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
    #[error("state source tick must be an unsigned integer, got {0}")]
    InvalidTick(Value),
    #[error("state source tick did not advance: previous {previous}, actual {actual}")]
    NonMonotonicTick { previous: u64, actual: u64 },
    #[error("state source schema version {actual:?} does not match expected {expected:?}")]
    SchemaMismatch { expected: String, actual: String },
    #[error("state source ETag is not a valid HTTP header: {0}")]
    InvalidEtag(String),
}

pub(crate) async fn observe_source(
    source: &StateSource,
    previous_etag: Option<&str>,
) -> Result<StateRead, ProfileStateError> {
    let client = state_client(source)?;
    observe_source_with_client(&client, source, previous_etag).await
}

fn state_client(source: &StateSource) -> Result<reqwest::Client, ProfileStateError> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_millis(source.timeout_ms))
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

async fn observe_source_with_client(
    client: &reqwest::Client,
    source: &StateSource,
    previous_etag: Option<&str>,
) -> Result<StateRead, ProfileStateError> {
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
    if has_flag(flags, "--watch") {
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

    let mut watcher = StateWatcher::new(source, flag_value(flags, "--etag"))?;
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
mod tests;
