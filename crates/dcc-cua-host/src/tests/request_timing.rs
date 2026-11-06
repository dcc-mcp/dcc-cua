use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rstest::rstest;
use serde_json::{Value, json};
use tokio::io::DuplexStream;

use super::task_authorization::observation_only_test_driver;
use super::*;

#[derive(Default)]
struct MemoryTimingObserver {
    samples: Mutex<Vec<HostTimingSample>>,
}

impl HostTimingObserver for MemoryTimingObserver {
    fn record(&self, sample: HostTimingSample) {
        let mut samples = self.samples.lock().unwrap();
        if samples.len() < 8 {
            samples.push(sample);
        }
    }
}

fn hello(request_id: &str, client_name: &str) -> Value {
    json!({
        "request_id": request_id,
        "method": "hello",
        "params": {
            "protocol_version": HOST_PROTOCOL_VERSION,
            "client_name": client_name,
            "snapshot_transport": "binary_frame"
        }
    })
}

fn ping(request_id: &str) -> Value {
    json!({"request_id": request_id, "method": "ping", "params": {}})
}

async fn send_request(client: &mut DuplexStream, request: &Value) {
    let body = serde_json::to_vec(request).unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        write_frame(client, &body, MAX_JSON_FRAME_BYTES),
    )
    .await
    .expect("synthetic request write completed")
    .unwrap();
}

async fn receive_response(client: &mut DuplexStream) -> Vec<u8> {
    tokio::time::timeout(
        Duration::from_secs(5),
        read_frame(client, MAX_JSON_FRAME_BYTES),
    )
    .await
    .expect("synthetic response completed")
    .unwrap()
    .expect("synthetic connection returned a frame")
}

async fn run_transcript(
    requests: &[Value],
    observer: Option<Arc<MemoryTimingObserver>>,
    pipeline_after_hello: bool,
) -> BTreeMap<String, Vec<u8>> {
    let (driver, exchanges) = observation_only_test_driver();
    let services = match observer {
        Some(observer) => HostSecurityServices::default().with_timing_observer(observer),
        None => HostSecurityServices::default(),
    };
    let (mut client, server_stream) = tokio::io::duplex(16 * 1024);
    let server = tokio::spawn(process_connection_with_security_services(
        driver,
        server_stream,
        services,
    ));

    send_request(&mut client, &requests[0]).await;
    let mut frames = vec![receive_response(&mut client).await];
    if pipeline_after_hello {
        for request in &requests[1..] {
            send_request(&mut client, request).await;
        }
        for _ in &requests[1..] {
            frames.push(receive_response(&mut client).await);
        }
    } else {
        for request in &requests[1..] {
            send_request(&mut client, request).await;
            frames.push(receive_response(&mut client).await);
        }
    }
    drop(client);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("synthetic connection shutdown completed")
        .unwrap()
        .unwrap();
    assert_eq!(exchanges.load(Ordering::Relaxed), 0);

    let mut indexed = BTreeMap::new();
    for frame in frames {
        let value: Value = serde_json::from_slice(&frame).unwrap();
        let request_id = value["request_id"].as_str().unwrap().to_owned();
        assert!(indexed.insert(request_id, frame).is_none());
    }
    indexed
}

fn assert_equivalent_frames(
    baseline: &BTreeMap<String, Vec<u8>>,
    observed: &BTreeMap<String, Vec<u8>>,
    hello_id: &str,
) {
    assert_eq!(
        baseline.keys().collect::<Vec<_>>(),
        observed.keys().collect::<Vec<_>>()
    );
    for (request_id, expected) in baseline {
        let actual = &observed[request_id];
        if request_id == hello_id {
            let mut expected: Value = serde_json::from_slice(expected).unwrap();
            let mut actual: Value = serde_json::from_slice(actual).unwrap();
            // Each connection receives its own independently generated identity.
            for response in [&mut expected, &mut actual] {
                let connection_id = response
                    .as_object_mut()
                    .unwrap()
                    .remove("connection_id")
                    .unwrap();
                assert!(connection_id.as_str().is_some_and(|id| !id.is_empty()));
            }
            assert_eq!(expected, actual);
        } else {
            assert_eq!(expected, actual);
        }
    }
}

fn assert_sample_sequences(samples: &[HostTimingSample]) {
    let mut sequences = samples
        .iter()
        .map(|sample| sample.sequence)
        .collect::<Vec<_>>();
    assert!(sequences.iter().all(|sequence| *sequence > 0));
    sequences.sort_unstable();
    sequences.dedup();
    assert_eq!(sequences.len(), samples.len());
    assert!(
        samples
            .iter()
            .all(|sample| sample.schema == "dcc-cua.host-request-timing.v1")
    );
}

#[rstest]
#[tokio::test]
async fn request_timing_preserves_normal_and_error_wire_responses() {
    let requests = [
        hello("hello-1", "synthetic-client"),
        ping("ping-1"),
        hello("hello-2", "synthetic-client"),
    ];
    let baseline = run_transcript(&requests, None, false).await;
    let observer = Arc::new(MemoryTimingObserver::default());
    let observed = run_transcript(&requests, Some(observer.clone()), false).await;
    assert_equivalent_frames(&baseline, &observed, "hello-1");

    let pong: Value = serde_json::from_slice(&observed["ping-1"]).unwrap();
    let duplicate: Value = serde_json::from_slice(&observed["hello-2"]).unwrap();
    assert_eq!(pong["type"], "pong");
    assert_eq!(duplicate["type"], "error");
    assert_eq!(duplicate["code"], "invalid_request");

    let samples = observer.samples.lock().unwrap();
    assert_eq!(samples.len(), 3);
    assert_sample_sequences(&samples);
    assert_eq!(
        samples
            .iter()
            .filter(|sample| sample.kind == HostTimingRequestKind::Handshake)
            .count(),
        2
    );
    assert_eq!(
        samples
            .iter()
            .filter(|sample| sample.outcome == HostTimingOutcome::Returned)
            .count(),
        2
    );
    assert_eq!(
        samples
            .iter()
            .filter(|sample| sample.outcome == HostTimingOutcome::Error)
            .count(),
        1
    );
}

#[rstest]
#[tokio::test]
async fn request_timing_records_pipelined_pings_without_driver_dispatch() {
    let requests = [
        hello("hello-1", "synthetic-client"),
        ping("ping-1"),
        ping("ping-2"),
    ];
    let baseline = run_transcript(&requests, None, true).await;
    let observer = Arc::new(MemoryTimingObserver::default());
    let observed = run_transcript(&requests, Some(observer.clone()), true).await;
    assert_equivalent_frames(&baseline, &observed, "hello-1");
    for request_id in ["ping-1", "ping-2"] {
        let response: Value = serde_json::from_slice(&observed[request_id]).unwrap();
        assert_eq!(response, with_request_id(ping_response(), Some(request_id)));
    }

    let samples = observer.samples.lock().unwrap();
    assert_eq!(samples.len(), 3);
    assert_sample_sequences(&samples);
    assert!(
        samples
            .iter()
            .all(|sample| sample.outcome == HostTimingOutcome::Returned)
    );
    assert_eq!(
        samples
            .iter()
            .filter(|sample| sample.kind == HostTimingRequestKind::Handshake)
            .count(),
        1
    );
    assert_eq!(
        samples
            .iter()
            .filter(|sample| sample.kind == HostTimingRequestKind::Other)
            .count(),
        2
    );
}

#[rstest]
#[tokio::test]
async fn request_timing_records_stateful_refusal_without_request_content() {
    let markers = [
        "synthetic-request-marker",
        "synthetic-client-marker",
        "synthetic-session-marker",
        "synthetic-grant-marker",
        "synthetic-capability-marker",
    ];
    let requests = [
        hello("hello-1", markers[1]),
        json!({
            "request_id": markers[0],
            "method": "get_session_state",
            "params": {
                "session_id": markers[2],
                "task_grant_id": markers[3],
                "window_capability": markers[4]
            }
        }),
    ];
    let baseline = run_transcript(&requests, None, false).await;
    let observer = Arc::new(MemoryTimingObserver::default());
    let observed = run_transcript(&requests, Some(observer.clone()), false).await;
    assert_equivalent_frames(&baseline, &observed, "hello-1");
    let refusal: Value = serde_json::from_slice(&observed[markers[0]]).unwrap();
    assert_eq!(refusal["type"], "error");

    let samples = observer.samples.lock().unwrap();
    assert_eq!(samples.len(), 2);
    assert_sample_sequences(&samples);
    assert_eq!(
        samples
            .iter()
            .filter(|sample| {
                sample.kind == HostTimingRequestKind::Other
                    && sample.outcome == HostTimingOutcome::Error
            })
            .count(),
        1
    );
    let serialized = serde_json::to_string(&*samples).unwrap();
    for marker in markers {
        assert!(!serialized.contains(marker));
    }
    let values = serde_json::to_value(&*samples).unwrap();
    for sample in values.as_array().unwrap() {
        let fields = sample.as_object().unwrap();
        assert_eq!(fields.len(), 6);
        for name in [
            "schema",
            "sequence",
            "kind",
            "outcome",
            "elapsed_ns",
            "stages",
        ] {
            assert!(fields.contains_key(name));
        }
        assert!(sample["elapsed_ns"].as_u64().is_some());
        for stage in sample["stages"].as_array().unwrap() {
            let fields = stage.as_object().unwrap();
            assert_eq!(fields.len(), 4);
            for name in ["phase", "count", "total_ns", "dropped_count"] {
                assert!(fields.contains_key(name));
            }
        }
    }
}
