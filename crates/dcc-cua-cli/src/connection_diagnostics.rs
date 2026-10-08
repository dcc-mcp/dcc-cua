//! Bounded, best-effort observability for one client-owned MCP stdio connection.
//!
//! This module never owns transports, tasks, or process shutdown. Registry files
//! supplement the in-memory resource and cannot make a working MCP fail.

mod process_identity;
mod registry;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const RESOURCE_URI: &str = "dcc-cua://connection/current";
pub const CONNECTION_SCHEMA: &str = "dcc-cua.connection.v1";
pub const CONNECTIONS_SCHEMA: &str = "dcc-cua.connections.v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeBuildIdentity {
    pub runtime_version: String,
    pub source_revision: Option<String>,
    pub source_dirty: Option<bool>,
    pub build_profile: Option<String>,
    pub target: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub creation_time_unix_ms: Option<u64>,
    /// OS creation identity, compared without rounding to fence PID reuse.
    pub creation_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    Live,
    Ended,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Connected,
    Initialized,
    Closed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    StdinEof,
    InputError,
    OutputError,
    FrameLimit,
    ServerDropped,
    PanicUnknown,
    ProcessExited,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientInfo {
    pub name: Option<String>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientMetadata {
    pub chat_id: Option<String>,
    pub task_id: Option<String>,
    pub source: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskCounts {
    pub pending: usize,
    pub active: usize,
    pub stopped: usize,
    pub expired: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AssociatedHost {
    pub kind: String,
    pub connection_id: Option<String>,
    pub process: ProcessIdentity,
}

impl AssociatedHost {
    #[must_use]
    pub fn embedded(connection_id: Option<String>) -> Self {
        Self {
            kind: "embedded_task_host".into(),
            connection_id,
            process: process_identity::current_process(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConnectionRecord {
    pub schema: String,
    pub connection_id: String,
    pub transport: String,
    pub bridge: ProcessIdentity,
    pub parent: Option<ProcessIdentity>,
    pub runtime: RuntimeBuildIdentity,
    pub created_at_unix_ms: u64,
    pub last_activity_at_unix_ms: u64,
    pub request_in_flight: bool,
    pub initialized_at_unix_ms: Option<u64>,
    pub closed_at_unix_ms: Option<u64>,
    pub state: ConnectionState,
    pub close_reason: Option<CloseReason>,
    pub close_reason_source: Option<String>,
    pub process_status: ProcessStatus,
    pub client_info: Option<ClientInfo>,
    pub client_metadata: ClientMetadata,
    pub tasks: TaskCounts,
    pub associated_hosts: Vec<AssociatedHost>,
    pub registry_available: bool,
}

#[derive(Debug, Serialize)]
pub struct ConnectionsReport {
    pub schema: &'static str,
    pub registry_available: bool,
    pub scan_truncated: bool,
    pub connections: Vec<ConnectionRecord>,
    pub limitations: Vec<&'static str>,
}

pub struct ConnectionDiagnostics {
    record: ConnectionRecord,
    directory: Option<PathBuf>,
}

impl ConnectionDiagnostics {
    pub fn new(runtime: RuntimeBuildIdentity, diagnostics_dir: Option<PathBuf>) -> Self {
        let now = unix_time_millis();
        let bridge = process_identity::current_process();
        let parent = process_identity::parent_process(&bridge);
        let process_status = process_identity::status(&bridge);
        let mut diagnostics = Self {
            record: ConnectionRecord {
                schema: CONNECTION_SCHEMA.into(),
                connection_id: format!("mcp-connection-{}", Uuid::new_v4()),
                transport: "stdio".into(),
                bridge,
                parent,
                runtime,
                created_at_unix_ms: now,
                last_activity_at_unix_ms: now,
                request_in_flight: false,
                initialized_at_unix_ms: None,
                closed_at_unix_ms: None,
                state: ConnectionState::Connected,
                close_reason: None,
                close_reason_source: None,
                process_status,
                client_info: None,
                client_metadata: ClientMetadata {
                    chat_id: None,
                    task_id: None,
                    source: "unknown".into(),
                },
                tasks: TaskCounts::default(),
                associated_hosts: Vec::new(),
                registry_available: false,
            },
            directory: diagnostics_dir.or_else(registry::default_directory),
        };
        diagnostics.publish();
        if let Some(directory) = diagnostics.directory.as_deref() {
            registry::retain_recent_terminal_records(directory);
        }
        diagnostics
    }

    #[must_use]
    pub fn snapshot(&self) -> ConnectionRecord {
        self.record.clone()
    }

    pub fn initialized(&mut self, params: &Value) {
        // First initialize binds the diagnostic identity. Never copy arbitrary
        // capabilities, parameters, titles, environment, or credentials.
        if self.record.initialized_at_unix_ms.is_some() {
            return;
        }
        self.record.initialized_at_unix_ms = Some(unix_time_millis());
        self.record.state = ConnectionState::Initialized;
        self.record.client_info = params.get("clientInfo").and_then(|value| {
            let name = sanitized_label(value.get("name"));
            let version = sanitized_label(value.get("version"));
            (name.is_some() || version.is_some()).then_some(ClientInfo { name, version })
        });
        let metadata = &params["_meta"]["dcc-cua"];
        self.record.client_metadata.chat_id = sanitized_id(metadata.get("chat_id"));
        self.record.client_metadata.task_id = sanitized_id(metadata.get("task_id"));
        if self.record.client_metadata.chat_id.is_some()
            || self.record.client_metadata.task_id.is_some()
        {
            self.record.client_metadata.source = "client_supplied".into();
        }
        self.record.last_activity_at_unix_ms = unix_time_millis();
        self.publish();
    }

    pub fn activity(&mut self) {
        if self.record.state != ConnectionState::Closed {
            let now = unix_time_millis();
            if self.record.last_activity_at_unix_ms != now {
                self.record.last_activity_at_unix_ms = now;
                self.publish();
            }
        }
    }

    pub fn set_tasks(&mut self, tasks: TaskCounts, associated_hosts: Vec<AssociatedHost>) {
        let associated_hosts = associated_hosts.into_iter().take(64).collect::<Vec<_>>();
        if self.record.tasks == tasks && self.record.associated_hosts == associated_hosts {
            return;
        }
        self.record.tasks = tasks;
        // The MCP server has a bounded task collection; still bound diagnostics
        // independently so a caller cannot grow the registry frame indefinitely.
        self.record.associated_hosts = associated_hosts;
        self.publish();
    }

    pub fn set_request_in_flight(&mut self, in_flight: bool) {
        if self.record.request_in_flight != in_flight {
            self.record.request_in_flight = in_flight;
            self.publish();
        }
    }

    pub fn close(&mut self, reason: CloseReason) {
        if self.record.state == ConnectionState::Closed {
            return;
        }
        let now = unix_time_millis();
        self.record.last_activity_at_unix_ms = now;
        self.record.closed_at_unix_ms = Some(now);
        self.record.state = ConnectionState::Closed;
        self.record.request_in_flight = false;
        self.record.close_reason = Some(reason);
        self.record.close_reason_source = Some("observed".into());
        self.publish();
        if let Some(directory) = self.directory.as_deref() {
            registry::retain_recent_terminal_records(directory);
        }
    }

    fn publish(&mut self) {
        self.record.registry_available = true;
        if self
            .directory
            .as_deref()
            .is_none_or(|directory| registry::publish(directory, &self.record).is_err())
        {
            self.record.registry_available = false;
        }
    }
}

impl Drop for ConnectionDiagnostics {
    fn drop(&mut self) {
        self.close(if std::thread::panicking() {
            CloseReason::PanicUnknown
        } else {
            CloseReason::ServerDropped
        });
    }
}

/// Query only this user's diagnostic records. This never starts a Host or
/// modifies registry files, process state, transports, or task authorization.
#[must_use]
pub fn list_connections(diagnostics_dir: Option<PathBuf>) -> ConnectionsReport {
    let directory = diagnostics_dir.or_else(registry::default_directory);
    let (registry_available, mut scan_truncated, mut connections) = directory
        .as_deref()
        .map(registry::read_records)
        .unwrap_or_default();
    for record in &mut connections {
        record.process_status = process_identity::status(&record.bridge);
        if record.state != ConnectionState::Closed && record.process_status == ProcessStatus::Ended
        {
            record.close_reason = Some(CloseReason::ProcessExited);
            record.close_reason_source = Some("inferred".into());
            // No transport event or exact exit time was observed by the writer.
            record.state = ConnectionState::Closed;
            record.closed_at_unix_ms = None;
        }
    }
    connections.sort_by_key(|record| std::cmp::Reverse(record.created_at_unix_ms));
    scan_truncated |= connections.len() > registry::MAX_REPORT_RECORDS;
    connections.truncate(registry::MAX_REPORT_RECORDS);
    ConnectionsReport {
        schema: CONNECTIONS_SCHEMA,
        registry_available,
        scan_truncated,
        connections,
        limitations: vec![
            "Only instrumented MCP connections in this registry are included; older bridges are unknown.",
            "Chat and external task identity are unknown unless explicitly supplied by the client.",
            "An idle initialized connection may remain valid; process count does not establish a leak.",
            "Process exit is inferred from OS creation identity and does not establish stdin EOF.",
            "Where OS creation identity is unavailable, process_status remains unknown.",
        ],
    }
}

fn sanitized_label(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?;
    if text.is_empty()
        || text.len() > 128
        || !text
            .chars()
            .all(|character| character.is_alphanumeric() || " ._-+".contains(character))
    {
        return None;
    }
    Some(text.to_owned())
}

fn sanitized_id(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?;
    if text.is_empty()
        || text.len() > 128
        || !text
            .bytes()
            .all(|character| character.is_ascii_alphanumeric() || b"._-:".contains(&character))
    {
        return None;
    }
    Some(text.to_owned())
}

fn unix_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
