use rstest::rstest;

use super::*;
use tokio::io::DuplexStream;

#[rstest]
fn default_endpoint_uses_the_shared_protocol_contract() {
    assert_eq!(
        HostClient::default_endpoint(),
        dcc_cua_protocol::default_endpoint()
    );
}

#[rstest]
fn live_observation_state_is_pipeline_safe() {
    assert!(is_pipeline_safe_method("live_observation_state"));
    assert!(!is_pipeline_safe_method("live_observation_start"));
}

#[rstest]
fn input_state_read_is_pipeline_safe_but_event_long_poll_is_not() {
    assert!(is_pipeline_safe_method("get_input_state"));
    assert!(is_pipeline_safe_method("session_health"));
    assert!(!is_pipeline_safe_method("poll_session_events"));
}

async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    body: &[u8],
    max: usize,
) -> HostClientResult<()> {
    write_frame_unflushed(writer, body, max).await?;
    writer.flush().await?;
    Ok(())
}

#[rstest]
#[tokio::test]
async fn client_negotiates_and_reads_binary_attachment() {
    let (client_stream, server_stream) = tokio::io::duplex(4096);
    let server = tokio::spawn(fake_server(server_stream));
    let mut client = HostClient::from_stream(client_stream);

    let hello = client.hello("test-client").await.unwrap();
    assert_eq!(hello.value["type"], "hello");
    assert!(client.supports_capability("binary_snapshot_frames"));
    assert!(!client.supports_capability("missing_capability"));
    let response = client.request("desktop_snapshot", json!({})).await.unwrap();
    assert_eq!(response.value["type"], "desktop_snapshot");
    assert_eq!(
        response.binary_attachment.as_deref(),
        Some(b"png".as_slice())
    );
    server.await.unwrap().unwrap();
}

#[rstest]
#[tokio::test]
async fn client_rejects_requests_before_hello() {
    let (client_stream, _server_stream) = tokio::io::duplex(64);
    let mut client = HostClient::from_stream(client_stream);
    assert!(matches!(
        client.request("list_apps", json!({})).await,
        Err(HostClientError::Protocol(message)) if message.contains("hello")
    ));
}

#[rstest]
#[tokio::test]
async fn client_ping_uses_the_lightweight_host_route() {
    let (client_stream, server_stream) = tokio::io::duplex(4096);
    let server = tokio::spawn(fake_ping_server(server_stream));
    let mut client = HostClient::from_stream(client_stream);
    client.hello("ping-client").await.unwrap();
    let response = client.ping().await.unwrap();
    assert_eq!(response.value["type"], "pong");
    assert_eq!(response.value["protocol_version"], HOST_PROTOCOL_VERSION);
    server.await.unwrap().unwrap();
}

#[rstest]
#[tokio::test]
async fn logical_task_reuses_one_connection_and_injects_exact_credentials() {
    let (client_stream, server_stream) = tokio::io::duplex(8192);
    let server = tokio::spawn(fake_logical_task_server(server_stream));
    let mut client = HostClient::from_stream(client_stream);
    client.hello("logical-task-client").await.unwrap();

    let mut task = client
        .open_logical_task_session(
            "task-7",
            json!({"task_grant_id":"grant-7", "process_id":42, "window_handle":99}),
            60_000,
        )
        .await
        .unwrap();
    assert_eq!(task.session_id(), "task-7");
    assert_eq!(task.task_grant_id(), "grant-7");
    assert_eq!(task.idle_timeout_ms(), 60_000);
    let snapshot = task
        .request("snapshot", json!({"max_depth": 4}))
        .await
        .unwrap();
    assert_eq!(snapshot.value["type"], "snapshot");
    let mut client = task.close().await.unwrap();
    assert_eq!(client.ping().await.unwrap().value["type"], "pong");
    server.await.unwrap().unwrap();
}

#[rstest]
fn logical_task_rejects_credential_override() {
    let result = bind_task_credentials(json!({"session_id":"other"}), "task-7", "grant-7", "cap-7");
    assert!(matches!(
        result,
        Err(HostClientError::Protocol(message)) if message.contains("cannot override session_id")
    ));
}

#[rstest]
fn stopped_host_process_reports_not_running() {
    let mut process = HostProcess {
        client: None,
        child: None,
    };
    assert!(!process.is_running().unwrap());
}

#[rstest]
fn client_rejects_malformed_host_capabilities() {
    assert!(response_capabilities(&json!({"capabilities": {}})).is_err());
    assert!(response_capabilities(&json!({"capabilities": ["ok", 1]})).is_err());
    assert_eq!(
        response_capabilities(&json!({})).unwrap(),
        Vec::<String>::new()
    );
}

#[rstest]
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

#[rstest]
#[tokio::test]
async fn client_rejects_duplicate_hello() {
    let (client_stream, server_stream) = tokio::io::duplex(4096);
    let server = tokio::spawn(fake_hello_only_server(server_stream));
    let mut client =
        HostClient::from_stream_with_transport(client_stream, SnapshotTransport::SharedMemory);
    client.hello("hello-once").await.unwrap();
    assert!(matches!(
        client.hello("hello-twice").await,
        Err(HostClientError::Protocol(message))
            if message.contains("already completed")
    ));
    server.await.unwrap().unwrap();
}

#[rstest]
#[tokio::test]
async fn client_preserves_caller_request_id() {
    let (client_stream, server_stream) = tokio::io::duplex(4096);
    let server = tokio::spawn(fake_request_id_server(server_stream));
    let mut client = HostClient::from_stream(client_stream);
    client.hello("request-id-client").await.unwrap();
    let response = client
        .request_with_id("core-task-42", "screen_size", json!({}))
        .await
        .unwrap();
    assert_eq!(response.value["request_id"], "core-task-42");
    server.await.unwrap().unwrap();
}

#[rstest]
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

#[rstest]
#[tokio::test]
async fn client_pipelines_caller_request_ids_in_order() {
    let (client_stream, server_stream) = tokio::io::duplex(4096);
    let server = tokio::spawn(fake_batch_request_id_server(server_stream));
    let mut client = HostClient::from_stream(client_stream);
    client.hello("batch-request-id-client").await.unwrap();

    let responses = client
        .request_batch_with_ids(vec![
            ("core-read-1".into(), "screen_size".into(), json!({})),
            ("core-read-2".into(), "cursor_position".into(), json!({})),
        ])
        .await
        .unwrap();

    assert_eq!(responses[0].value["request_id"], "core-read-1");
    assert_eq!(responses[1].value["request_id"], "core-read-2");
    server.await.unwrap().unwrap();
}

#[rstest]
#[tokio::test]
async fn client_batch_all_drains_remote_errors() {
    let (client_stream, server_stream) = tokio::io::duplex(4096);
    let server = tokio::spawn(fake_batch_mixed_server(server_stream));
    let mut client = HostClient::from_stream(client_stream);
    client.hello("batch-all-client").await.unwrap();

    let results = client
        .request_batch_with_ids_all(vec![
            ("read-error".into(), "screen_size".into(), json!({})),
            ("read-ok".into(), "cursor_position".into(), json!({})),
        ])
        .await
        .unwrap();

    assert!(matches!(
        &results[0],
        Err(HostClientError::Remote { code, .. }) if code == "not_ready"
    ));
    assert_eq!(results[1].as_ref().unwrap().value["request_id"], "read-ok");
    server.await.unwrap().unwrap();
}

#[rstest]
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
    assert!(matches!(
        client
            .request_batch_with_ids(vec![(
                "".into(),
                "screen_size".into(),
                json!({})
            )])
            .await,
        Err(HostClientError::Protocol(message))
            if message.contains("1..")
    ));
    assert!(matches!(
        client
            .request_batch_with_ids(vec![
                ("duplicate".into(), "screen_size".into(), json!({})),
                ("duplicate".into(), "cursor_position".into(), json!({})),
            ])
            .await,
        Err(HostClientError::Protocol(message))
            if message.contains("duplicate")
    ));
    server.await.unwrap().unwrap();
}

#[rstest]
#[tokio::test]
async fn client_bounds_one_parallel_discovery_batch() {
    let (client_stream, server_stream) = tokio::io::duplex(4096);
    let server = tokio::spawn(fake_hello_only_server(server_stream));
    let mut client =
        HostClient::from_stream_with_transport(client_stream, SnapshotTransport::SharedMemory);
    client.hello("bounded-batch-client").await.unwrap();

    let requests = (0..=MAX_PARALLEL_DISCOVERY_REQUESTS)
        .map(|index| (format!("ping-{index}"), "ping".into(), json!({})))
        .collect::<Vec<_>>();
    assert!(matches!(
        client.request_batch_with_ids(requests).await,
        Err(HostClientError::Protocol(message)) if message.contains("at most 32")
    ));
    server.await.unwrap().unwrap();
}

#[rstest]
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

#[rstest]
#[tokio::test]
async fn client_can_cancel_window_wait_with_generated_request_id() {
    let (client_stream, server_stream) = tokio::io::duplex(4096);
    let server = tokio::spawn(fake_window_wait_cancel_server(server_stream));
    let mut client = HostClient::from_stream(client_stream);
    client.hello("window-wait-client").await.unwrap();
    let response = client
        .wait_for_window_with_cancel(
            json!({"query":{"app":"UE5Editor.exe"},"timeout_ms":30000}),
            async {},
        )
        .await
        .unwrap();
    assert_eq!(response.value["type"], "window_wait_cancelled");
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
        json!({
            "type":"hello",
            "capabilities":["binary_snapshot_frames"]
        }),
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

async fn fake_ping_server(mut stream: DuplexStream) -> HostClientResult<()> {
    let hello = read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
        .await?
        .unwrap();
    let hello: Value = serde_json::from_slice(&hello).unwrap();
    write_json_response(
        &mut stream,
        hello["request_id"].as_str().unwrap(),
        json!({"type":"hello","capabilities":["host_ping"]}),
    )
    .await?;
    let ping = read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
        .await?
        .unwrap();
    let ping: Value = serde_json::from_slice(&ping).unwrap();
    assert_eq!(ping["method"], "ping");
    write_json_response(
        &mut stream,
        ping["request_id"].as_str().unwrap(),
        json!({"type":"pong","protocol_version":HOST_PROTOCOL_VERSION}),
    )
    .await
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

async fn fake_request_id_server(mut stream: DuplexStream) -> HostClientResult<()> {
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
    let request = read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
        .await?
        .unwrap();
    let request: Value = serde_json::from_slice(&request).unwrap();
    assert_eq!(request["request_id"], "core-task-42");
    write_json_response(&mut stream, "core-task-42", json!({"type":"screen_size"})).await
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

async fn fake_batch_request_id_server(mut stream: DuplexStream) -> HostClientResult<()> {
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
    assert_eq!(first["request_id"], "core-read-1");
    assert_eq!(second["request_id"], "core-read-2");
    write_json_response(
        &mut stream,
        "core-read-2",
        json!({"type":"cursor_position"}),
    )
    .await?;
    write_json_response(&mut stream, "core-read-1", json!({"type":"screen_size"})).await
}

async fn fake_batch_mixed_server(mut stream: DuplexStream) -> HostClientResult<()> {
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
    assert_eq!(first["request_id"], "read-error");
    assert_eq!(second["request_id"], "read-ok");
    write_json_response(&mut stream, "read-ok", json!({"type":"cursor_position"})).await?;
    write_json_response(
        &mut stream,
        "read-error",
        json!({"type":"error","code":"not_ready","message":"try again"}),
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

async fn fake_window_wait_cancel_server(mut stream: DuplexStream) -> HostClientResult<()> {
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
    let wait = if first["method"] == "wait_for_window" {
        first.clone()
    } else {
        second.clone()
    };
    let cancel = if second["method"] == "cancel_window_wait" {
        second
    } else {
        first
    };
    assert_eq!(wait["method"], "wait_for_window");
    assert_eq!(cancel["method"], "cancel_window_wait");
    assert_eq!(cancel["params"]["wait_id"], wait["request_id"]);
    write_json_response(
        &mut stream,
        cancel["request_id"].as_str().unwrap(),
        json!({"type":"window_wait_cancel_requested"}),
    )
    .await?;
    write_json_response(
        &mut stream,
        wait["request_id"].as_str().unwrap(),
        json!({"type":"window_wait_cancelled"}),
    )
    .await
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

async fn fake_logical_task_server(mut stream: DuplexStream) -> HostClientResult<()> {
    let hello: Value = serde_json::from_slice(
        &read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
            .await?
            .unwrap(),
    )
    .unwrap();
    write_json_response(
        &mut stream,
        hello["request_id"].as_str().unwrap(),
        json!({"type":"hello", "capabilities":[]}),
    )
    .await?;

    let open: Value = serde_json::from_slice(
        &read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
            .await?
            .unwrap(),
    )
    .unwrap();
    assert_eq!(open["method"], "open_session");
    assert_eq!(open["params"]["session_id"], "task-7");
    assert_eq!(open["params"]["grant"]["task_grant_id"], "grant-7");
    assert_eq!(open["params"]["idle_timeout_ms"], 60_000);
    write_json_response(
        &mut stream,
        open["request_id"].as_str().unwrap(),
        json!({
            "type":"session_opened",
            "session_id":"task-7",
            "window_capability":"cap-7",
        }),
    )
    .await?;

    let snapshot: Value = serde_json::from_slice(
        &read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
            .await?
            .unwrap(),
    )
    .unwrap();
    assert_eq!(snapshot["method"], "snapshot");
    assert_eq!(snapshot["params"]["session_id"], "task-7");
    assert_eq!(snapshot["params"]["task_grant_id"], "grant-7");
    assert_eq!(snapshot["params"]["window_capability"], "cap-7");
    assert_eq!(snapshot["params"]["max_depth"], 4);
    write_json_response(
        &mut stream,
        snapshot["request_id"].as_str().unwrap(),
        json!({"type":"snapshot"}),
    )
    .await?;

    let stop: Value = serde_json::from_slice(
        &read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
            .await?
            .unwrap(),
    )
    .unwrap();
    assert_eq!(stop["method"], "stop_session");
    assert_eq!(stop["params"]["session_id"], "task-7");
    write_json_response(
        &mut stream,
        stop["request_id"].as_str().unwrap(),
        json!({"type":"session_stopped", "session_id":"task-7"}),
    )
    .await?;

    let ping: Value = serde_json::from_slice(
        &read_frame(&mut stream, MAX_JSON_FRAME_BYTES)
            .await?
            .unwrap(),
    )
    .unwrap();
    assert_eq!(ping["method"], "ping");
    write_json_response(
        &mut stream,
        ping["request_id"].as_str().unwrap(),
        json!({"type":"pong"}),
    )
    .await
}
