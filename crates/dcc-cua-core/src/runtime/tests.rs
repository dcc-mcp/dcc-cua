use super::*;
use cua_driver_sdk::remote::{
    DRIVER_ENVELOPE_VERSION, DriverChannelCapabilities, DriverEnvelopeChannel,
    DriverRequestEnvelope, DriverResponseEnvelope,
};
use cua_driver_sdk::{CuaDriver, TrustedSessionOptions};
use rstest::rstest;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct OwnedBrowserRemoteChannel {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    prepare_result: Option<Value>,
}

#[async_trait::async_trait]
impl DriverEnvelopeChannel for OwnedBrowserRemoteChannel {
    async fn negotiate(&self) -> Result<DriverChannelCapabilities, String> {
        Ok(DriverChannelCapabilities {
            minimum_envelope_version: DRIVER_ENVELOPE_VERSION,
            maximum_envelope_version: DRIVER_ENVELOPE_VERSION,
            supports_cancellation: true,
        })
    }

    async fn exchange(
        &self,
        request: DriverRequestEnvelope,
    ) -> Result<DriverResponseEnvelope, String> {
        let name = request.name.clone().unwrap_or_default();
        let arguments = request.arguments.clone().unwrap_or_else(|| json!({}));
        self.calls
            .lock()
            .expect("record owned-browser request")
            .push((name.clone(), arguments));
        let structured = match name.as_str() {
            "browser_prepare" => self.prepare_result.clone().unwrap_or_else(|| {
                json!({
                    "status": "ok",
                    "action": "launched_isolated_browser",
                    "prepared_pid": 9081,
                    "side_effects": {"launched_browser": true, "created_profile": true}
                })
            }),
            "list_windows" => json!({
                "windows": [{
                    "pid": 9081,
                    "window_id": 771,
                    "title": "Task browser",
                    "app_name": "chrome.exe",
                    "bounds": {"x": 0, "y": 0, "width": 1200, "height": 800},
                    "is_foreground": false,
                    "minimized": false,
                    "is_on_screen": true
                }]
            }),
            "get_browser_state" => json!({
                "binding_quality": "exact",
                "mutation_allowed": true,
                "target_id": "owned-target-1",
                "tabs": [{"tab_id": "tab-1", "active": true}]
            }),
            _ => json!({"success": true}),
        };
        Ok(DriverResponseEnvelope {
            envelope_version: DRIVER_ENVELOPE_VERSION,
            request_id: request.request_id,
            ok: true,
            result: Some(json!({
                "content": [{"type": "text", "text": "ok"}],
                "structuredContent": structured,
                "isError": false
            })),
            error: None,
            error_code: None,
            completion_known: true,
        })
    }

    async fn bind_session(
        &self,
        _options: TrustedSessionOptions,
    ) -> Result<Arc<dyn DriverEnvelopeChannel>, String> {
        Ok(Arc::new(self.clone()))
    }

    async fn close(&self) -> Result<(), String> {
        Ok(())
    }

    async fn cancel(&self, _request_id: &str) -> Result<(), String> {
        Ok(())
    }

    fn authenticated_principal(&self) -> &str {
        "owned-browser-test"
    }

    fn connection_generation(&self) -> &str {
        "owned-browser-generation"
    }
}

#[rstest]
#[tokio::test]
async fn owned_browser_bootstrap_derives_exact_identity_without_caller_nominated_fields() {
    let channel = OwnedBrowserRemoteChannel::default();
    let calls = Arc::clone(&channel.calls);
    let raw = CuaDriver::connect_remote(Arc::new(channel)).expect("remote CUA driver");
    let driver = ComputerUseDriver::from_driver((raw, false));

    let binding = launch_owned_browser_target(
        &driver,
        ComputerUseOwnedBrowserLaunchSpec {
            browser: ComputerUseOwnedBrowserFamily::Chromium,
            profile: ComputerUseOwnedBrowserProfile::IsolatedNew,
        },
        "owned-session",
        "Agent controls task browser",
    )
    .await
    .unwrap();

    assert_eq!(binding.process_id, 9081);
    assert_eq!(binding.window_handle, 771);
    assert_eq!(binding.target_id, "owned-target-1");
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        [
            "start_session",
            "browser_prepare",
            "list_windows",
            "get_browser_state"
        ]
    );
    let prepare = &calls[1].1;
    assert_eq!(prepare["allow_launch"], true);
    assert_eq!(prepare["profile"]["mode"], "isolated_new");
    for forbidden in [
        "pid",
        "window_id",
        "executable",
        "profile_path",
        "cdp_endpoint",
    ] {
        assert!(prepare.get(forbidden).is_none());
    }
}

#[rstest]
#[tokio::test]
async fn owned_browser_bootstrap_preserves_safe_refusal_reason_and_never_binds() {
    let channel = OwnedBrowserRemoteChannel {
        prepare_result: Some(json!({
            "status": "refused",
            "refusal": {
                "code": "browser_consent_required",
                "message": "private profile path must not be exposed",
                "detail": {"profile_path": "private-path", "endpoint": "private-endpoint"}
            }
        })),
        ..Default::default()
    };
    let calls = Arc::clone(&channel.calls);
    let raw = CuaDriver::connect_remote(Arc::new(channel)).expect("remote CUA driver");
    let driver = ComputerUseDriver::from_driver((raw, false));

    let error = launch_owned_browser_target(
        &driver,
        ComputerUseOwnedBrowserLaunchSpec {
            browser: ComputerUseOwnedBrowserFamily::Chromium,
            profile: ComputerUseOwnedBrowserProfile::IsolatedNew,
        },
        "owned-session",
        "Agent controls task browser",
    )
    .await
    .err()
    .expect("refused preparation cannot bind");

    assert_eq!(error.code, ComputerUseErrorCode::BrowserRefused);
    assert!(error.message.contains("browser_consent_required"));
    assert!(!error.message.contains("private"));
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["start_session", "browser_prepare"]
    );
}

fn successful_owned_browser_prepare() -> Value {
    json!({
        "status": "ok",
        "action": "launched_isolated_browser",
        "prepared_pid": 9081,
        "side_effects": {"launched_browser": true, "created_profile": true}
    })
}

#[rstest]
#[case("/status", json!("refused"))]
#[case("/action", json!("attached_existing_profile"))]
#[case("/side_effects/launched_browser", json!(false))]
#[case("/side_effects/created_profile", json!(false))]
#[case("/side_effects/created_profile", json!("true"))]
#[case("/side_effects/created_profile", Value::Null)]
fn owned_browser_prepare_rejects_invalid_ownership_evidence(
    #[case] path: &str,
    #[case] value: Value,
) {
    let mut prepared = successful_owned_browser_prepare();
    *prepared.pointer_mut(path).unwrap() = value;
    let error = owned_browser_prepare_pid(&prepared).expect_err("ownership must be proven");
    assert_eq!(error.code, ComputerUseErrorCode::BrowserRefused);
    assert!(error.message.contains(path));
}

#[rstest]
#[case(Value::Null)]
#[case(json!(0))]
#[case(json!(-1))]
#[case(json!(u64::from(u32::MAX) + 1))]
#[case(json!("9081"))]
fn owned_browser_prepare_rejects_invalid_pid(#[case] pid: Value) {
    let mut prepared = successful_owned_browser_prepare();
    prepared["prepared_pid"] = pid;
    assert_eq!(
        owned_browser_prepare_pid(&prepared).unwrap_err().code,
        ComputerUseErrorCode::BackendUnavailable
    );
}

#[rstest]
fn owned_browser_prepare_diagnostics_do_not_copy_untrusted_fields() {
    let error = owned_browser_prepare_pid(&json!({
        "status": "private-path",
        "refusal": {"code": "private-endpoint", "message": "private-message"}
    }))
    .unwrap_err();
    assert!(error.message.contains("/status=unexpected_value"));
    assert!(error.message.contains("/action=missing"));
    assert!(error.message.contains("unrecognized_refusal"));
    assert!(!error.message.contains("private"));
}

#[rstest]
fn owned_browser_prepare_route_refusal_explains_protected_installation_requirement() {
    let error = owned_browser_prepare_pid(&json!({
        "status": "refused",
        "refusal": {"code": "browser_route_unavailable"}
    }))
    .unwrap_err();
    assert!(error.message.contains("browser_route_unavailable"));
    assert_eq!(
        error.message.contains("standard-user runtime"),
        cfg!(windows)
    );
    assert_eq!(
        error.message.contains("If the runtime is elevated"),
        cfg!(windows)
    );
}

fn active_desktop_session() -> ComputerUseDesktopSession {
    let mut session = ComputerUseDesktopSession::new(
        ComputerUseDriver::create().expect("test driver"),
        "Agent".into(),
        "desktop-test".into(),
    )
    .expect("desktop session");
    session.active = true;
    session
}

fn desktop_click(observation_id: Option<&str>) -> ComputerUseAction {
    ComputerUseAction {
        action: "click".into(),
        x: Some(10.0),
        y: Some(20.0),
        observation_id: observation_id.map(str::to_owned),
        ..Default::default()
    }
}

#[rstest]
#[tokio::test]
async fn desktop_action_without_any_snapshot_is_rejected() {
    let mut session = active_desktop_session();

    let error = session
        .perform_action(&desktop_click(None))
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
}

#[rstest]
#[tokio::test]
async fn desktop_action_cannot_reuse_an_observation_after_it_is_consumed() {
    let mut session = active_desktop_session();
    session.latest_observation_id = None;

    let error = session
        .perform_action(&desktop_click(Some("consumed-observation")))
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
}
