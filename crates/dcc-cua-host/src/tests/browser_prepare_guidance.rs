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
struct PrepareGuidanceChannel {
    prepared: Value,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl DriverEnvelopeChannel for PrepareGuidanceChannel {
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
                .push((name.clone(), request.arguments.clone().unwrap_or_default()));
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
                        "mutation_allowed": true,
                        "mode": "bind",
                        "tabs": [{"tab_id": "tab-1"}]
                    }),
                    false,
                ),
                "browser_prepare" => (self.prepared.clone(), false),
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
        "browser-prepare-guidance-test"
    }

    fn connection_generation(&self) -> &str {
        "generation-1"
    }
}

fn sdk_prepared_receipt() -> Value {
    // Pinned CUA tools.rs serializes this successful existing-profile receipt.
    json!({
        "status": "ok",
        "prepared": true,
        "action": "attached_existing_profile",
        "message": "Attached to the approved existing Chromium profile. Bind the native window again before using browser capabilities.",
        "endpoint_ownership": {"method": "listening_socket_pid", "owner_pid": 42, "detail": null},
        "prepared_pid": 42,
        "side_effects": {"opened_setup_page": true, "closed_setup_page": true},
        "attachment": {
            "kind": "existing_profile",
            "browser": "chromium",
            "capabilities_invalidated": true,
            "next_action": "get_browser_state"
        }
    })
}

fn prepare_sessions(driver: &ComputerUseDriver, allow_prepare: bool) -> ConnectionSessions {
    let mut host = cached_host_session(driver);
    host.allow_browser_prepare = allow_prepare;
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
    sessions
}

fn browser_request(method: &str, request: Value) -> Request {
    serde_json::from_value(json!({
        "method": method,
        "params": {
            "session_id": "session-1",
            "task_grant_id": "grant-1",
            "window_capability": "capability-1",
            "request": request
        }
    }))
    .unwrap()
}

#[rstest]
#[tokio::test]
async fn prepared_browser_hint_can_be_followed_through_the_same_host_bridge() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let receipt = sdk_prepared_receipt();
    let driver = ComputerUseDriver::from_test_remote_channel(Arc::new(PrepareGuidanceChannel {
        prepared: receipt.clone(),
        calls: Arc::clone(&calls),
    }))
    .unwrap();
    let mut sessions = prepare_sessions(&driver, true);
    let mut transport = Some(SnapshotTransport::BinaryFrame);
    let mut shared_image = None;
    let cancellation = CancellationRegistry::default();

    let (response, attachment) = handle_request(
        &driver,
        &mut sessions,
        &mut transport,
        &mut shared_image,
        &cancellation,
        browser_request(
            "browser_prepare",
            json!({"strategy": {"kind": "existing_profile"}}),
        ),
    )
    .await
    .unwrap();
    assert_eq!(response["type"], "browser_prepared");
    assert!(attachment.is_none());
    let mut expected = receipt;
    expected["attachment"]["next_action"] = json!("browser_snapshot");
    assert_eq!(response["result"]["structuredContent"], expected);
    let host = &sessions.windows["session-1"];
    assert_eq!(host.browser.target_id(), None);
    assert_eq!(host.browser.latest_snapshot_id(), None);
    assert_eq!(host.latest_observation_id, None);
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|(name, _)| name == "get_browser_state")
    );

    let next_action = response["result"]["structuredContent"]["attachment"]["next_action"]
        .as_str()
        .unwrap();
    let (bound, _) = handle_request(
        &driver,
        &mut sessions,
        &mut transport,
        &mut shared_image,
        &cancellation,
        browser_request(next_action, json!({})),
    )
    .await
    .unwrap();
    assert_eq!(bound["type"], "browser_snapshot");
    assert_eq!(bound["result"]["structuredContent"]["mode"], "bind");
    assert_eq!(
        bound["result"]["structuredContent"]["binding_quality"],
        "exact"
    );
    assert_eq!(
        bound["result"]["structuredContent"]["target_id"],
        "target-1"
    );
    assert_eq!(
        bound["result"]["structuredContent"]["tabs"][0]["tab_id"],
        "tab-1"
    );
    assert_eq!(
        sessions.windows["session-1"].browser.target_id(),
        Some("target-1")
    );
    let calls = calls.lock().unwrap();
    let browser_calls: Vec<_> = calls
        .iter()
        .filter(|(name, _)| name != "list_windows")
        .collect();
    assert_eq!(browser_calls.len(), 2);
    assert_eq!(browser_calls[0].0, "browser_prepare");
    assert_eq!(browser_calls[1].0, "get_browser_state");
    for (_, arguments) in browser_calls {
        assert_eq!(arguments["pid"], 42);
        assert_eq!(arguments["window_id"], 77);
        assert_eq!(arguments["session"], "runtime-session-1");
    }
}

#[rstest]
#[tokio::test]
async fn browser_prepare_guidance_does_not_bypass_the_prepare_grant() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let driver = ComputerUseDriver::from_test_remote_channel(Arc::new(PrepareGuidanceChannel {
        prepared: sdk_prepared_receipt(),
        calls: Arc::clone(&calls),
    }))
    .unwrap();
    let mut sessions = prepare_sessions(&driver, false);
    let error = handle_request(
        &driver,
        &mut sessions,
        &mut Some(SnapshotTransport::BinaryFrame),
        &mut None,
        &CancellationRegistry::default(),
        browser_request(
            "browser_prepare",
            json!({"strategy": {"kind": "existing_profile"}}),
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(error_code(&error), "browser_prepare_not_granted");
    assert!(calls.lock().unwrap().is_empty());
}

#[rstest]
#[case("existing_profile", true)]
#[case("isolated_new", false)]
#[tokio::test]
async fn refused_setup_reports_manual_recovery_without_followup_browser_calls(
    #[case] strategy: &str,
    #[case] expects_recovery: bool,
) {
    let receipt = json!({
        "status": "refused",
        "refusal": {
            "code": "browser_wrong_target_refused",
            "message": "the exact remote-debugging setup page did not become ready",
            "detail": {"setup_side_effects": {
                "opened_setup_page": true,
                "closed_setup_page": false,
                "enabled_remote_debugging": false
            }}
        }
    });
    let calls = Arc::new(Mutex::new(Vec::new()));
    let driver = ComputerUseDriver::from_test_remote_channel(Arc::new(PrepareGuidanceChannel {
        prepared: receipt.clone(),
        calls: Arc::clone(&calls),
    }))
    .unwrap();
    let mut sessions = prepare_sessions(&driver, true);
    let (response, attachment) = handle_request(
        &driver,
        &mut sessions,
        &mut Some(SnapshotTransport::BinaryFrame),
        &mut None,
        &CancellationRegistry::default(),
        browser_request("browser_prepare", json!({"strategy": {"kind": strategy}})),
    )
    .await
    .unwrap();
    assert!(attachment.is_none());
    let structured = &response["result"]["structuredContent"];
    assert_eq!(structured["status"], "refused");
    assert_eq!(structured["refusal"], receipt["refusal"]);
    assert_eq!(structured.get("recovery").is_some(), expects_recovery);
    if expects_recovery {
        let recovery = &structured["recovery"];
        assert_eq!(recovery["target"], json!({"pid": 42, "window_id": 77}));
        assert_eq!(recovery["cleanup"]["status"], "manual_cleanup_required");
        assert_eq!(recovery["automatic_cleanup"], false);
        assert_eq!(recovery["automatic_retry"], false);
        // The synthetic application is unknown; the adapter must not guess Edge.
        assert!(recovery["setup"].get("url").is_none());
    }
    let host = &sessions.windows["session-1"];
    assert_eq!(host.browser.target_id(), None);
    assert_eq!(host.browser.latest_snapshot_id(), None);
    assert_eq!(host.latest_observation_id, None);
    let calls = calls.lock().unwrap();
    let browser_calls: Vec<_> = calls
        .iter()
        .filter(|(name, _)| name != "list_windows")
        .collect();
    assert_eq!(browser_calls.len(), 1);
    assert_eq!(browser_calls[0].0, "browser_prepare");
    assert_eq!(browser_calls[0].1["pid"], 42);
    assert_eq!(browser_calls[0].1["window_id"], 77);
}
