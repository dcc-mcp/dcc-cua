use rstest::rstest;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use dcc_cua_semantic_profiles::parse_profile;
use serde_json::json;

use super::*;

#[rstest]
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

#[rstest]
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

#[rstest]
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
