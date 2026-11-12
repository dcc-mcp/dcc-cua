use rstest::rstest;
use serde_json::{Value, json};

use crate::browser_extension::{BrowserExtensionRegistry, validate_extension_response};

#[rstest]
fn navigation_only_revokes_the_provider_after_evidence_consumption_or_unknown_delivery() {
    use crate::browser_extension::navigation_provider_must_revoke;
    for code in [
        "navigation_unsupported",
        "target_not_foreground",
        "stale_observation",
        "command_expired",
        "navigation_target_refused",
        "navigation_cancelled",
    ] {
        assert!(!navigation_provider_must_revoke(&Err(
            crate::HostError::Protocol(format!("{code}: refused"))
        )));
    }
    for code in [
        "navigation_pairing_required",
        "navigation_outcome_unknown",
        "provider disconnected",
    ] {
        assert!(navigation_provider_must_revoke(&Err(
            crate::HostError::Protocol(code.into())
        )));
    }
    assert!(navigation_provider_must_revoke(&Ok(
        json!({"result":{"phase":"dispatched"}})
    )));
}

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
async fn pairing_claim_never_transfers_between_tasks_or_targets() {
    let registry = BrowserExtensionRegistry::new();
    let registered = registry
        .register(&hello(), "chrome-extension://published-extension/", 42)
        .await
        .unwrap();
    let id = registered["provider_id"].as_str().unwrap();
    assert!(
        registry
            .claim(id, 43, 77, "https://example.com", "bad-task")
            .await
            .is_err()
    );
    assert!(
        registry
            .claim(id, 42, 77, "https://evil.test", "bad-task")
            .await
            .is_err()
    );
    assert!(
        registry
            .claim(id, 42, 78, "https://example.com", "bad-task")
            .await
            .is_err()
    );
    registry
        .claim(id, 42, 77, "https://example.com", "task-one")
        .await
        .unwrap();
    registry
        .claim(id, 42, 77, "https://example.com", "task-one")
        .await
        .unwrap();
    assert!(
        registry
            .claim(id, 42, 77, "https://example.com", "task-two")
            .await
            .is_err()
    );
}

#[rstest]
#[tokio::test]
async fn rust_host_accepts_the_shared_extension_hello_fixture() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../../../browser-extension/chrome/protocol-v1.fixtures.json"
    ))
    .unwrap();
    let hello = fixtures["valid"]
        .as_array()
        .unwrap()
        .iter()
        .find(|fixture| fixture["type"] == "hello")
        .expect("shared hello fixture");

    BrowserExtensionRegistry::new()
        .register(hello, "chrome-extension://abcdefghijklmnop/", 42)
        .await
        .unwrap();
}

#[rstest]
#[tokio::test]
async fn rust_host_rejects_every_invalid_shared_hello_fixture() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../../../browser-extension/chrome/protocol-v1.fixtures.json"
    ))
    .unwrap();

    for hello in fixtures["invalid"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|fixture| fixture["type"] == "hello")
    {
        assert!(
            BrowserExtensionRegistry::new()
                .register(hello, "chrome-extension://abcdefghijklmnop/", 42)
                .await
                .is_err(),
            "invalid hello was accepted: {hello}"
        );
    }
}

#[rstest]
fn rust_host_validates_shared_extension_response_fixtures() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../../../browser-extension/chrome/protocol-v1.fixtures.json"
    ))
    .unwrap();
    for response in fixtures["valid"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|fixture| fixture["type"] == "response")
    {
        validate_extension_response(response).unwrap();
    }
    for response in fixtures["invalid"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|fixture| fixture["type"] == "response")
    {
        assert!(validate_extension_response(response).is_err());
    }
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
#[case(true)]
#[case(false)]
#[tokio::test]
async fn extension_navigation_requires_provider_capability_and_host_owned_deadline(
    #[case] valid_receipt: bool,
) {
    let registry = BrowserExtensionRegistry::new();
    let legacy = registry
        .register(&hello(), "chrome-extension://published-extension/", 42)
        .await
        .unwrap();
    let params = json!({"snapshot_id":"snapshot-1","url":"https://example.com/next"});
    let error = registry
        .call(
            legacy["provider_id"].as_str().unwrap(),
            "https://example.com",
            42,
            "navigate",
            &params,
            1000,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("navigation_unsupported"));
    let mut supported = hello();
    supported["capabilities"]
        .as_array_mut()
        .unwrap()
        .push(json!("tab_navigation_v1"));
    let registered = registry
        .register(&supported, "chrome-extension://published-extension/", 42)
        .await
        .unwrap();
    let id = registered["provider_id"].as_str().unwrap().to_owned();
    let secret = registered["provider_secret"].as_str().unwrap().to_owned();
    let mut forged = params.clone();
    forged["deadline_unix_ms"] = json!(u64::MAX);
    assert!(
        registry
            .call(&id, "https://example.com", 42, "navigate", &forged, 1000)
            .await
            .is_err()
    );
    let worker_registry = registry.clone();
    let worker_id = id.clone();
    let worker = tokio::spawn(async move {
        let command = worker_registry
            .next_command(&worker_id, &secret, 1000)
            .await
            .unwrap()["command"]
            .clone();
        assert!(command["deadline_unix_ms"].as_u64().unwrap() > 0);
        assert_eq!(
            command["deadline_unix_ms"],
            command["_dcc_deadline_unix_ms"]
        );
        worker_registry.complete(&worker_id, &secret, json!({"schema":"dcc-cua.browser-extension.v1","type":"response","request_id":command["request_id"],"ok":true,"result":{"phase":"dispatched","navigation_requested":true,"effect":"unverified","completion_known":!valid_receipt,"pairing_required":true,"fresh_observation_required":true}})).await.unwrap();
    });
    let result = registry
        .call(&id, "https://example.com", 42, "navigate", &params, 1000)
        .await;
    assert_eq!(result.is_ok(), valid_receipt);
    worker.await.unwrap();
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

impl BrowserExtensionRegistry {
    pub(crate) async fn register(
        &self,
        hello: &Value,
        invocation_origin: &str,
        browser_process_id: u32,
    ) -> Result<Value, crate::HostError> {
        self.register_with_window(hello, invocation_origin, browser_process_id, Some(77), None)
            .await
    }
}

#[rstest]
#[tokio::test]
async fn listener_constructor_claims_pairing_before_any_task_can_race() {
    let registry = BrowserExtensionRegistry::new();
    let registered = registry
        .register_with_window(
            &hello(),
            "chrome-extension://published-extension/",
            42,
            Some(77),
            Some("constructor-owner"),
        )
        .await
        .unwrap();
    let id = registered["provider_id"].as_str().unwrap();
    assert!(
        registry
            .claim(id, 42, 77, "https://example.com", "racing-task")
            .await
            .is_err()
    );
    registry
        .claim(id, 42, 77, "https://example.com", "constructor-owner")
        .await
        .unwrap();
}
