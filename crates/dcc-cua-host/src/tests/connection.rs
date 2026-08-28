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
fn session_event_poll_uses_the_interruptible_connection_lane() {
    let poll: Request = serde_json::from_value(json!({
        "method": "poll_session_events",
        "params": {
            "session_id": "session-1",
            "task_grant_id": "grant-1",
            "window_capability": "cap-1",
            "after_sequence": 0,
            "timeout_ms": 30_000
        }
    }))
    .unwrap();
    let ping: Request = serde_json::from_value(json!({"method": "ping", "params": {}})).unwrap();

    assert!(is_interruptible_connection_request(&poll));
    assert!(!is_interruptible_connection_request(&ping));
}

#[rstest]
#[case(0, 0)]
#[case(1, 1)]
#[case(50, 50)]
#[case(5_000, 50)]
fn long_delays_poll_the_interrupt_boundary_at_least_every_fifty_ms(
    #[case] remaining_ms: u64,
    #[case] expected_ms: u64,
) {
    assert_eq!(
        interrupt_poll_slice(std::time::Duration::from_millis(remaining_ms)),
        std::time::Duration::from_millis(expected_ms),
    );
}

#[rstest]
#[tokio::test]
async fn process_connection_requires_hello_pings_and_rejects_duplicate_hello() {
    let (mut client, server_stream): (DuplexStream, DuplexStream) = tokio::io::duplex(16 * 1024);
    let driver = ComputerUseDriver::create().unwrap();
    let expected_capabilities = host_capabilities(
        crate::request_contract::cursor_render_backend(driver.upstream_cursor_renderer_enabled())
            != "unavailable",
    );
    let server = tokio::spawn(process_connection_with_security_services(
        driver,
        server_stream,
        HostSecurityServices::default(),
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
    assert_eq!(response["capabilities"], json!(expected_capabilities));
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
    let server = tokio::spawn(process_connection_parts_with_security_services(
        ComputerUseDriver::create().unwrap(),
        reader,
        writer,
        std::time::Duration::from_millis(25),
        HostSecurityServices::default(),
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
