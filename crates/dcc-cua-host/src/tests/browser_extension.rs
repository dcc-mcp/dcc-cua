use rstest::rstest;
use serde_json::{Value, json};

use crate::browser_extension::BrowserExtensionRegistry;

fn hello() -> Value {
    json!({
        "schema": "dcc-cua.browser-extension.v1",
        "type": "hello",
        "protocol": {"min": 1, "max": 1},
        "extension": {"id": "published-extension", "version": "0.1.0"},
        "capabilities": ["explicit_tab_pairing_v1", "semantic_snapshot_v1"],
        "pairing": {
            "session_nonce": "nonce-1",
            "tab_id": 7,
            "window_id": 9,
            "origin": "https://example.com",
            "document_id": "document-1"
        }
    })
}

#[rstest]
#[tokio::test]
async fn registered_provider_round_trips_one_extension_call() {
    let registry = BrowserExtensionRegistry::new();
    let registered = registry
        .register(&hello(), "chrome-extension://published-extension/", 42)
        .await
        .unwrap();
    let provider_id = registered["provider_id"].as_str().unwrap().to_owned();
    let provider_secret = registered["provider_secret"].as_str().unwrap().to_owned();

    let bridge_registry = registry.clone();
    let bridge_provider_id = provider_id.clone();
    let bridge = tokio::spawn(async move {
        let command = bridge_registry
            .next_command(&bridge_provider_id, &provider_secret, 1_000)
            .await
            .unwrap()["command"]
            .clone();
        assert_eq!(command["method"], "snapshot");
        bridge_registry
            .complete(
                &bridge_provider_id,
                &provider_secret,
                json!({
                    "schema": "dcc-cua.browser-extension.v1",
                    "type": "response",
                    "request_id": command["request_id"],
                    "ok": true,
                    "result": {"snapshot_id":"snapshot-1"}
                }),
            )
            .await
            .unwrap();
    });
    let result = registry
        .call(
            &provider_id,
            "https://example.com",
            42,
            "snapshot",
            &json!({"max_nodes": 100}),
            1_000,
        )
        .await
        .unwrap();
    bridge.await.unwrap();
    assert_eq!(result["result"]["snapshot_id"], "snapshot-1");
}

#[rstest]
#[tokio::test]
async fn registration_rejects_origin_identity_mismatch() {
    let registry = BrowserExtensionRegistry::new();
    let error = registry
        .register(&hello(), "chrome-extension://different-extension/", 42)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("does not match"));
}

#[rstest]
#[tokio::test]
async fn exact_window_process_cannot_use_another_browser_provider() {
    let registry = BrowserExtensionRegistry::new();
    let registered = registry
        .register(&hello(), "chrome-extension://published-extension/", 42)
        .await
        .unwrap();
    let error = registry
        .call(
            registered["provider_id"].as_str().unwrap(),
            "https://example.com",
            99,
            "snapshot",
            &json!({"max_nodes": 100}),
            1_000,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("exact window session"));
}

#[rstest]
#[tokio::test]
async fn status_exposes_only_providers_for_the_exact_window_process() {
    let registry = BrowserExtensionRegistry::new();
    registry
        .register(&hello(), "chrome-extension://published-extension/", 42)
        .await
        .unwrap();
    registry
        .register(&hello(), "chrome-extension://published-extension/", 99)
        .await
        .unwrap();

    let status = registry.status_for_process(42).await;

    assert_eq!(status["providers"].as_array().unwrap().len(), 1);
    assert_eq!(
        status["providers"][0]["extension"]["browser_process_id"],
        42
    );
}
