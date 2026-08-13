use rstest::rstest;
use serde_json::Value;
use tokio::io::{AsyncWrite, DuplexStream};

use super::*;

async fn write_json_request(
    writer: &mut (impl AsyncWrite + Unpin),
    value: Value,
) -> Result<(), HostError> {
    write_frame(
        writer,
        &serde_json::to_vec(&value).unwrap(),
        MAX_JSON_FRAME_BYTES,
    )
    .await
}

#[rstest]
#[tokio::test]
async fn process_connection_requires_hello_pings_and_rejects_duplicate_hello() {
    let (mut client, server_stream): (DuplexStream, DuplexStream) = tokio::io::duplex(16 * 1024);
    let server = tokio::spawn(process_connection_with_confirmation_host(
        ComputerUseDriver::create().unwrap(),
        server_stream,
        None,
    ));

    write_json_request(
        &mut client,
        json!({"request_id":"pre-hello", "method":"ping", "params":{}}),
    )
    .await
    .unwrap();
    let response = read_frame(&mut client, MAX_JSON_FRAME_BYTES)
        .await
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "protocol_error");
    assert_eq!(response["request_id"], "pre-hello");

    write_json_request(
        &mut client,
        json!({
            "request_id": "hello-1",
            "method": "hello",
            "params": {
                "protocol_version": HOST_PROTOCOL_VERSION,
                "client_name": "host-integration-test",
                "snapshot_transport": "binary_frame"
            }
        }),
    )
    .await
    .unwrap();
    let response = read_frame(&mut client, MAX_JSON_FRAME_BYTES)
        .await
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["type"], "hello");
    assert_eq!(response["request_id"], "hello-1");
    assert_eq!(response["protocol_version"], HOST_PROTOCOL_VERSION);
    for capability in [
        "pipelined_read_requests",
        "window_inventory_filters",
        "host_ping",
        "host_diagnostics",
        "serialized_raw_input",
    ] {
        assert!(
            response["capabilities"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item == capability))
        );
    }
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(300),
            read_frame(&mut client, MAX_JSON_FRAME_BYTES),
        )
        .await
        .is_err(),
        "session event monitoring must never emit an unsolicited Host frame",
    );

    write_json_request(
        &mut client,
        json!({"request_id":"ping-1", "method":"ping", "params":{}}),
    )
    .await
    .unwrap();
    let response = read_frame(&mut client, MAX_JSON_FRAME_BYTES)
        .await
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["type"], "pong");
    assert_eq!(response["request_id"], "ping-1");
    assert_eq!(response["protocol_version"], HOST_PROTOCOL_VERSION);

    write_json_request(
        &mut client,
        json!({
            "request_id": "hello-2",
            "method": "hello",
            "params": {
                "protocol_version": HOST_PROTOCOL_VERSION,
                "client_name": "host-integration-test",
                "snapshot_transport": "shared_memory"
            }
        }),
    )
    .await
    .unwrap();
    let response = read_frame(&mut client, MAX_JSON_FRAME_BYTES)
        .await
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["type"], "error");
    assert_eq!(response["request_id"], "hello-2");
    assert_eq!(response["code"], "invalid_request");
    drop(client);
    assert!(server.await.unwrap().is_ok());
}

#[rstest]
#[tokio::test]
async fn connection_closes_when_hello_misses_its_absolute_deadline() {
    let (mut client, server_stream): (DuplexStream, DuplexStream) = tokio::io::duplex(4096);
    let (reader, writer) = tokio::io::split(server_stream);
    let server = tokio::spawn(process_connection_parts_with_confirmation_host(
        ComputerUseDriver::create().unwrap(),
        reader,
        writer,
        std::time::Duration::from_millis(25),
        None,
    ));
    write_json_request(
        &mut client,
        json!({"request_id":"pre-hello", "method":"ping", "params":{}}),
    )
    .await
    .unwrap();
    let response = read_frame(&mut client, MAX_JSON_FRAME_BYTES)
        .await
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["code"], "protocol_error");
    let error = tokio::time::timeout(std::time::Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("hello was not completed within 25 ms")
    );
}

#[rstest]
#[tokio::test]
async fn connection_finalizer_aborts_tasks_and_cleans_up_after_errors() {
    let cleaned = Arc::new(AtomicBool::new(false));
    let cleanup_flag = cleaned.clone();
    let mut tasks = JoinSet::new();
    tasks.spawn(std::future::pending::<Result<(), HostError>>());
    let result = finalize_connection(
        Err(HostError::Protocol("broken connection".into())),
        &mut tasks,
        async move {
            cleanup_flag.store(true, Ordering::Release);
            Ok(())
        },
    )
    .await;
    assert!(matches!(result, Err(HostError::Protocol(_))));
    assert!(cleaned.load(Ordering::Acquire));
    assert!(tasks.is_empty());
}
