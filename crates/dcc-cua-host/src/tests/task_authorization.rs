use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dcc_cua_core::{ComputerUseTargetAvailability, ComputerUseTargetStatus};
use rstest::rstest;

use super::*;
use crate::action_confirmation::ConfirmationBinding;
use crate::task_authorization::{
    TaskAuthorizationBinding, TaskAuthorizationOutcome, authorize_task_scoped_action,
    issue_task_authorization,
};

// This channel cannot reach a desktop or native driver. Even a regression that
// crosses a Host authorization boundary records a failure rather than input.
#[derive(Clone, Default)]
struct ObservationOnlyTestChannel {
    exchanges: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl cua_driver_sdk::remote::DriverEnvelopeChannel for ObservationOnlyTestChannel {
    async fn negotiate(&self) -> Result<cua_driver_sdk::remote::DriverChannelCapabilities, String> {
        Ok(cua_driver_sdk::remote::DriverChannelCapabilities {
            minimum_envelope_version: cua_driver_sdk::remote::DRIVER_ENVELOPE_VERSION,
            maximum_envelope_version: cua_driver_sdk::remote::DRIVER_ENVELOPE_VERSION,
            supports_cancellation: true,
        })
    }
    async fn exchange(
        &self,
        _: cua_driver_sdk::remote::DriverRequestEnvelope,
    ) -> Result<cua_driver_sdk::remote::DriverResponseEnvelope, String> {
        self.exchanges
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err("observation-only authorization test forbids all driver dispatch".into())
    }
    async fn bind_session(
        &self,
        _: cua_driver_sdk::TrustedSessionOptions,
    ) -> Result<Arc<dyn cua_driver_sdk::remote::DriverEnvelopeChannel>, String> {
        Ok(Arc::new(self.clone()))
    }
    async fn close(&self) -> Result<(), String> {
        Ok(())
    }
    async fn cancel(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn authenticated_principal(&self) -> &str {
        "observation-only-pure-test"
    }
    fn connection_generation(&self) -> &str {
        "generation-1"
    }
}

async fn observation_only_lease() -> (
    TrustedTaskAuthorizationIssuer,
    Arc<dyn TrustedTaskAuthorizationHost>,
    TrustedTaskAuthorizationLease,
) {
    let (issuer, authority) = trusted_task_authorization_broker();
    let mut registration = browser_credential_registration(unix_time_millis() + 60_000);
    registration.connection_id = Some("connection-test".into());
    registration.task_id = Some("session-1".into());
    registration.application_label = "Test DCC".into();
    registration.target = TrustedTaskAuthorizationTarget::ExactWindow {
        process_id: 42,
        window_handle: 77,
    };
    // Method names alone must never create an action scope, even if a caller
    // declares input/minimize methods alongside observation methods.
    registration.allowed_host_methods = vec![
        "snapshot".into(),
        "get_window_state".into(),
        "change_window_state".into(),
        "execute_action".into(),
        "minimize_window".into(),
    ];
    registration.allowed_actions.clear();
    registration.allowed_browser_origins.clear();
    let receipt = issuer.register(registration).unwrap();
    let lease = issue_task_authorization(
        Some(authority.as_ref()),
        TaskAuthorizationBinding::window(
            "connection-test",
            &receipt.authorization_id,
            "session-1",
            "grant-1",
            "Test DCC",
            &receipt.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 77,
            },
        ),
    )
    .await
    .unwrap();
    (issuer, authority, lease)
}

#[rstest]
#[tokio::test]
async fn observation_only_authorization_registers_starts_and_refuses_all_input_and_minimize() {
    let (_issuer, authority, lease) = observation_only_lease().await;
    assert!(lease.allowed_actions.is_empty());
    crate::task_authorization::validate_active_task_authorization(
        Some(authority.as_ref()),
        Some(&lease),
    )
    .await
    .unwrap();
    let channel = ObservationOnlyTestChannel::default();
    let driver = ComputerUseDriver::from_test_remote_channel(Arc::new(channel.clone())).unwrap();
    let mut host = cached_host_session(&driver);
    host.observation_mode = TaskObservationMode::PixelsOnly;
    host.capability = lease.window_capability.clone();
    host.task_authorization_host = Some(authority);
    host.task_authorization = Some(lease.clone());
    host.allow_raw_input = true; // Stronger negative: a boolean cannot invent scopes.
    host.require_task_authorized_method("snapshot").unwrap();
    host.require_task_authorized_method("get_window_state")
        .unwrap();
    let mut sessions = ConnectionSessions {
        connection_id: "connection-test".into(),
        ..ConnectionSessions::default()
    };
    sessions.windows.insert("session-1".into(), host);
    for action_name in TrustedTaskActionScope::PIXELS_INPUT_ACTIONS {
        let action: HostAction = serde_json::from_value(
            json!({"action": action_name, "input_kind":"raw_input", "intent":"ordinary_edit"}),
        )
        .unwrap();
        assert!(
            sessions.windows["session-1"]
                .require_pixels_input_grant("session-1", &action)
                .is_err()
        );
        let request = serde_json::from_value(json!({"method":"execute_action", "params":{
            "session_id":"session-1", "task_grant_id":"grant-1", "window_capability":lease.window_capability,
            "observation_id":"observation-before-transition", "action":action, "capture_after":false
        }})).unwrap();
        let error = handle_request(
            &driver,
            &mut sessions,
            &mut Some(SnapshotTransport::BinaryFrame),
            &mut None,
            &CancellationRegistry::default(),
            request,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                error,
                HostError::CodedProtocol {
                    code: HostProtocolErrorCode::TaskAuthorizationDenied,
                    ..
                }
            ),
            "{error:?}"
        );
    }
    assert!(
        sessions.windows["session-1"]
            .require_minimize_grant("session-1")
            .is_err()
    );
    let request = serde_json::from_value(json!({"method":"minimize_window", "params":{
        "session_id":"session-1", "task_grant_id":"grant-1", "window_capability":lease.window_capability,
        "observation_id":"observation-before-transition"
    }})).unwrap();
    let error = handle_request(
        &driver,
        &mut sessions,
        &mut Some(SnapshotTransport::BinaryFrame),
        &mut None,
        &CancellationRegistry::default(),
        request,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            error,
            HostError::CodedProtocol {
                code: HostProtocolErrorCode::TaskAuthorizationDenied,
                ..
            }
        ),
        "{error:?}"
    );
    assert_eq!(
        channel.exchanges.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[rstest]
#[tokio::test]
async fn observation_only_authorization_retains_exact_expiry_revocation_and_validator_fences() {
    let (issuer, authority, lease) = observation_only_lease().await;
    for field in [
        "connection",
        "session",
        "grant",
        "capability",
        "pid",
        "hwnd",
        "digest",
    ] {
        let mut foreign = lease.clone();
        match field {
            "connection" => foreign.connection_id.push_str("-foreign"),
            "session" => foreign.session_id.push_str("-foreign"),
            "grant" => foreign.task_grant_id.push_str("-foreign"),
            "capability" => foreign.window_capability.push_str("-foreign"),
            "pid" => foreign.target_process_id += 1,
            "hwnd" => foreign.target_window_handle += 1,
            "digest" => foreign.request_digest.push_str("-foreign"),
            _ => unreachable!(),
        }
        assert!(
            crate::task_authorization::validate_active_task_authorization(
                Some(authority.as_ref()),
                Some(&foreign)
            )
            .await
            .is_err(),
            "{field}"
        );
    }
    let mut expired = lease.clone();
    expired.expires_at_unix_ms = unix_time_millis().saturating_sub(1);
    assert!(
        crate::task_authorization::validate_active_task_authorization(
            Some(authority.as_ref()),
            Some(&expired)
        )
        .await
        .is_err()
    );
    assert!(
        crate::task_authorization::validate_active_task_authorization(None, Some(&lease))
            .await
            .is_err()
    );
    issuer.revoke(&lease.authorization_id).unwrap();
    let error = crate::task_authorization::validate_active_task_authorization(
        Some(authority.as_ref()),
        Some(&lease),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        HostError::CodedProtocol {
            code: HostProtocolErrorCode::TaskAuthorizationRevoked,
            ..
        }
    ));
}

#[rstest]
fn native_frame_action_scope_is_closed_and_grants_no_raw_input() {
    let value = json!({"action":"set_window_frame","input_kind":"window_state","secret_input":false,"authorization_category":"window_state"});
    let scope: TrustedTaskActionScope = serde_json::from_value(value.clone()).unwrap();
    assert!(scope.validate());
    assert!(scope.is_window_frame());
    assert!(!scope.is_window_minimize());
    assert!(!scope.is_pixels_input());
    for (field, replacement) in [
        ("action", json!("activate")),
        ("input_kind", json!("raw_input")),
        ("secret_input", json!(true)),
        ("authorization_category", json!("raw_input")),
        ("browser_origin", json!("https://example.com")),
    ] {
        let mut changed = value.clone();
        changed[field] = replacement;
        let invalid: TrustedTaskActionScope = serde_json::from_value(changed).unwrap();
        assert!(!invalid.is_window_frame());
        assert!(!invalid.validate());
    }
}

#[rstest]
fn native_frame_host_pre_dispatch_refusal_preserves_known_completion() {
    let result = crate::request_handler::native_frame_pre_dispatch_failure(HostError::ComputerUse(
        ComputerUseError::new(
            ComputerUseErrorCode::UserInterrupted,
            "stopped before queue dispatch",
        ),
    ));
    let HostError::ComputerUse(error) = result else {
        panic!("typed failure required")
    };
    let details = error.details.unwrap();
    assert_eq!(details.action_attempted, Some(false));
    assert_eq!(
        details.completion,
        Some(dcc_cua_core::ComputerUseCompletionState::Known)
    );
    assert_eq!(details.effect_unknown, Some(false));
    assert_eq!(details.blind_retry, Some(false));
}

async fn native_frame_test_lease() -> (
    TrustedTaskAuthorizationIssuer,
    Arc<dyn TrustedTaskAuthorizationHost>,
    TrustedTaskAuthorizationLease,
) {
    let (issuer, authority) = trusted_task_authorization_broker();
    let mut registration = browser_credential_registration(unix_time_millis() + 60_000);
    registration.application_label = "Test DCC".into();
    registration.target = TrustedTaskAuthorizationTarget::ExactWindow {
        process_id: 42,
        window_handle: 77,
    };
    registration.allowed_host_methods = vec!["get_window_state".into(), "set_window_frame".into()];
    registration.allowed_actions=vec![serde_json::from_value(json!({"action":"set_window_frame","input_kind":"window_state","secret_input":false,"authorization_category":"window_state"})).unwrap()];
    registration.allowed_browser_origins.clear();
    let receipt = issuer.register(registration).unwrap();
    let lease = issue_task_authorization(
        Some(authority.as_ref()),
        TaskAuthorizationBinding::window(
            "connection-test",
            &receipt.authorization_id,
            "session-1",
            "grant-1",
            "Test DCC",
            &receipt.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 77,
            },
        ),
    )
    .await
    .unwrap();
    (issuer, authority, lease)
}

#[rstest]
#[tokio::test]
async fn native_frame_broker_lease_rejects_foreign_expired_and_revoked_bindings() {
    let (issuer, authority, lease) = native_frame_test_lease().await;
    let permitted = |candidate: &TrustedTaskAuthorizationLease| {
        crate::session_state::native_frame_lease_matches_binding(
            Some(candidate),
            "session-1",
            "grant-1",
            &lease.window_capability,
            42,
            77,
        )
    };
    assert!(permitted(&lease));
    for field in 0..8 {
        let mut changed = lease.clone();
        match field {
            0 => changed.session_id.push('x'),
            1 => changed.task_grant_id.push('x'),
            2 => changed.window_capability.push('x'),
            3 => changed.target_process_id += 1,
            4 => changed.target_window_handle += 1,
            5 => changed
                .allowed_host_methods
                .retain(|m| m != "get_window_state"),
            6 => changed
                .allowed_host_methods
                .retain(|m| m != "set_window_frame"),
            _ => changed.allowed_actions.clear(),
        }
        assert!(!permitted(&changed), "field {field}");
    }
    crate::task_authorization::validate_active_task_authorization(
        Some(authority.as_ref()),
        Some(&lease),
    )
    .await
    .unwrap();
    let mut expired = lease.clone();
    expired.expires_at_unix_ms = unix_time_millis().saturating_sub(1);
    assert!(
        crate::task_authorization::validate_active_task_authorization(
            Some(authority.as_ref()),
            Some(&expired)
        )
        .await
        .is_err()
    );
    issuer.revoke(&lease.authorization_id).unwrap();
    assert!(
        crate::task_authorization::validate_active_task_authorization(
            Some(authority.as_ref()),
            Some(&lease)
        )
        .await
        .is_err()
    );
}

#[rstest]
#[tokio::test]
async fn native_frame_host_refuses_missing_metadata_without_driver_dispatch() {
    let (_issuer, authority, lease) = native_frame_test_lease().await;
    let channel = ObservationOnlyTestChannel::default();
    let driver = ComputerUseDriver::from_test_remote_channel(Arc::new(channel.clone())).unwrap();
    let mut host = cached_host_session(&driver);
    host.observation_mode = TaskObservationMode::PixelsOnly;
    host.capability = lease.window_capability.clone();
    host.task_authorization = Some(lease.clone());
    host.task_authorization_host = Some(authority);
    host.require_window_frame_grant("session-1").unwrap();
    assert!(host.require_minimize_grant("session-1").is_err());
    for action in TrustedTaskActionScope::PIXELS_INPUT_ACTIONS {
        let action = serde_json::from_value(
            json!({"action":action,"input_kind":"raw_input","intent":"ordinary_edit"}),
        )
        .unwrap();
        assert!(
            host.require_pixels_input_grant("session-1", &action)
                .is_err()
        );
    }
    let mut sessions = ConnectionSessions {
        connection_id: "connection-test".into(),
        ..ConnectionSessions::default()
    };
    sessions.windows.insert("session-1".into(), host);
    let request=serde_json::from_value(json!({"method":"set_window_frame","params":{
        "session_id":"session-1","task_grant_id":"grant-1","window_capability":lease.window_capability,
        "frame":{"x":50,"y":800,"width":926,"height":680}}})).unwrap();
    assert!(
        handle_request(
            &driver,
            &mut sessions,
            &mut Some(SnapshotTransport::BinaryFrame),
            &mut None,
            &CancellationRegistry::default(),
            request
        )
        .await
        .is_err()
    );
    assert!(
        sessions.windows["session-1"]
            .latest_observation_id
            .is_none()
    );
    assert_eq!(
        channel.exchanges.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[rstest]
fn native_frame_available_transition_keeps_old_pixels_invalid_without_reconstructing_metadata() {
    let channel = ObservationOnlyTestChannel::default();
    let driver = ComputerUseDriver::from_test_remote_channel(Arc::new(channel.clone())).unwrap();
    let mut host = cached_host_session(&driver);
    host.observe_target_availability(ComputerUseTargetAvailability {
        status: ComputerUseTargetStatus::Unavailable,
        code: "target_unavailable".into(),
        visible: false,
        minimized: false,
        foreground: false,
    });
    host.latest_observation_id = Some("old-pixel".into());
    assert!(host.observe_target_state(
        &json!({"exists":true,"visible":true,"minimized":false,"foreground":false,
        "process_id":42,"window_handle":77,"window_state_id":"untrusted-state-id"})
    ));
    assert!(host.latest_observation_id.is_none());
    assert_eq!(
        channel.exchanges.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

struct TaskAuthorizationHost {
    revoked: bool,
}

struct DenyingTaskAuthorizationHost;

async fn pixel_input_host(actions: &[&str]) -> HostSession {
    let (issuer, authority) = trusted_task_authorization_broker();
    let mut registration = browser_credential_registration(unix_time_millis() + 60_000);
    registration.application_label = "Test DCC".into();
    registration.target = TrustedTaskAuthorizationTarget::ExactWindow {
        process_id: 42,
        window_handle: 77,
    };
    registration.allowed_host_methods = vec!["snapshot".into(), "execute_action".into()];
    registration.allowed_actions = actions
        .iter()
        .map(|action| TrustedTaskActionScope {
            action: (*action).into(),
            input_kind: "raw_input".into(),
            secret_input: false,
            authorization_category: "raw_input".into(),
            browser_origin: None,
        })
        .collect();
    registration.allowed_browser_origins.clear();
    let receipt = issuer.register(registration).unwrap();
    let lease = issue_task_authorization(
        Some(authority.as_ref()),
        TaskAuthorizationBinding::window(
            "connection-test",
            &receipt.authorization_id,
            "session-1",
            "grant-1",
            "Test DCC",
            &receipt.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 77,
            },
        ),
    )
    .await
    .unwrap();
    let driver = ComputerUseDriver::create().unwrap();
    let mut host = cached_host_session(&driver);
    host.observation_mode = TaskObservationMode::PixelsOnly;
    host.capability = receipt.window_capability;
    host.task_authorization = Some(lease);
    host.task_authorization_host = Some(authority);
    host
}

#[rstest]
#[tokio::test]
async fn pixel_input_uses_real_scope_and_rejects_missing_stale_or_foreign_bindings() {
    let mut host = pixel_input_host(&["click"]).await;
    let action: HostAction = serde_json::from_value(json!({"action":"click", "input_kind":"raw_input", "intent":"ordinary_edit", "x":30, "y":40})).unwrap();
    host.require_pixels_input_grant("session-1", &action)
        .unwrap();
    assert!(
        host.require_pixels_input_grant("other-session", &action)
            .is_err()
    );
    host.target_process_id += 1;
    assert!(
        host.require_pixels_input_grant("session-1", &action)
            .is_err()
    );
    host.target_process_id -= 1;
    host.target_window_handle += 1;
    assert!(
        host.require_pixels_input_grant("session-1", &action)
            .is_err()
    );
    host.target_window_handle -= 1;
    host.allow_raw_input = false;
    assert!(
        host.require_pixels_input_grant("session-1", &action)
            .is_err()
    );
    host.allow_raw_input = true;
    let capability = host.capability.clone();
    let mut sessions = ConnectionSessions::default();
    sessions.windows.insert("session-1".into(), host);
    for (id, allowed) in [
        ("", false),
        ("stale", false),
        ("observation-before-transition", true),
    ] {
        let request: Request = serde_json::from_value(json!({"method":"execute_action", "params":{
            "session_id":"session-1", "task_grant_id":"grant-1", "window_capability":capability,
            "observation_id":id, "action":{"action":"click", "input_kind":"raw_input", "intent":"ordinary_edit", "x":30,"y":40}
        }})).unwrap();
        assert_eq!(
            crate::task_authorization_scope::enforce_task_authorized_method(
                &mut sessions,
                &request
            )
            .is_ok(),
            allowed
        );
    }
}

#[rstest]
#[tokio::test]
async fn pixel_input_stop_while_queued_refuses_before_any_core_attempt() {
    let mut host = pixel_input_host(&["click"]).await;
    let queue = tokio::sync::Mutex::new(());
    let held_turn = queue.lock().await;
    let stopped = std::cell::Cell::new(false);
    let core_attempted = std::cell::Cell::new(false);
    let queued = async {
        let _turn = queue.lock().await;
        // The connection's stop latch changed while this request was queued.
        host.interrupted = stopped.get();
        crate::request_handler::revalidate_queued_window_mutation(&mut host).await?;
        core_attempted.set(true);
        Ok::<_, HostError>(())
    };
    tokio::pin!(queued);
    std::future::poll_fn(|context| {
        assert!(std::future::Future::poll(queued.as_mut(), context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    stopped.set(true);
    drop(held_turn);
    let error = queued.await.unwrap_err();
    assert!(
        matches!(error, HostError::ComputerUse(ref error) if error.code == ComputerUseErrorCode::UserInterrupted)
    );
    assert!(!core_attempted.get());
}

#[rstest]
#[tokio::test]
async fn pixel_input_cannot_widen_a_click_grant_or_borrow_semantic_tokens() {
    let host = pixel_input_host(&["click"]).await;
    let base = json!({"action":"click", "input_kind":"raw_input", "intent":"ordinary_edit", "x":30, "y":40});
    for (field, value) in [
        ("action", json!("keypress")),
        ("action", json!("drag")),
        ("action", json!("scroll")),
        ("input_kind", json!("semantic")),
        ("element_index", json!(1)),
        ("element_token", json!("foreign")),
        ("secret_handle", json!("fixture-secret")),
        ("input_backend_id", json!("post_message")),
        ("delivery_mode", json!("background")),
    ] {
        let mut value_action = base.clone();
        value_action[field] = value;
        let action: HostAction = serde_json::from_value(value_action).unwrap();
        assert!(
            host.require_pixels_input_grant("session-1", &action)
                .is_err(),
            "field {field}"
        );
    }
}

#[rstest]
fn pixel_input_scope_is_a_strict_subset_of_raw_input() {
    for action in TrustedTaskActionScope::PIXELS_INPUT_ACTIONS {
        let scope = TrustedTaskActionScope {
            action: (*action).into(),
            input_kind: "raw_input".into(),
            secret_input: false,
            authorization_category: "raw_input".into(),
            browser_origin: None,
        };
        assert!(scope.validate());
        assert!(scope.is_pixels_input());
    }
    let mut scope = TrustedTaskActionScope {
        action: "click".into(),
        input_kind: "raw_input".into(),
        secret_input: false,
        authorization_category: "raw_input".into(),
        browser_origin: None,
    };
    scope.secret_input = true;
    assert!(!scope.is_pixels_input());
    scope.secret_input = false;
    scope.authorization_category = "credential".into();
    assert!(!scope.is_pixels_input());
}

#[rstest]
fn native_minimize_action_scope_is_closed_and_does_not_grant_input() {
    let scope = json!({"action":"minimize_window", "input_kind":"window_state", "secret_input":false, "authorization_category":"window_state"});
    let parsed: TrustedTaskActionScope = serde_json::from_value(scope.clone()).unwrap();
    assert!(parsed.is_window_minimize());
    assert!(parsed.validate());
    for (field, value) in [
        ("action", json!("restore_activate")),
        ("input_kind", json!("raw_input")),
        ("secret_input", json!(true)),
        ("authorization_category", json!("raw_input")),
        ("browser_origin", json!("https://example.com")),
    ] {
        let mut invalid = scope.clone();
        invalid[field] = value;
        let invalid: TrustedTaskActionScope = serde_json::from_value(invalid).unwrap();
        assert!(!invalid.is_window_minimize());
        assert!(!invalid.validate());
    }
}

#[rstest]
#[case("https://EXAMPLE.com")]
#[case("https://example.com/")]
#[case("https://example.com:443")]
#[case("https://user@example.com")]
#[case("https://example.com/path")]
#[case("https://example.com?query=1")]
#[case("javascript:alert(1)")]
fn browser_origins_must_be_exact_canonical_http_origins(#[case] origin: &str) {
    assert!(!crate::task_authorization::valid_browser_origin(origin));
    assert!(crate::task_authorization::valid_browser_origin(
        "https://example.com"
    ));
}

#[async_trait::async_trait]
impl TrustedTaskAuthorizationHost for DenyingTaskAuthorizationHost {
    async fn authorize(
        &self,
        _request: TrustedTaskAuthorizationRequest,
    ) -> Result<TrustedTaskAuthorizationLease, TrustedTaskAuthorizationHostError> {
        Err(TrustedTaskAuthorizationHostError::Denied)
    }

    async fn validate(
        &self,
        _request: TrustedTaskAuthorizationValidationRequest,
    ) -> Result<TrustedTaskAuthorizationValidationDecision, TrustedTaskAuthorizationHostError> {
        panic!("a denied task authorization must not be validated")
    }

    async fn validate_lease(
        &self,
        _request: TrustedTaskAuthorizationLeaseValidationRequest,
    ) -> Result<TrustedTaskAuthorizationValidationDecision, TrustedTaskAuthorizationHostError> {
        panic!("a denied task authorization must not be validated")
    }
}

#[async_trait::async_trait]
impl TrustedTaskAuthorizationHost for TaskAuthorizationHost {
    async fn authorize(
        &self,
        request: TrustedTaskAuthorizationRequest,
    ) -> Result<TrustedTaskAuthorizationLease, TrustedTaskAuthorizationHostError> {
        let now = unix_time_millis();
        Ok(TrustedTaskAuthorizationLease {
            connection_id: request.connection_id,
            authorization_id: request.authorization_id,
            session_id: request.session_id,
            task_grant_id: request.task_grant_id,
            application_label: request.application_label,
            window_capability: request.window_capability,
            target_process_id: request.target_process_id,
            target_window_handle: request.target_window_handle,
            allowed_host_methods: vec!["execute_action".into(), "snapshot".into()],
            allowed_actions: vec![TrustedTaskActionScope {
                action: "type_chars".into(),
                input_kind: "raw_input".into(),
                secret_input: false,
                authorization_category: "raw_input".into(),
                browser_origin: None,
            }],
            allowed_browser_origins: Vec::new(),
            browser_scope: None,
            recording_output_dir: None,
            capture_preparation: None,
            issued_at_unix_ms: now,
            expires_at_unix_ms: now + 60_000,
            request_digest: request.request_digest,
        })
    }

    async fn validate(
        &self,
        request: TrustedTaskAuthorizationValidationRequest,
    ) -> Result<TrustedTaskAuthorizationValidationDecision, TrustedTaskAuthorizationHostError> {
        Ok(TrustedTaskAuthorizationValidationDecision {
            status: if self.revoked {
                TrustedTaskAuthorizationStatus::Revoked
            } else {
                TrustedTaskAuthorizationStatus::Active
            },
            request_digest: request.request_digest,
        })
    }

    async fn validate_lease(
        &self,
        request: TrustedTaskAuthorizationLeaseValidationRequest,
    ) -> Result<TrustedTaskAuthorizationValidationDecision, TrustedTaskAuthorizationHostError> {
        Ok(TrustedTaskAuthorizationValidationDecision {
            status: if self.revoked {
                TrustedTaskAuthorizationStatus::Revoked
            } else {
                TrustedTaskAuthorizationStatus::Active
            },
            request_digest: request.request_digest,
        })
    }
}

fn unix_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[rstest]
#[tokio::test]
async fn native_recording_authorization_binds_directory_target_and_video_before_core() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-recording-scope-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    #[cfg(not(windows))]
    let directory = std::fs::canonicalize(&directory)
        .expect("canonicalize pre-created recording authorization fixture directory");
    let output = directory.to_str().unwrap().to_owned();
    let (issuer, authority) = trusted_task_authorization_broker();
    let mut registration = browser_credential_registration(unix_time_millis() + 60_000);
    registration.connection_id = Some("connection-test".into());
    registration.task_id = Some("session-1".into());
    registration.application_label = "Test DCC".into();
    registration.target = TrustedTaskAuthorizationTarget::ExactWindow {
        process_id: 42,
        window_handle: 77,
    };
    registration.allowed_host_methods = vec![
        "recording_start".into(),
        "recording_state".into(),
        "recording_stop".into(),
    ];
    registration.allowed_actions.clear();
    registration.allowed_browser_origins.clear();
    registration.recording_output_dir = Some(output.clone());
    let receipt = issuer.register(registration).unwrap();
    let security = HostSecurityServices::default().with_task_authorization_host(authority.clone());
    let grant_value = json!({"task_grant_id":"grant-1","application_label":"Test DCC",
        "observation_mode":"pixels_only","process_id":42,"window_handle":77,
        "allow_recording":true,"allow_live_observation":true,"recording_output_dir":output,
        "task_authorization_id":receipt.authorization_id,"task_authorization_window_capability":receipt.window_capability});
    let grant: TaskGrant = serde_json::from_value(grant_value.clone()).unwrap();
    grant.validate_identity().unwrap();
    let lease = crate::task_authorization_scope::preauthorize_task_session(
        &security,
        "connection-test",
        &grant,
        "session-1",
        &receipt.window_capability,
        false,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(lease.recording_output_dir.as_deref(), Some(output.as_str()));
    for (field, value) in [
        (
            "recording_output_dir",
            json!(directory.join("elsewhere").to_str().unwrap()),
        ),
        ("allow_recording", json!(false)),
        ("observation_mode", json!("semantic")),
        ("process_id", json!(43)),
        ("window_handle", json!(78)),
        ("task_authorization_id", json!("unknown-authorization")),
    ] {
        let mut changed = grant_value.clone();
        changed[field] = value;
        let changed: TaskGrant = serde_json::from_value(changed).unwrap();
        assert!(
            crate::task_authorization_scope::preauthorize_task_session(
                &security,
                "connection-test",
                &changed,
                "session-1",
                &receipt.window_capability,
                false
            )
            .await
            .is_err(),
            "{field}"
        );
    }
    let channel = ObservationOnlyTestChannel::default();
    let driver = ComputerUseDriver::from_test_remote_channel(Arc::new(channel.clone())).unwrap();
    let mut host = cached_host_session(&driver);
    host.observation_mode = TaskObservationMode::PixelsOnly;
    host.capability = receipt.window_capability.clone();
    host.allow_recording = true;
    host.allow_live_observation = true;
    host.task_authorization_host = Some(authority);
    host.task_authorization = Some(lease);
    let mut sessions = ConnectionSessions {
        connection_id: "connection-test".into(),
        ..ConnectionSessions::default()
    };
    sessions.windows.insert("session-1".into(), host);
    let valid = json!({"method":"recording_start","params":{"session_id":"session-1","task_grant_id":"grant-1",
        "window_capability":receipt.window_capability,"request":{"record_video":true,"output_dir":output}}});
    let request = serde_json::from_value(valid.clone()).unwrap();
    // Grant enforcement only: a valid native start is deliberately not executed.
    crate::task_authorization_scope::enforce_task_authorized_method(&mut sessions, &request)
        .unwrap();
    for changed in [
        json!({"output_dir":directory.join("outside").to_str().unwrap(),"record_video":true}),
        json!({"output_dir":output,"record_video":false}),
    ] {
        let mut invalid = valid.clone();
        invalid["params"]["request"] = changed;
        let request = serde_json::from_value(invalid).unwrap();
        assert!(
            handle_request(
                &driver,
                &mut sessions,
                &mut Some(SnapshotTransport::BinaryFrame),
                &mut None,
                &CancellationRegistry::default(),
                request
            )
            .await
            .is_err()
        );
    }
    sessions
        .windows
        .get_mut("session-1")
        .unwrap()
        .allow_recording = false;
    let request = serde_json::from_value(valid.clone()).unwrap();
    assert!(
        crate::task_authorization_scope::enforce_task_authorized_method(&mut sessions, &request)
            .is_err()
    );
    sessions
        .windows
        .get_mut("session-1")
        .unwrap()
        .allow_recording = true;
    issuer.revoke(&receipt.authorization_id).unwrap();
    let request = serde_json::from_value(valid).unwrap();
    assert!(
        handle_request(
            &driver,
            &mut sessions,
            &mut Some(SnapshotTransport::BinaryFrame),
            &mut None,
            &CancellationRegistry::default(),
            request
        )
        .await
        .is_err()
    );
    assert_eq!(
        channel.exchanges.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    std::fs::remove_dir(directory).unwrap();
}

#[rstest]
fn task_action_scope_rejects_method_names_and_incoherent_special_actions() {
    let scope = |action: &str, input_kind: &str, secret_input: bool| TrustedTaskActionScope {
        action: action.into(),
        input_kind: input_kind.into(),
        secret_input,
        authorization_category: "credential".into(),
        browser_origin: None,
    };

    assert!(!scope("browser_click", "semantic", false).validate());
    assert!(!scope("browser_type", "semantic", true).validate());
    assert!(!scope("clipboard_capture_secret", "semantic", true).validate());
    assert!(scope("type", "semantic", true).validate());
}

fn task_binding<'a>() -> TaskAuthorizationBinding<'a> {
    TaskAuthorizationBinding::window(
        "connection-test",
        "authorization-1",
        "session-1",
        "grant-1",
        "CWS upload",
        "capability-1",
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 7,
        },
    )
}

fn type_chars_confirmation(secret_handle: Option<&str>) -> TrustedActionConfirmationRequest {
    type_chars_confirmation_for_target(secret_handle, "raw_input", 42, 7)
}

fn type_chars_confirmation_for_target(
    secret_handle: Option<&str>,
    authorization_category: &str,
    process_id: u32,
    window_handle: u64,
) -> TrustedActionConfirmationRequest {
    let action = HostAction {
        action: "type_chars".into(),
        element_index: None,
        element_token: None,
        delivery_mode: Some("foreground".into()),
        input_backend_id: None,
        input_kind: "raw_input".into(),
        intent: "ordinary_edit".into(),
        x: None,
        y: None,
        button: None,
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: Vec::new(),
        text: secret_handle.is_none().then(|| "bounded path".into()),
        secret_handle: secret_handle.map(str::to_owned),
        delay_ms: None,
        type_chars_only: true,
        checked: None,
        keys: Vec::new(),
        modifiers: Vec::new(),
        duration_ms: None,
        steps: None,
    };
    let mut action_value = serde_json::to_value(&action).unwrap();
    action_value["authorization_category"] = json!(authorization_category);
    TrustedActionConfirmationRequest::for_bound_window_action_value(
        ConfirmationBinding::window(
            "session-1",
            "grant-1",
            "capability-1",
            ConfirmationWindowIdentity {
                process_id,
                window_handle,
            },
            "observation-1",
            Some("accessibility-1"),
        ),
        &action.intent,
        action_value,
    )
    .unwrap()
}

fn browser_secret_confirmation(
    origin: &str,
    window_capability: &str,
) -> TrustedActionConfirmationRequest {
    TrustedActionConfirmationRequest::for_bound_window_action_value(
        ConfirmationBinding::window(
            "session-1",
            "grant-1",
            window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
            "snapshot-1",
            Some("tab-1"),
        ),
        "credential_input",
        json!({
            "method": "browser_type",
            "browser_origin": origin,
            "request": {
                "target_id": "target-1",
                "tab_id": "tab-1",
                "snapshot_id": "snapshot-1",
                "ref": "field-1",
                "secret_handle": "chrome-web-store.refresh-token"
            }
        }),
    )
    .unwrap()
}

fn browser_credential_registration(
    expires_at_unix_ms: u64,
) -> TrustedTaskAuthorizationRegistration {
    TrustedTaskAuthorizationRegistration {
        connection_id: None,
        task_id: None,
        task_grant_id: "grant-1".into(),
        application_label: "Chrome Web Store upload".into(),
        target: TrustedTaskAuthorizationTarget::ExactWindow {
            process_id: 42,
            window_handle: 7,
        },
        allowed_host_methods: vec!["browser_snapshot".into(), "browser_type".into()],
        allowed_actions: vec![TrustedTaskActionScope {
            action: "browser_type".into(),
            input_kind: "browser".into(),
            secret_input: true,
            authorization_category: "credential".into(),
            browser_origin: Some("https://chromewebstore.google.com".into()),
        }],
        allowed_browser_origins: vec!["https://chromewebstore.google.com".into()],
        browser_scope: None,
        recording_output_dir: None,
        capture_preparation: None,
        expires_at_unix_ms,
    }
}

#[rstest]
#[tokio::test]
async fn broker_turns_one_trusted_embedding_registration_into_an_exact_no_popup_lease() {
    let (issuer, host) = trusted_task_authorization_broker();
    let receipt = issuer
        .register(browser_credential_registration(unix_time_millis() + 60_000))
        .unwrap();
    assert!(receipt.window_capability.starts_with("cua-window-"));
    let binding = TaskAuthorizationBinding::window(
        "connection-test",
        &receipt.authorization_id,
        "session-1",
        "grant-1",
        "Chrome Web Store upload",
        &receipt.window_capability,
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 7,
        },
    );

    let lease = issue_task_authorization(Some(host.as_ref()), binding)
        .await
        .unwrap();
    let outcome = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &browser_secret_confirmation(
            "https://chromewebstore.google.com",
            &receipt.window_capability,
        ),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::Allowed);
    assert_eq!(lease.allowed_actions.len(), 1);
}

#[rstest]
#[tokio::test]
async fn native_minimize_lease_grants_only_its_exact_window_operation() {
    let (issuer, authority) = trusted_task_authorization_broker();
    let mut registration = browser_credential_registration(unix_time_millis() + 60_000);
    registration.allowed_host_methods = vec!["snapshot".into(), "minimize_window".into()];
    registration.allowed_actions = vec![
        serde_json::from_value(json!({
            "action":"minimize_window", "input_kind":"window_state", "secret_input":false,
            "authorization_category":"window_state"
        }))
        .unwrap(),
    ];
    registration.allowed_browser_origins.clear();
    let receipt = issuer.register(registration).unwrap();
    let binding = TaskAuthorizationBinding::window(
        "connection-test",
        &receipt.authorization_id,
        "session-1",
        "grant-1",
        "Chrome Web Store upload",
        &receipt.window_capability,
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 7,
        },
    );
    let lease = issue_task_authorization(Some(authority.as_ref()), binding)
        .await
        .unwrap();
    assert_eq!(lease.allowed_actions.len(), 1);
    assert!(lease.allowed_actions[0].is_window_minimize());
    let driver = ComputerUseDriver::create().unwrap();
    let mut host = cached_host_session(&driver);
    assert!(host.require_minimize_grant("session-1").is_err());
    host.target_window_handle = 7;
    host.capability = receipt.window_capability.clone();
    host.task_authorization = Some(lease);
    host.require_minimize_grant("session-1").unwrap();
    assert!(host.require_minimize_grant("other-session").is_err());
    host.target_window_handle = 77;
    assert!(host.require_minimize_grant("session-1").is_err());
    host.target_window_handle = 7;
    host.require_task_authorized_method("minimize_window")
        .unwrap();
    assert!(
        host.require_task_authorized_method("execute_action")
            .is_err()
    );
    host.task_authorization.as_mut().unwrap().allowed_actions[0].action = "click".into();
    assert!(host.require_minimize_grant("session-1").is_err());
}

#[rstest]
#[tokio::test]
async fn broker_registration_is_single_use_and_cannot_be_replayed_into_a_second_session() {
    let (issuer, host) = trusted_task_authorization_broker();
    let receipt = issuer
        .register(browser_credential_registration(unix_time_millis() + 60_000))
        .unwrap();
    let first = TaskAuthorizationBinding::window(
        "connection-test",
        &receipt.authorization_id,
        "session-1",
        "grant-1",
        "Chrome Web Store upload",
        &receipt.window_capability,
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 7,
        },
    );
    issue_task_authorization(Some(host.as_ref()), first)
        .await
        .unwrap();
    let replay = TaskAuthorizationBinding::window(
        "connection-test",
        &receipt.authorization_id,
        "session-2",
        "grant-1",
        "Chrome Web Store upload",
        &receipt.window_capability,
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 7,
        },
    );

    let error = issue_task_authorization(Some(host.as_ref()), replay)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        HostError::CodedProtocol {
            code: HostProtocolErrorCode::TaskAuthorizationDenied,
            ..
        }
    ));
}

#[rstest]
#[tokio::test]
async fn broker_binds_a_task_owned_browser_to_the_host_derived_target_once() {
    let (issuer, host) = trusted_task_authorization_broker();
    let receipt = issuer
        .register(TrustedTaskAuthorizationRegistration {
            connection_id: None,
            task_id: None,
            task_grant_id: "grant-owned".into(),
            application_label: "Firefox add-on upload".into(),
            target: TrustedTaskAuthorizationTarget::OwnedBrowser(
                dcc_cua_core::ComputerUseOwnedBrowserLaunchSpec {
                    browser: dcc_cua_core::ComputerUseOwnedBrowserFamily::Chromium,
                    profile: dcc_cua_core::ComputerUseOwnedBrowserProfile::IsolatedNew,
                },
            ),
            allowed_host_methods: vec!["browser_snapshot".into(), "browser_type".into()],
            allowed_actions: vec![TrustedTaskActionScope {
                action: "browser_type".into(),
                input_kind: "browser".into(),
                secret_input: true,
                authorization_category: "credential".into(),
                browser_origin: Some("https://addons.mozilla.org".into()),
            }],
            allowed_browser_origins: vec!["https://addons.mozilla.org".into()],
            browser_scope: None,
            recording_output_dir: None,
            capture_preparation: None,
            expires_at_unix_ms: unix_time_millis() + 60_000,
        })
        .unwrap();
    let binding = TaskAuthorizationBinding::window(
        "connection-test",
        &receipt.authorization_id,
        "session-owned",
        "grant-owned",
        "Firefox add-on upload",
        &receipt.window_capability,
        ConfirmationWindowIdentity {
            process_id: 9081,
            window_handle: 771,
        },
    );

    let lease = issue_task_authorization(Some(host.as_ref()), binding)
        .await
        .unwrap();

    assert_eq!(lease.target_process_id, 9081);
    assert_eq!(lease.target_window_handle, 771);
}

#[rstest]
#[tokio::test]
async fn broker_rejects_a_different_exact_target_before_opening_the_session() {
    let (issuer, host) = trusted_task_authorization_broker();
    let receipt = issuer
        .register(browser_credential_registration(unix_time_millis() + 60_000))
        .unwrap();
    let changed_target = TaskAuthorizationBinding::window(
        "connection-test",
        &receipt.authorization_id,
        "session-1",
        "grant-1",
        "Chrome Web Store upload",
        &receipt.window_capability,
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 8,
        },
    );

    let error = issue_task_authorization(Some(host.as_ref()), changed_target)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        HostError::CodedProtocol {
            code: HostProtocolErrorCode::TaskAuthorizationDenied,
            ..
        }
    ));
}

#[rstest]
#[tokio::test]
async fn broker_revocation_stops_an_active_task_without_falling_back_to_a_popup() {
    let (issuer, host) = trusted_task_authorization_broker();
    let receipt = issuer
        .register(browser_credential_registration(unix_time_millis() + 60_000))
        .unwrap();
    let binding = TaskAuthorizationBinding::window(
        "connection-test",
        &receipt.authorization_id,
        "session-1",
        "grant-1",
        "Chrome Web Store upload",
        &receipt.window_capability,
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 7,
        },
    );
    let lease = issue_task_authorization(Some(host.as_ref()), binding)
        .await
        .unwrap();
    issuer.revoke(&receipt.authorization_id).unwrap();

    let outcome = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &browser_secret_confirmation(
            "https://chromewebstore.google.com",
            &receipt.window_capability,
        ),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::Revoked);
}

#[rstest]
fn broker_rejects_expired_or_ambiguous_scope_before_issuing_an_authorization_id() {
    let (issuer, _host) = trusted_task_authorization_broker();
    let expired = issuer.register(browser_credential_registration(
        unix_time_millis().saturating_sub(1),
    ));
    assert!(matches!(
        expired,
        Err(TrustedTaskAuthorizationBrokerError::InvalidRegistration { .. })
    ));

    let mut duplicate = browser_credential_registration(unix_time_millis() + 60_000);
    duplicate
        .allowed_actions
        .push(duplicate.allowed_actions[0].clone());
    assert!(matches!(
        issuer.register(duplicate),
        Err(TrustedTaskAuthorizationBrokerError::InvalidRegistration { .. })
    ));
}

#[rstest]
#[tokio::test]
async fn active_task_authorization_allows_the_exact_scoped_action_without_a_popup() {
    let host: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: false });
    let lease = issue_task_authorization(Some(host.as_ref()), task_binding())
        .await
        .unwrap();

    let outcome = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &type_chars_confirmation(None),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::Allowed);
}

#[rstest]
#[tokio::test]
async fn explicit_task_start_denial_is_typed_and_never_opens_a_session() {
    let host: Arc<dyn TrustedTaskAuthorizationHost> = Arc::new(DenyingTaskAuthorizationHost);
    let error = issue_task_authorization(Some(host.as_ref()), task_binding())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        HostError::CodedProtocol {
            code: HostProtocolErrorCode::TaskAuthorizationDenied,
            ..
        }
    ));
}

#[rstest]
#[tokio::test]
async fn task_authorization_does_not_widen_from_plaintext_to_secret_input() {
    let host: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: false });
    let lease = issue_task_authorization(Some(host.as_ref()), task_binding())
        .await
        .unwrap();

    let outcome = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &type_chars_confirmation(Some("secret-handle-1")),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::OutOfScope);
}

#[rstest]
#[tokio::test]
async fn revoked_task_authorization_fails_closed_without_falling_back_to_a_popup() {
    let issuing: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: false });
    let lease = issue_task_authorization(Some(issuing.as_ref()), task_binding())
        .await
        .unwrap();
    let revoked: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: true });

    let outcome = authorize_task_scoped_action(
        Some(revoked.as_ref()),
        Some(&lease),
        &type_chars_confirmation(None),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::Revoked);
}

#[rstest]
#[tokio::test]
async fn expired_task_authorization_fails_before_constructor_host_validation() {
    let host: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: false });
    let mut lease = issue_task_authorization(Some(host.as_ref()), task_binding())
        .await
        .unwrap();
    lease.expires_at_unix_ms = unix_time_millis().saturating_sub(1);

    let outcome = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &type_chars_confirmation(None),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::Expired);
}

#[rstest]
#[tokio::test]
async fn target_change_requires_a_new_task_authorization() {
    let host: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: false });
    let lease = issue_task_authorization(Some(host.as_ref()), task_binding())
        .await
        .unwrap();

    let outcome = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &type_chars_confirmation_for_target(None, "raw_input", 42, 8),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::OutOfScope);
}

#[rstest]
#[tokio::test]
async fn risk_category_cannot_widen_from_raw_input_to_payment() {
    let host: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: false });
    let lease = issue_task_authorization(Some(host.as_ref()), task_binding())
        .await
        .unwrap();

    let outcome = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &type_chars_confirmation_for_target(None, "payment", 42, 7),
    )
    .await;

    assert_eq!(outcome, TaskAuthorizationOutcome::OutOfScope);
}

#[rstest]
#[tokio::test]
async fn browser_credential_scope_requires_the_exact_observed_origin() {
    let host: Arc<dyn TrustedTaskAuthorizationHost> =
        Arc::new(TaskAuthorizationHost { revoked: false });
    let mut lease = issue_task_authorization(Some(host.as_ref()), task_binding())
        .await
        .unwrap();
    lease.allowed_actions = vec![TrustedTaskActionScope {
        action: "browser_type".into(),
        input_kind: "browser".into(),
        secret_input: true,
        authorization_category: "credential".into(),
        browser_origin: Some("https://chromewebstore.google.com".into()),
    }];

    let allowed = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &browser_secret_confirmation("https://chromewebstore.google.com", "capability-1"),
    )
    .await;
    let refused = authorize_task_scoped_action(
        Some(host.as_ref()),
        Some(&lease),
        &browser_secret_confirmation("https://payments.google.com", "capability-1"),
    )
    .await;

    assert_eq!(allowed, TaskAuthorizationOutcome::Allowed);
    assert_eq!(refused, TaskAuthorizationOutcome::OutOfScope);
}

#[rstest]
fn a_bare_boolean_cannot_enable_task_authorization() {
    let grant = serde_json::from_value::<TaskGrant>(json!({
        "task_grant_id": "grant-1",
        "application_label": "CWS upload",
        "task_authorization": true
    }));
    assert!(grant.is_err());
}

#[rstest]
fn task_authorization_failures_are_machine_readable_and_non_modal() {
    for (outcome, expected) in [
        (
            ActionConfirmationOutcome::TaskAuthorizationRequired,
            "task_authorization_required",
        ),
        (
            ActionConfirmationOutcome::TaskAuthorizationOutOfScope,
            "task_authorization_out_of_scope",
        ),
        (
            ActionConfirmationOutcome::TaskAuthorizationExpired,
            "task_authorization_expired",
        ),
        (
            ActionConfirmationOutcome::TaskAuthorizationRevoked,
            "task_authorization_revoked",
        ),
    ] {
        let response = crate::request_contract::action_confirmation_refusal(outcome).0;
        assert_eq!(response["error"], expected);
        assert_eq!(response["success"], false);
    }
}

fn capture_preparation_scope() -> TrustedTaskActionScope {
    TrustedTaskActionScope {
        action: "capture_preparation_begin".into(),
        input_kind: "window_state".into(),
        secret_input: false,
        authorization_category: "window_state".into(),
        browser_origin: None,
    }
}

fn capture_preparation_registration(directory: &str) -> TrustedTaskAuthorizationRegistration {
    let mut registration = browser_credential_registration(unix_time_millis() + 60_000);
    registration.application_label = "Test DCC".into();
    registration.target = TrustedTaskAuthorizationTarget::ExactWindow {
        process_id: 42,
        window_handle: 77,
    };
    registration.allowed_host_methods = [
        "get_window_state",
        "capture_preparation_begin",
        "capture_preparation_state",
        "capture_preparation_stop",
        "capture_preparation_snapshot",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    registration.allowed_actions = vec![capture_preparation_scope()];
    registration.allowed_browser_origins.clear();
    registration.capture_preparation = Some(
        dcc_cua_protocol::capture_preparation::CapturePreparationAuthorization {
            journal_directory: directory.into(),
            max_lifetime_ms: 30_000,
        },
    );
    registration
}

#[rstest]
fn capture_preparation_registration_requires_closed_scope_complete_lifecycle_and_ordinary_journal()
{
    let root = std::env::temp_dir().join(format!("capture-preparation-auth-{}", Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    #[cfg(not(windows))]
    let root = root.canonicalize().unwrap();
    let directory = root.to_str().unwrap();
    let registration = capture_preparation_registration(directory);
    registration.validate().unwrap();
    for required in [
        "get_window_state",
        "capture_preparation_begin",
        "capture_preparation_state",
        "capture_preparation_stop",
    ] {
        let mut invalid = registration.clone();
        invalid
            .allowed_host_methods
            .retain(|method| method != required);
        assert!(invalid.validate().is_err(), "{required}");
    }
    let mut invalid = registration.clone();
    invalid.capture_preparation = None;
    assert!(invalid.validate().is_err());
    invalid = registration.clone();
    invalid.allowed_actions.clear();
    assert!(invalid.validate().is_err());
    for (field, value) in [
        ("action", "set_window_frame"),
        ("input_kind", "raw_input"),
        ("authorization_category", "raw_input"),
    ] {
        let mut action = serde_json::to_value(capture_preparation_scope()).unwrap();
        action[field] = json!(value);
        invalid = registration.clone();
        invalid.allowed_actions = vec![serde_json::from_value(action).unwrap()];
        assert!(invalid.validate().is_err(), "{field}");
    }
    invalid = registration.clone();
    invalid.allowed_actions[0].secret_input = true;
    assert!(invalid.validate().is_err());
    for lifetime in [0, 30_001, u64::MAX] {
        invalid = registration.clone();
        invalid
            .capture_preparation
            .as_mut()
            .unwrap()
            .max_lifetime_ms = lifetime;
        assert!(invalid.validate().is_err());
    }
    for journal in [
        "relative".to_owned(),
        root.join("missing").to_string_lossy().into_owned(),
    ] {
        invalid = registration.clone();
        invalid
            .capture_preparation
            .as_mut()
            .unwrap()
            .journal_directory = journal;
        assert!(invalid.validate().is_err());
    }
    invalid = registration.clone();
    invalid.recording_output_dir = Some(directory.into());
    assert!(invalid.validate().is_err());
    std::fs::remove_dir(root).unwrap();
}

#[rstest]
#[tokio::test]
async fn capture_preparation_exact_lease_refuses_missing_changed_or_partial_permission_without_native_calls()
 {
    let root = std::env::temp_dir().join(format!("capture-preparation-lease-{}", Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    #[cfg(not(windows))]
    let root = root.canonicalize().unwrap();
    let (issuer, authority) = trusted_task_authorization_broker();
    let receipt = issuer
        .register(capture_preparation_registration(root.to_str().unwrap()))
        .unwrap();
    let lease = issue_task_authorization(
        Some(authority.as_ref()),
        TaskAuthorizationBinding::window(
            "connection-test",
            &receipt.authorization_id,
            "session-1",
            "grant-1",
            "Test DCC",
            &receipt.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 77,
            },
        ),
    )
    .await
    .unwrap();
    let channel = ObservationOnlyTestChannel::default();
    let driver = ComputerUseDriver::from_test_remote_channel(Arc::new(channel.clone())).unwrap();
    let mut host = cached_host_session(&driver);
    host.observation_mode = TaskObservationMode::PixelsOnly;
    assert!(
        host.require_capture_preparation_grant("session-1", "capture_preparation_begin")
            .is_err()
    );
    host.capability = receipt.window_capability;
    host.task_authorization = Some(lease.clone());
    host.require_capture_preparation_grant("session-1", "capture_preparation_begin")
        .unwrap();
    let mut grant: TaskGrant = serde_json::from_value(json!({
        "task_grant_id":"grant-1", "application_label":"Test DCC", "observation_mode":"pixels_only",
        "process_id":42,"window_handle":77,"task_authorization_id":lease.authorization_id,
        "task_authorization_window_capability":lease.window_capability,
    }))
    .unwrap();
    assert!(
        crate::task_authorization_scope::validate_grant_against_task_authorization(&grant, &lease)
            .is_err()
    );
    grant.allow_capture_preparation = true;
    crate::task_authorization_scope::validate_grant_against_task_authorization(&grant, &lease)
        .unwrap();
    for change in 0..8 {
        let mut invalid = lease.clone();
        match change {
            0 => invalid.session_id = "another-session".into(),
            1 => invalid.task_grant_id = "another-grant".into(),
            2 => invalid.window_capability = "another-capability".into(),
            3 => invalid.target_process_id = 99,
            4 => invalid.target_window_handle = 99,
            5 => invalid.allowed_actions.clear(),
            6 => invalid
                .allowed_host_methods
                .retain(|method| method != "capture_preparation_stop"),
            _ => invalid.capture_preparation = None,
        }
        host.task_authorization = Some(invalid);
        assert!(
            host.require_capture_preparation_grant("session-1", "capture_preparation_begin")
                .is_err(),
            "{change}"
        );
    }
    host.task_authorization = Some(lease);
    host.observation_mode = TaskObservationMode::Semantic;
    assert!(
        host.require_capture_preparation_grant("session-1", "capture_preparation_begin")
            .is_err()
    );
    assert_eq!(
        channel.exchanges.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    std::fs::remove_dir(root).unwrap();
}

#[rstest]
fn capture_preparation_old_grants_and_leases_default_to_no_permission() {
    let grant: TaskGrant =
        serde_json::from_value(json!({"task_grant_id":"old", "application_label":"Old task"}))
            .unwrap();
    assert!(!grant.allow_capture_preparation);
    grant.validate_identity().unwrap();
    let mut invalid = json!({"task_grant_id":"old", "application_label":"Old task", "allow_capture_preparation":true});
    assert!(
        serde_json::from_value::<TaskGrant>(invalid.clone())
            .unwrap()
            .validate_identity()
            .is_err()
    );
    invalid["capture_preparation"] =
        json!({"journal_directory":"caller", "max_lifetime_ms":30_000});
    assert!(serde_json::from_value::<TaskGrant>(invalid).is_err());
    assert_eq!(
        host_capabilities(false).contains(&"exact_window_capture_preparation_v1"),
        cfg!(windows)
    );
}

#[rstest]
#[tokio::test]
async fn capture_preparation_legacy_lease_serde_defaults_without_widening_authority() {
    let (_issuer, _authority, lease) = observation_only_lease().await;
    let old_wire = serde_json::to_value(lease).unwrap();
    assert!(old_wire.get("capture_preparation").is_none());
    let old_lease: TrustedTaskAuthorizationLease = serde_json::from_value(old_wire).unwrap();
    assert!(old_lease.capture_preparation.is_none());
    assert!(
        !old_lease
            .allowed_actions
            .iter()
            .any(TrustedTaskActionScope::is_capture_preparation)
    );
}

#[rstest]
fn capture_preparation_wire_rejects_journal_target_and_input_tokens() {
    for method in [
        "capture_preparation_begin",
        "capture_preparation_state",
        "capture_preparation_stop",
        "capture_preparation_snapshot",
    ] {
        let mut params = json!({"session_id":"s", "task_grant_id":"g", "window_capability":"c"});
        if method == "capture_preparation_begin" {
            params["request"] = json!({"window_state_id":"fresh", "lifetime_ms":1_000});
        }
        let request =
            serde_json::from_value::<Request>(json!({"method":method,"params":params})).unwrap();
        assert_eq!(request.window_method_scope().unwrap().3, method);
        assert!(request_handler::window_evidence_epoch_route(&request).is_some());
        for field in [
            "journal_directory",
            "target",
            "observation_id",
            "window_state_id",
        ] {
            let mut invalid = params.clone();
            invalid[field] = json!("caller");
            assert!(
                serde_json::from_value::<Request>(json!({"method":method,"params":invalid}))
                    .is_err(),
                "{method}/{field}"
            );
        }
        if method == "capture_preparation_begin" {
            params["request"]["journal_directory"] = json!("caller");
            assert!(
                serde_json::from_value::<Request>(json!({"method":method,"params":params}))
                    .is_err()
            );
        }
    }
}

#[rstest]
fn capture_preparation_passive_snapshot_has_image_and_metadata_without_action_tokens() {
    let metadata = json!({"passive":true,"input_authorized":false,"preparation_id":[1],"process_id":42,"window_handle":77});
    let transport = || {
        prepare_image_transport(
            vec![ComputerUseImage {
                data: vec![1, 2, 3],
                mime_type: "image/png".into(),
            }],
            SnapshotTransport::BinaryFrame,
            &mut None,
        )
        .unwrap()
    };
    let (response, bytes) = request_handler::passive_preparation_snapshot_response(
        "session-1",
        metadata.clone(),
        transport(),
    )
    .unwrap();
    assert_eq!(bytes, Some(vec![1, 2, 3]));
    assert_eq!(response["passive"], true);
    assert_eq!(response["input_authorized"], false);
    for field in [
        "observation_id",
        "accessibility_state_id",
        "element_token",
        "window_state_id",
    ] {
        assert!(response.get(field).is_none());
        let mut invalid = metadata.clone();
        invalid[field] = json!("must-not-become-actionable");
        assert!(
            request_handler::passive_preparation_snapshot_response("s", invalid, transport())
                .is_err()
        );
    }
}

#[rstest]
#[tokio::test]
async fn capture_preparation_actual_host_routes_refuse_legacy_sessions_before_native_dispatch() {
    let channel = ObservationOnlyTestChannel::default();
    let driver = ComputerUseDriver::from_test_remote_channel(Arc::new(channel.clone())).unwrap();
    for method in [
        "capture_preparation_begin",
        "capture_preparation_state",
        "capture_preparation_stop",
        "capture_preparation_snapshot",
    ] {
        let mut sessions = ConnectionSessions::default();
        let mut host = cached_host_session(&driver);
        host.observation_mode = TaskObservationMode::PixelsOnly;
        sessions.windows.insert("session-1".into(), host);
        let mut params = json!({"session_id":"session-1", "task_grant_id":"grant-1", "window_capability":"capability-1"});
        if method == "capture_preparation_begin" {
            params["request"] = json!({"window_state_id":"fresh", "lifetime_ms":1_000});
        }
        let request: Request =
            serde_json::from_value(json!({"method":method,"params":params})).unwrap();
        let result = handle_request(
            &driver,
            &mut sessions,
            &mut Some(SnapshotTransport::BinaryFrame),
            &mut None,
            &Arc::new(Mutex::new(HashMap::new())),
            request,
        )
        .await;
        let error = result.unwrap_err();
        assert_eq!(
            error_code(&error),
            "task_authorization_denied",
            "{method}/{error}"
        );
        if method == "capture_preparation_begin" {
            assert!(
                sessions.windows["session-1"]
                    .latest_observation_id
                    .is_none()
            );
            assert!(
                sessions.windows["session-1"]
                    .latest_accessibility_state_id
                    .is_none()
            );
        }
    }
    assert_eq!(
        channel.exchanges.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[rstest]
#[tokio::test]
async fn capture_preparation_no_active_native_handle_never_returns_fake_state_or_cleanup() {
    let root =
        std::env::temp_dir().join(format!("capture-preparation-no-handle-{}", Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    #[cfg(not(windows))]
    let root = root.canonicalize().unwrap();
    let (issuer, authority) = trusted_task_authorization_broker();
    let receipt = issuer
        .register(capture_preparation_registration(root.to_str().unwrap()))
        .unwrap();
    let lease = issue_task_authorization(
        Some(authority.as_ref()),
        TaskAuthorizationBinding::window(
            "connection-test",
            &receipt.authorization_id,
            "session-1",
            "grant-1",
            "Test DCC",
            &receipt.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 77,
            },
        ),
    )
    .await
    .unwrap();
    let channel = ObservationOnlyTestChannel::default();
    let driver = ComputerUseDriver::from_test_remote_channel(Arc::new(channel.clone())).unwrap();
    for method in ["capture_preparation_state", "capture_preparation_stop"] {
        let mut host = cached_host_session(&driver);
        host.observation_mode = TaskObservationMode::PixelsOnly;
        host.capability = lease.window_capability.clone();
        host.task_authorization = Some(lease.clone());
        host.task_authorization_host = Some(authority.clone());
        let mut sessions = ConnectionSessions::default();
        sessions.windows.insert("session-1".into(), host);
        let request: Request = serde_json::from_value(json!({"method":method,"params":{
            "session_id":"session-1","task_grant_id":"grant-1","window_capability":lease.window_capability}})).unwrap();
        assert!(
            handle_request(
                &driver,
                &mut sessions,
                &mut Some(SnapshotTransport::BinaryFrame),
                &mut None,
                &Arc::new(Mutex::new(HashMap::new())),
                request
            )
            .await
            .is_err(),
            "{method}"
        );
    }
    assert_eq!(
        channel.exchanges.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    std::fs::remove_dir(root).unwrap();
}
