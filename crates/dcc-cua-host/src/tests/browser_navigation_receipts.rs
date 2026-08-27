use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use cua_driver_sdk::TrustedSessionOptions;
use cua_driver_sdk::remote::{
    DRIVER_ENVELOPE_VERSION, DriverChannelCapabilities, DriverEnvelopeChannel,
    DriverRequestEnvelope, DriverResponseEnvelope,
};
use dcc_cua_core::ComputerUseObservation;
use rstest::rstest;

use super::*;

#[derive(Clone)]
struct NavigationReceiptChannel {
    failure: Value,
    calls: Arc<Mutex<Vec<String>>>,
}

impl DriverEnvelopeChannel for NavigationReceiptChannel {
    fn negotiate<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<
        Box<dyn Future<Output = Result<DriverChannelCapabilities, String>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async {
            Ok(DriverChannelCapabilities {
                minimum_envelope_version: DRIVER_ENVELOPE_VERSION,
                maximum_envelope_version: DRIVER_ENVELOPE_VERSION,
                supports_cancellation: true,
            })
        })
    }

    fn exchange<'life0, 'async_trait>(
        &'life0 self,
        request: DriverRequestEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<DriverResponseEnvelope, String>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let name = request.name.clone().unwrap_or_default();
            self.calls
                .lock()
                .expect("record SDK tool call")
                .push(name.clone());
            let (structured, is_error) = match name.as_str() {
                "list_windows" => (
                    json!({"windows": [{
                        "pid": 42,
                        "window_id": 77,
                        "title": "Synthetic browser",
                        "app_name": "synthetic-test.exe",
                        "bounds": {"x": 0, "y": 0, "width": 800, "height": 600},
                        "is_foreground": true,
                        "minimized": false,
                        "is_on_screen": true
                    }]}),
                    false,
                ),
                "get_browser_state" => (
                    json!({
                        "status": "ok",
                        "target_id": "target-1",
                        "binding_quality": "exact",
                        "mutation_allowed": true
                    }),
                    false,
                ),
                "browser_navigate" => (self.failure.clone(), true),
                other => panic!("unexpected SDK tool call: {other}"),
            };
            Ok(DriverResponseEnvelope {
                envelope_version: DRIVER_ENVELOPE_VERSION,
                request_id: request.request_id,
                ok: true,
                result: Some(json!({
                    "content": [{"type": "text", "text": "bounded browser result"}],
                    "structuredContent": structured,
                    "isError": is_error
                })),
                error: None,
                error_code: None,
                completion_known: true,
            })
        })
    }

    fn bind_session<'life0, 'async_trait>(
        &'life0 self,
        _options: TrustedSessionOptions,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Arc<dyn DriverEnvelopeChannel>, String>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(Arc::new(self.clone()) as Arc<dyn DriverEnvelopeChannel>) })
    }

    fn close<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }

    fn cancel<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _request_id: &'life1 str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }

    fn authenticated_principal(&self) -> &str {
        "browser-navigation-receipt-test"
    }

    fn connection_generation(&self) -> &str {
        "generation-1"
    }
}

async fn navigate_through_host(failure: Value) -> Result<Value, HostError> {
    let channel = NavigationReceiptChannel {
        failure,
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    let driver = ComputerUseDriver::from_test_remote_channel(Arc::new(channel)).unwrap();
    let mut host = cached_host_session(&driver);
    host.allow_browser_input = true;
    host.session
        .seed_test_active_window(ComputerUseObservation {
            observation_id: "browser-observation-1".into(),
            window_handle: 77,
            process_id: 42,
            window_title: "Synthetic browser".into(),
            width: 800,
            height: 600,
            source_rect: [0, 0, 800, 600],
            capture_backend: "synthetic-test".into(),
            capture_provenance: json!({"source": "test-only"}),
            session_id: "runtime-session-1".into(),
        });
    let mut sessions = ConnectionSessions::default();
    sessions.windows.insert("session-1".into(), host);
    let mut transport = Some(SnapshotTransport::BinaryFrame);
    let mut shared_image = None;
    let cancellation = CancellationRegistry::default();

    handle_request(
        &driver,
        &mut sessions,
        &mut transport,
        &mut shared_image,
        &cancellation,
        serde_json::from_value(json!({
            "method": "browser_snapshot",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "grant-1",
                "window_capability": "capability-1",
                "request": {}
            }
        }))
        .unwrap(),
    )
    .await?;

    handle_request(
        &driver,
        &mut sessions,
        &mut transport,
        &mut shared_image,
        &cancellation,
        serde_json::from_value(json!({
            "method": "browser_navigate",
            "params": {
                "session_id": "session-1",
                "task_grant_id": "grant-1",
                "window_capability": "capability-1",
                "request": {
                    "target_id": "target-1",
                    "tab_id": "tab-1",
                    "url": "https://example.test/next",
                    "delivery_mode": "foreground"
                }
            }
        }))
        .unwrap(),
    )
    .await
    .map(|(response, _)| response)
}

#[rstest]
#[tokio::test]
async fn sdk_dispatch_failure_receipt_reaches_host_without_false_success() {
    let receipt = json!({
        "status": "error",
        "target_id": "target-1",
        "tab_id": "tab-1",
        "url": "https://example.test/next",
        "delivery_mode": "foreground",
        "dispatched": false,
        "activated": false,
        "activation_state": "not_started",
        "readback_state": "not_started",
        "refs_invalidated": false,
        "error_code": "navigation_dispatch_failed",
        "error": "browser navigation dispatch failed"
    });

    let response = navigate_through_host(receipt.clone())
        .await
        .expect("structured SDK failure receipt must reach Host");

    assert_eq!(response["type"], "browser_navigated");
    assert_eq!(response["result"]["structuredContent"], receipt);
    assert_eq!(response["result"]["structuredContent"]["status"], "error");
    assert!(response["result"]["content"].is_null());
}

#[rstest]
#[tokio::test]
async fn sdk_activation_failure_receipt_reaches_host_without_false_success() {
    let receipt = json!({
        "status": "error",
        "target_id": "target-1",
        "tab_id": "tab-1",
        "url": "https://example.test/next",
        "delivery_mode": "foreground",
        "dispatched": true,
        "activated": false,
        "activation_state": "failed",
        "readback_state": "not_started",
        "refs_invalidated": true,
        "error_code": "target_activation_failed",
        "error": "foreground target activation failed"
    });

    let response = navigate_through_host(receipt.clone())
        .await
        .expect("structured activation failure receipt must reach Host");

    assert_eq!(response["type"], "browser_navigated");
    assert_eq!(response["result"]["structuredContent"], receipt);
    assert_eq!(response["result"]["structuredContent"]["activated"], false);
}

#[rstest]
#[tokio::test]
async fn sdk_readback_failure_receipt_reaches_host_without_false_success() {
    let receipt = json!({
        "status": "error",
        "target_id": "target-1",
        "tab_id": "tab-1",
        "url": "https://example.test/next",
        "delivery_mode": "foreground",
        "dispatched": true,
        "activated": true,
        "activation_state": "succeeded",
        "readback_state": "failed",
        "refs_invalidated": true,
        "error_code": "navigation_readback_failed",
        "error": "foreground navigation readback failed"
    });

    let response = navigate_through_host(receipt.clone())
        .await
        .expect("structured readback failure receipt must reach Host");

    assert_eq!(response["type"], "browser_navigated");
    assert_eq!(response["result"]["structuredContent"], receipt);
    assert_eq!(
        response["result"]["structuredContent"]["readback_state"],
        "failed"
    );
}

#[rstest]
#[tokio::test]
async fn malformed_sdk_navigation_failure_still_fails_closed_before_host_publication() {
    let error = navigate_through_host(json!({
        "status": "error",
        "target_id": "target-drift",
        "tab_id": "tab-1",
        "delivery_mode": "foreground",
        "dispatched": true,
        "activated": false,
        "activation_state": "failed",
        "readback_state": "not_started",
        "refs_invalidated": true,
        "error_code": "target_activation_failed",
        "error": "foreground target activation failed"
    }))
    .await
    .expect_err("inconsistent SDK receipt must fail closed");

    let HostError::ComputerUse(error) = error else {
        panic!("expected typed Computer Use failure")
    };
    assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
    assert_eq!(
        error.message,
        "browser navigation receipt returned inconsistent target, tab, or delivery mode"
    );
}
