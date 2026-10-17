//! Real public CLI MCP transport. It never calls an internal dismiss helper.
#[allow(unused_imports)]
use rstest::rstest;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

pub(super) struct Mcp {
    child: Child,
    input: Option<ChildStdin>,
    output: mpsc::Receiver<Value>,
    reader: Option<std::thread::JoinHandle<()>>,
    stderr: Option<std::thread::JoinHandle<Vec<u8>>>,
    sequence: u64,
}

impl Mcp {
    pub fn new(binary: &Path) -> Self {
        let mut child = Command::new(binary)
            .arg("mcp-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn native menu MCP fixture runtime");
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (tx, output) = mpsc::sync_channel(16);
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.expect("MCP stdout line");
                assert!(line.len() <= 1_048_576);
                let value = serde_json::from_str(&line).expect("MCP stdout must remain JSON-only");
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        let stderr = std::thread::spawn(move || {
            use std::io::Read;
            let mut bytes = Vec::new();
            stderr
                .by_ref()
                .take(1_048_577)
                .read_to_end(&mut bytes)
                .unwrap();
            bytes
        });
        let mut client = Self {
            child,
            input,
            output,
            reader: Some(reader),
            stderr: Some(stderr),
            sequence: 0,
        };
        let initialized = client.rpc("initialize", json!({"protocolVersion":"2024-11-05","clientInfo":{"name":"controlled-native-menu-dismiss-ci","version":"1"},"capabilities":{}}));
        assert_eq!(initialized["serverInfo"]["name"], "dcc-cua-task-automation");
        client
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.sequence += 1;
        let request = json!({"jsonrpc":"2.0","id":self.sequence,"method":method,"params":params});
        let input = self.input.as_mut().unwrap();
        writeln!(input, "{request}").unwrap();
        input.flush().unwrap();
        let response = self
            .output
            .recv_timeout(Duration::from_secs(30))
            .expect("bounded MCP response");
        assert_eq!(response["id"], self.sequence, "{response}");
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    pub fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.rpc("tools/call", json!({"name":name,"arguments":arguments}))
    }

    pub fn call(&mut self, task: &str, method: &str, params: Value) -> Value {
        self.tool(
            "dcc_cua_task_call",
            json!({"task_id":task,"method":method,"params":params}),
        )
    }

    pub fn close(&mut self) {
        drop(self.input.take());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "MCP runtime exited {status}");
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "MCP runtime did not exit after stop/EOF"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        self.reader.take().unwrap().join().unwrap();
        let stderr = self.stderr.take().unwrap().join().unwrap();
        assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
