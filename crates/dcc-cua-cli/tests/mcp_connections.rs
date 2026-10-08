use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rstest::rstest;
use serde_json::{Value, json};

const PROCESS_TIMEOUT: Duration = Duration::from_secs(15);
const CURRENT_CONNECTION_URI: &str = "dcc-cua://connection/current";

fn isolated_directory() -> tempfile::TempDir {
    let temporary_root = std::env::temp_dir()
        .canonicalize()
        .expect("resolve the platform temporary directory");
    let mut builder = tempfile::Builder::new();
    builder.prefix("dcc-cua-connections-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The registry intentionally rejects group/world-accessible directories.
        // tempfile's default directory mode otherwise depends on the runner umask.
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder
        .tempdir_in(temporary_root)
        .expect("isolated diagnostic directory")
}

/// Every process in these tests is an owned child with isolated diagnostics.
/// A failed assertion kills only that child, without enumerating live Hosts.
struct Bridge {
    child: Child,
    input: Option<ChildStdin>,
    responses: Receiver<Result<Value, String>>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr: Receiver<Vec<u8>>,
    stderr_reader: Option<JoinHandle<()>>,
}

impl Bridge {
    fn spawn(directory: &Path) -> Self {
        Self::spawn_with_stdout(directory, true)
    }

    fn spawn_with_stdout(directory: &Path, read_stdout: bool) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
            .arg("mcp-server")
            .arg("--diagnostics-dir")
            .arg(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn isolated MCP bridge");
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (sender, responses) = mpsc::channel();
        let stdout_reader = if read_stdout {
            Some(thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let response = line.map_err(|error| error.to_string()).and_then(|line| {
                        let value: Value = serde_json::from_str(&line)
                            .map_err(|error| format!("non-protocol stdout: {error}"))?;
                        if value["jsonrpc"] != "2.0"
                            || (value.get("result").is_none() && value.get("error").is_none())
                        {
                            return Err("stdout must contain only JSON-RPC responses".into());
                        }
                        Ok(value)
                    });
                    if sender.send(response).is_err() {
                        break;
                    }
                }
            }))
        } else {
            drop(stdout);
            None
        };
        let mut stderr_pipe = child.stderr.take().unwrap();
        let (stderr_sender, stderr) = mpsc::channel();
        let stderr_reader = Some(thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr_pipe.read_to_end(&mut bytes).unwrap();
            let _ = stderr_sender.send(bytes);
        }));
        Self {
            child,
            input,
            responses,
            stdout_reader,
            stderr,
            stderr_reader,
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn send(&mut self, message: Value) {
        let input = self.input.as_mut().expect("bridge stdin is open");
        writeln!(input, "{message}").unwrap();
        input.flush().unwrap();
    }

    fn response(&self) -> Value {
        self.responses
            .recv_timeout(PROCESS_TIMEOUT)
            .expect("bridge must reply within the bounded deadline")
            .expect("MCP stdout remains protocol-pure")
    }

    fn initialize(&mut self, name: &str, chat_id: &str) {
        self.send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "clientInfo": {"name": name, "version": "test-version"},
                "_meta": {"dcc-cua": {
                    "chat_id": chat_id, "task_id": "client-task",
                    "token": "PRIVATE_TOKEN_MUST_NOT_APPEAR"
                }},
                "token": "PRIVATE_TOP_LEVEL_TOKEN_MUST_NOT_APPEAR"
            }
        }));
        assert_eq!(self.response()["id"], 1);
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    }

    fn current(&mut self) -> Value {
        self.send(json!({
            "jsonrpc": "2.0", "id": 2, "method": "resources/read",
            "params": {"uri": CURRENT_CONNECTION_URI}
        }));
        let response = self.response();
        assert_eq!(response["id"], 2);
        let content = &response["result"]["contents"][0];
        assert_eq!(content["uri"], CURRENT_CONNECTION_URI);
        assert_eq!(content["mimeType"], "application/json");
        serde_json::from_str(content["text"].as_str().expect("resource JSON text")).unwrap()
    }

    fn close_input(&mut self) {
        drop(self.input.take());
    }

    fn finish(&mut self) -> ExitStatus {
        let status = wait_for_exit(&mut self.child);
        if let Some(reader) = self.stdout_reader.take() {
            reader.join().unwrap();
        }
        for response in self.responses.try_iter() {
            response.expect("all remaining stdout is protocol-pure");
        }
        if let Some(reader) = self.stderr_reader.take() {
            reader.join().unwrap();
        }
        assert!(
            self.stderr
                .recv_timeout(PROCESS_TIMEOUT)
                .unwrap()
                .is_empty(),
            "bridge lifecycle diagnostics must not emit sensitive stderr"
        );
        status
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        drop(self.input.take());
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn wait_for_exit(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("owned child did not exit within the bounded deadline");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn connections(directory: &Path) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .arg("connections")
        .arg("--diagnostics-dir")
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn isolated connection query");
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let status = wait_for_exit(&mut child);
    let stdout = stdout_reader.join().unwrap();
    let stderr = stderr_reader.join().unwrap();
    assert!(status.success(), "connection query failed");
    assert!(stderr.is_empty(), "query diagnostics stay in JSON");
    let report: Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(report["schema"], "dcc-cua.connections.v1");
    assert_eq!(report["registry_available"], true);
    assert_eq!(report["scan_truncated"], false);
    assert!(!report.to_string().contains("PRIVATE_"));
    report
}

fn record_for_pid(report: &Value, pid: u32) -> &Value {
    let records = report["connections"].as_array().unwrap();
    let matching = records
        .iter()
        .filter(|record| record["bridge"]["pid"] == pid)
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), 1, "one record per owned bridge");
    matching[0]
}

fn assert_process_status(record: &Value, supported_status: &str) {
    let expected = if cfg!(any(windows, target_os = "linux")) {
        supported_status
    } else {
        "unknown"
    };
    assert_eq!(record["process_status"], expected);
}

fn assert_observed_close(record: &Value, reason: &str) {
    assert_eq!(record["state"], "closed");
    assert_eq!(record["close_reason"], reason);
    assert_eq!(record["close_reason_source"], "observed");
    assert_process_status(record, "ended");
    assert!(record["closed_at_unix_ms"].is_u64());
}

#[rstest]
fn stdin_eof_before_initialize_closes_only_the_owned_bridge() {
    let directory = isolated_directory();
    let mut bridge = Bridge::spawn(directory.path());
    let pid = bridge.pid();
    bridge.close_input();
    assert!(bridge.finish().success());
    let report = connections(directory.path());
    let record = record_for_pid(&report, pid);
    assert_observed_close(record, "stdin_eof");
    assert!(record["initialized_at_unix_ms"].is_null());
    assert!(record["client_info"].is_null());
    assert_eq!(record["client_metadata"]["source"], "unknown");
}

#[rstest]
fn concurrent_initialized_bridges_have_private_resources_and_remain_valid_while_idle() {
    let directory = isolated_directory();
    let mut first = Bridge::spawn(directory.path());
    let mut second = Bridge::spawn(directory.path());
    first.initialize("first-test-client", "first-chat");
    second.initialize("second-test-client", "second-chat");
    let first_current = first.current();
    let second_current = second.current();
    assert_ne!(
        first_current["connection_id"],
        second_current["connection_id"]
    );
    assert_eq!(first_current["bridge"]["pid"], first.pid());
    assert_eq!(second_current["bridge"]["pid"], second.pid());
    assert_eq!(first_current["client_info"]["name"], "first-test-client");
    assert_eq!(second_current["client_info"]["name"], "second-test-client");
    assert_eq!(first_current["client_metadata"]["chat_id"], "first-chat");
    assert_eq!(second_current["client_metadata"]["chat_id"], "second-chat");
    for current in [&first_current, &second_current] {
        assert_eq!(current["schema"], "dcc-cua.connection.v1");
        assert_eq!(current["transport"], "stdio");
        assert_eq!(current["state"], "initialized");
        assert_process_status(current, "live");
        assert_eq!(current["client_metadata"]["source"], "client_supplied");
        assert_eq!(current["client_metadata"]["task_id"], "client-task");
        assert_eq!(current["client_info"]["version"], "test-version");
        assert_eq!(
            current["runtime"]["runtime_version"],
            env!("CARGO_PKG_VERSION")
        );
        assert!(current["parent"]["pid"].is_u64());
        assert!(current["created_at_unix_ms"].is_u64());
        assert!(current["last_activity_at_unix_ms"].is_u64());
        assert!(current["initialized_at_unix_ms"].is_u64());
        assert_eq!(current["associated_hosts"], json!([]));
        assert!(!current.to_string().contains("PRIVATE_"));
    }
    let report = connections(directory.path());
    assert_eq!(report["connections"].as_array().unwrap().len(), 2);
    assert_eq!(
        record_for_pid(&report, first.pid())["connection_id"],
        first_current["connection_id"]
    );
    assert_eq!(
        record_for_pid(&report, second.pid())["connection_id"],
        second_current["connection_id"]
    );

    first.send(json!({"jsonrpc": "2.0", "id": 4, "method": "resources/list"}));
    let resource_list = first.response();
    let resources = resource_list["result"]["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0]["uri"], CURRENT_CONNECTION_URI);
    first.send(json!({
        "jsonrpc": "2.0", "id": 5, "method": "resources/read",
        "params": {"uri": "dcc-cua://connection/all"}
    }));
    assert!(first.response().get("error").is_some());

    thread::sleep(Duration::from_millis(300));
    assert!(first.child.try_wait().unwrap().is_none());
    assert!(second.child.try_wait().unwrap().is_none());
    first.send(json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}));
    assert_eq!(first.response()["result"], json!({}));
    assert!(
        first.current()["last_activity_at_unix_ms"]
            .as_u64()
            .unwrap()
            > first_current["last_activity_at_unix_ms"].as_u64().unwrap()
    );

    first.close_input();
    assert!(first.finish().success());
    assert!(second.child.try_wait().unwrap().is_none());
    assert_eq!(
        second.current()["connection_id"],
        second_current["connection_id"]
    );
    second.close_input();
    assert!(second.finish().success());
    let report = connections(directory.path());
    assert_observed_close(record_for_pid(&report, first.pid()), "stdin_eof");
    assert_observed_close(record_for_pid(&report, second.pid()), "stdin_eof");
}

#[rstest]
fn abrupt_owned_bridge_exit_requires_creation_identity_before_inference() {
    let directory = isolated_directory();
    let mut bridge = Bridge::spawn(directory.path());
    bridge.initialize("abrupt-test-client", "explicit-chat");
    let current = bridge.current();
    bridge.child.kill().unwrap();
    assert!(!bridge.finish().success());
    let report = connections(directory.path());
    let record = record_for_pid(&report, bridge.pid());
    assert_eq!(record["connection_id"], current["connection_id"]);
    assert_process_status(record, "ended");
    if cfg!(any(windows, target_os = "linux")) {
        assert_eq!(record["state"], "closed");
        assert_eq!(record["close_reason"], "process_exited");
        assert_eq!(record["close_reason_source"], "inferred");
    } else {
        // Without a creation identity, a reused PID cannot certify process exit.
        assert_eq!(record["state"], "initialized");
        assert!(record["bridge"]["creation_id"].is_null());
        assert!(record["bridge"]["creation_time_unix_ms"].is_null());
        assert!(record["close_reason"].is_null());
        assert!(record["close_reason_source"].is_null());
    }
    assert!(record["closed_at_unix_ms"].is_null());
}

#[rstest]
fn malformed_json_is_recoverable_and_oversized_frame_closes_the_record() {
    let directory = isolated_directory();
    let mut bridge = Bridge::spawn(directory.path());
    bridge.initialize("parse-error-test-client", "parse-chat");
    writeln!(bridge.input.as_mut().unwrap(), "{{invalid-json").unwrap();
    assert_eq!(bridge.response()["error"]["code"], -32700);
    assert_eq!(bridge.current()["state"], "initialized");

    let mut input = bridge.input.take().unwrap();
    let writer = thread::spawn(move || {
        let _ = input.write_all(&vec![b' '; dcc_cua_protocol::MAX_JSON_FRAME_BYTES + 1]);
    });
    assert!(!bridge.finish().success());
    writer.join().unwrap();
    let report = connections(directory.path());
    assert_observed_close(record_for_pid(&report, bridge.pid()), "frame_limit");
}

#[rstest]
fn closed_response_pipe_records_observed_output_error() {
    let directory = isolated_directory();
    let mut bridge = Bridge::spawn_with_stdout(directory.path(), false);
    bridge.send(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}));
    assert!(!bridge.finish().success());
    let report = connections(directory.path());
    assert_observed_close(record_for_pid(&report, bridge.pid()), "output_error");
}
