use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};

use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey};
use rstest::rstest;

use super::*;
use crate::action_confirmation::{ConfirmationBinding, ConfirmationWindowIdentity};
use crate::task_authorization::{
    TaskAuthorizationBinding, TaskAuthorizationOutcome, authorize_task_scoped_action,
    issue_task_authorization,
};

#[derive(Clone)]
struct FixedClock(u64);

impl CrossClientTaskAuthorizationClock for FixedClock {
    fn unix_time_millis(&self) -> Result<u64, CrossClientTaskAuthorizationError> {
        Ok(self.0)
    }
}

struct MutableClock(AtomicU64);

impl CrossClientTaskAuthorizationClock for MutableClock {
    fn unix_time_millis(&self) -> Result<u64, CrossClientTaskAuthorizationError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

struct UnavailableTrust;

impl CrossClientTaskAuthorizationTrustStore for UnavailableTrust {
    fn lookup(
        &self,
        _key_id: &str,
    ) -> Result<
        Option<CrossClientTaskAuthorizationTrustState>,
        CrossClientTaskAuthorizationTrustError,
    > {
        Err(CrossClientTaskAuthorizationTrustError)
    }
}

struct FixedTargetObserver;

impl CrossClientTaskAuthorizationTargetObserver for FixedTargetObserver {
    fn process_creation_identity(
        &self,
        process_id: u32,
        window_handle: u64,
    ) -> Result<String, CrossClientTaskAuthorizationTargetError> {
        if process_id == 0 || window_handle == 0 {
            return Err(CrossClientTaskAuthorizationTargetError);
        }
        Ok("windows-filetime:123456".into())
    }
}

struct MutableTargetObserver(Mutex<String>);

impl CrossClientTaskAuthorizationTargetObserver for MutableTargetObserver {
    fn process_creation_identity(
        &self,
        _process_id: u32,
        _window_handle: u64,
    ) -> Result<String, CrossClientTaskAuthorizationTargetError> {
        self.0
            .lock()
            .map(|identity| identity.clone())
            .map_err(|_| CrossClientTaskAuthorizationTargetError)
    }
}

fn audience() -> CrossClientTaskAuthorizationAudience {
    CrossClientTaskAuthorizationAudience {
        client_id: "codex-desktop".into(),
        tenant_id: "tenant-1".into(),
        user_id: "user-1".into(),
    }
}

fn runtime() -> CrossClientTaskAuthorizationRuntime {
    CrossClientTaskAuthorizationRuntime {
        runtime_version: env!("CARGO_PKG_VERSION").into(),
        runtime_instance_id: "runtime-instance-1".into(),
        boot_generation: 7,
    }
}

fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ])
}

fn test_now() -> u64 {
    crate::task_authorization::unix_time_millis()
}

fn exact_window_request() -> CrossClientTaskAuthorizationChallengeRequest {
    CrossClientTaskAuthorizationChallengeRequest {
        audience: audience(),
        task_id: "task-1".into(),
        connection_id: "connection-1".into(),
        task_grant_id: "grant-1".into(),
        application_label: "Bound publishing task".into(),
        purpose: "Edit one already selected draft".into(),
        target: CrossClientTaskAuthorizationTarget::ExactWindow {
            process_id: 42,
            process_creation_identity: "windows-filetime:123456".into(),
            window_handle: 7,
        },
        allowed_host_methods: vec!["snapshot".into(), "execute_action".into()],
        allowed_actions: vec![TrustedTaskActionScope {
            action: "type_chars".into(),
            input_kind: "raw_input".into(),
            secret_input: false,
            authorization_category: "raw_input".into(),
            browser_origin: None,
        }],
        allowed_browser_origins: Vec::new(),
        secret_handle_policy: CrossClientTaskAuthorizationSecretPolicy::Forbidden,
        irreversible_operation: None,
        expires_at_unix_ms: test_now() + 60_000,
    }
}

fn bridge() -> (
    CrossClientTaskAuthorizationVerifier,
    TrustedTaskAuthorizationTrustAdmin,
    Arc<dyn TrustedTaskAuthorizationHost>,
    SigningKey,
) {
    let signer = signing_key();
    let now = test_now();
    let key = TrustedTaskAuthorizationKey {
        key_id: "issuer-key-1".into(),
        audience: audience(),
        verifying_key: signer.verifying_key().to_bytes(),
    };
    let (admin, trust) = trusted_task_authorization_trust_registry(vec![key]).unwrap();
    let (verifier, host) = cross_client_task_authorization_with_clock_and_target_observer(
        runtime(),
        trust,
        Arc::new(FixedClock(now)),
        Arc::new(FixedTargetObserver),
    )
    .unwrap();
    (verifier, admin, host, signer)
}

fn signed_receipt(
    challenge: &CrossClientTaskAuthorizationChallenge,
    signer: &SigningKey,
    decision: CrossClientTaskAuthorizationDecision,
) -> Vec<u8> {
    let unsigned = CrossClientTaskAuthorizationUnsignedDecision::for_challenge(
        challenge,
        "issuer-key-1",
        decision,
        challenge.issued_at_unix_ms,
        challenge.issued_at_unix_ms + 30_000,
    )
    .unwrap();
    let payload = cross_client_task_authorization_signing_payload(&unsigned).unwrap();
    let signature = signer.sign(&payload);
    canonical_cross_client_task_authorization_receipt(&CrossClientTaskAuthorizationSignedDecision {
        decision: unsigned,
        signature: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes()),
    })
    .unwrap()
}

fn bytes_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[rstest]
fn ed25519_uses_the_published_rfc_8032_test_vector() {
    let signer = signing_key();
    assert_eq!(
        bytes_hex(&signer.verifying_key().to_bytes()),
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
    );
    assert_eq!(
        bytes_hex(&signer.sign(b"").to_bytes()),
        concat!(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155",
            "5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
        )
    );
}

#[rstest]
fn protocol_jcs_and_signature_vector_is_stable() {
    let decision = CrossClientTaskAuthorizationUnsignedDecision {
        schema: CROSS_CLIENT_TASK_AUTHORIZATION_RECEIPT_SCHEMA.into(),
        issuer_key_id: "issuer-key-1".into(),
        audience: audience(),
        runtime_instance_id: "runtime-instance-1".into(),
        boot_generation: 7,
        challenge_id: "task-challenge-vector".into(),
        nonce: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
        challenge_sha256: format!("sha256:{}", "00".repeat(32)),
        decision: CrossClientTaskAuthorizationDecision::Allow,
        issued_at_unix_ms: 1_700_000_000_000,
        expires_at_unix_ms: 1_700_000_030_000,
    };
    let canonical = concat!(
        "{\"audience\":{\"client_id\":\"codex-desktop\",\"tenant_id\":\"tenant-1\",",
        "\"user_id\":\"user-1\"},\"boot_generation\":7,",
        "\"challenge_id\":\"task-challenge-vector\",",
        "\"challenge_sha256\":\"sha256:0000000000000000000000000000000000000000000000000000000000000000\",",
        "\"decision\":\"allow\",\"expires_at_unix_ms\":1700000030000,",
        "\"issued_at_unix_ms\":1700000000000,\"issuer_key_id\":\"issuer-key-1\",",
        "\"nonce\":\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\",",
        "\"runtime_instance_id\":\"runtime-instance-1\",",
        "\"schema\":\"dcc-cua.task-authorization-receipt.v2\"}"
    );
    let payload = cross_client_task_authorization_signing_payload(&decision).unwrap();
    let mut expected_payload = b"dcc-cua.task-authorization-receipt.v2\0".to_vec();
    expected_payload.extend_from_slice(canonical.as_bytes());
    assert_eq!(payload, expected_payload);
    assert_eq!(
        bytes_hex(&signing_key().sign(&payload).to_bytes()),
        concat!(
            "e0b7d7007e123362be82c9451a10504770f116584d69dcd9f289eef88dc1e90b",
            "fe63ff2847933bdd323bde0cc39b77bdaf5fcdd87bb5f6614f93b4ec6f363e09"
        )
    );
}

#[rstest]
#[tokio::test]
async fn signed_allow_is_consumed_once_and_issues_only_the_retained_exact_scope() {
    let (verifier, _admin, host, signer) = bridge();
    let challenge = verifier.prepare(exact_window_request()).unwrap();
    let receipt = signed_receipt(
        &challenge,
        &signer,
        CrossClientTaskAuthorizationDecision::Allow,
    );

    let opaque = match verifier.consume(&receipt).unwrap() {
        CrossClientTaskAuthorizationConsumption::Task(receipt) => receipt,
        other => panic!("expected task authorization, got {other:?}"),
    };
    assert_eq!(
        verifier.consume(&receipt).unwrap_err(),
        CrossClientTaskAuthorizationError::Replay
    );

    let wrong_connection = issue_task_authorization(
        Some(host.as_ref()),
        TaskAuthorizationBinding::window(
            "connection-2",
            &opaque.authorization_id,
            "session-1",
            "grant-1",
            "Bound publishing task",
            &opaque.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
        ),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        wrong_connection,
        HostError::CodedProtocol {
            code: HostProtocolErrorCode::TaskAuthorizationDenied,
            ..
        }
    ));

    let wrong_task = issue_task_authorization(
        Some(host.as_ref()),
        TaskAuthorizationBinding::window(
            "connection-1",
            &opaque.authorization_id,
            "session-1",
            "grant-1",
            "Bound publishing task",
            &opaque.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
        ),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        wrong_task,
        HostError::CodedProtocol {
            code: HostProtocolErrorCode::TaskAuthorizationDenied,
            ..
        }
    ));

    let lease = issue_task_authorization(
        Some(host.as_ref()),
        TaskAuthorizationBinding::window(
            "connection-1",
            &opaque.authorization_id,
            "task-1",
            "grant-1",
            "Bound publishing task",
            &opaque.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(lease.allowed_host_methods, challenge.allowed_host_methods);
    assert_eq!(lease.allowed_actions, challenge.allowed_actions);

    let driver = ComputerUseDriver::create().unwrap();
    let mut session = cached_host_session(&driver);
    session.capability = opaque.window_capability.clone();
    session.task_authorization = Some(lease);
    session.task_authorization_host = Some(Arc::clone(&host));
    let mut sessions = ConnectionSessions::default();
    sessions.windows.insert("task-1".into(), session);
    let snapshot = serde_json::from_value::<Request>(json!({
        "method": "snapshot",
        "params": {
            "session_id": "task-1",
            "task_grant_id": "grant-1",
            "window_capability": opaque.window_capability
        }
    }))
    .unwrap();
    crate::task_authorization_scope::enforce_task_authorized_method(&mut sessions, &snapshot)
        .unwrap();
    let widened = serde_json::from_value::<Request>(json!({
        "method": "clipboard_read",
        "params": {
            "session_id": "task-1",
            "task_grant_id": "grant-1",
            "window_capability": opaque.window_capability
        }
    }))
    .unwrap();
    assert!(matches!(
        crate::task_authorization_scope::enforce_task_authorized_method(&mut sessions, &widened,),
        Err(HostError::CodedProtocol {
            code: HostProtocolErrorCode::TaskAuthorizationDenied,
            ..
        })
    ));
}

#[rstest]
fn forged_wrong_runtime_widened_and_noncanonical_receipts_never_consume_the_challenge() {
    let (verifier, _admin, _host, signer) = bridge();
    let challenge = verifier.prepare(exact_window_request()).unwrap();
    let valid = signed_receipt(
        &challenge,
        &signer,
        CrossClientTaskAuthorizationDecision::Allow,
    );

    let mut misleading_relay = challenge.clone();
    misleading_relay.purpose = "Display a different task than the retained digest".into();
    assert_eq!(
        CrossClientTaskAuthorizationUnsignedDecision::for_challenge(
            &misleading_relay,
            "issuer-key-1",
            CrossClientTaskAuthorizationDecision::Allow,
            misleading_relay.issued_at_unix_ms,
            misleading_relay.issued_at_unix_ms + 30_000,
        )
        .unwrap_err(),
        CrossClientTaskAuthorizationError::InvalidChallenge
    );

    let mut forged = valid.clone();
    *forged.last_mut().unwrap() ^= 1;
    assert!(verifier.consume(&forged).is_err());

    let mut wrong_runtime = CrossClientTaskAuthorizationUnsignedDecision::for_challenge(
        &challenge,
        "issuer-key-1",
        CrossClientTaskAuthorizationDecision::Allow,
        challenge.issued_at_unix_ms,
        challenge.issued_at_unix_ms + 30_000,
    )
    .unwrap();
    wrong_runtime.runtime_instance_id = "substituted-runtime".into();
    let payload = cross_client_task_authorization_signing_payload(&wrong_runtime).unwrap();
    let wrong_runtime = canonical_cross_client_task_authorization_receipt(
        &CrossClientTaskAuthorizationSignedDecision {
            decision: wrong_runtime,
            signature: base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(signer.sign(&payload).to_bytes()),
        },
    )
    .unwrap();
    assert_eq!(
        verifier.consume(&wrong_runtime).unwrap_err(),
        CrossClientTaskAuthorizationError::BindingMismatch
    );

    let pretty: Vec<u8> =
        serde_json::to_string_pretty(&serde_json::from_slice::<serde_json::Value>(&valid).unwrap())
            .unwrap()
            .into_bytes();
    assert_eq!(
        verifier.consume(&pretty).unwrap_err(),
        CrossClientTaskAuthorizationError::NoncanonicalReceipt
    );

    assert!(matches!(
        verifier.consume(&valid).unwrap(),
        CrossClientTaskAuthorizationConsumption::Task(_)
    ));
}

#[rstest]
fn duplicate_unknown_and_out_of_range_receipt_fields_are_rejected_before_trust() {
    let (verifier, _admin, _host, signer) = bridge();
    let challenge = verifier.prepare(exact_window_request()).unwrap();
    let valid = signed_receipt(
        &challenge,
        &signer,
        CrossClientTaskAuthorizationDecision::Allow,
    );
    assert_eq!(
        verifier.consume(&vec![b' '; 16 * 1_024 + 1]).unwrap_err(),
        CrossClientTaskAuthorizationError::MalformedReceipt
    );
    let text = String::from_utf8(valid.clone()).unwrap();
    let signature = serde_json::from_slice::<serde_json::Value>(&valid).unwrap()["signature"]
        .as_str()
        .unwrap()
        .to_owned();
    let duplicate = text.replacen(
        &format!("\"signature\":\"{signature}\"}}"),
        &format!("\"signature\":\"{signature}\",\"signature\":\"{signature}\"}}"),
        1,
    );
    assert_eq!(
        verifier.consume(duplicate.as_bytes()).unwrap_err(),
        CrossClientTaskAuthorizationError::MalformedReceipt
    );

    let mut unknown = serde_json::from_slice::<serde_json::Value>(&valid).unwrap();
    unknown["allowed_actions"] = json!(["execute_everything"]);
    let unknown = serde_jcs::to_vec(&unknown).unwrap();
    assert_eq!(
        verifier.consume(&unknown).unwrap_err(),
        CrossClientTaskAuthorizationError::MalformedReceipt
    );

    let unsafe_integer = text.replacen(
        "\"boot_generation\":7",
        "\"boot_generation\":9007199254740992",
        1,
    );
    assert_eq!(
        verifier.consume(unsafe_integer.as_bytes()).unwrap_err(),
        CrossClientTaskAuthorizationError::MalformedReceipt
    );
}

#[rstest]
fn unsafe_runtime_and_challenge_integers_use_their_own_error_contracts() {
    let signer = signing_key();
    let key = TrustedTaskAuthorizationKey {
        key_id: "issuer-key-1".into(),
        audience: audience(),
        verifying_key: signer.verifying_key().to_bytes(),
    };
    let (_admin, trust) = trusted_task_authorization_trust_registry(vec![key]).unwrap();
    let mut invalid_runtime = runtime();
    invalid_runtime.boot_generation = 9_007_199_254_740_992;
    assert!(matches!(
        cross_client_task_authorization_with_clock(
            invalid_runtime,
            trust,
            Arc::new(FixedClock(test_now())),
        ),
        Err(CrossClientTaskAuthorizationError::InvalidConfiguration)
    ));

    let (verifier, _admin, _host, _signer) = bridge();
    let mut invalid_challenge = exact_window_request();
    invalid_challenge.expires_at_unix_ms = 9_007_199_254_740_992;
    assert_eq!(
        verifier.prepare(invalid_challenge).unwrap_err(),
        CrossClientTaskAuthorizationError::InvalidChallenge
    );
    let mut invalid_window = exact_window_request();
    invalid_window.target = CrossClientTaskAuthorizationTarget::ExactWindow {
        process_id: 42,
        process_creation_identity: "windows-filetime:123456".into(),
        window_handle: 9_007_199_254_740_992,
    };
    assert_eq!(
        verifier.prepare(invalid_window).unwrap_err(),
        CrossClientTaskAuthorizationError::InvalidChallenge
    );
    let mut invalid_label = exact_window_request();
    invalid_label.application_label = "x".repeat(crate::MAX_APPLICATION_LABEL_CHARS + 1);
    assert_eq!(
        verifier.prepare(invalid_label).unwrap_err(),
        CrossClientTaskAuthorizationError::InvalidChallenge
    );
}

#[rstest]
fn unknown_revoked_and_unavailable_trust_all_fail_closed_without_consumption() {
    let signer = signing_key();
    let unknown_key = TrustedTaskAuthorizationKey {
        key_id: "different-key".into(),
        audience: audience(),
        verifying_key: signer.verifying_key().to_bytes(),
    };
    let (_admin, trust) = trusted_task_authorization_trust_registry(vec![unknown_key]).unwrap();
    let (unknown_verifier, _host) = cross_client_task_authorization_with_clock_and_target_observer(
        runtime(),
        trust,
        Arc::new(FixedClock(test_now())),
        Arc::new(FixedTargetObserver),
    )
    .unwrap();
    let challenge = unknown_verifier.prepare(exact_window_request()).unwrap();
    let receipt = signed_receipt(
        &challenge,
        &signer,
        CrossClientTaskAuthorizationDecision::Allow,
    );
    assert_eq!(
        unknown_verifier.consume(&receipt).unwrap_err(),
        CrossClientTaskAuthorizationError::UnknownKey
    );

    let (revoked_verifier, admin, _host, signer) = bridge();
    let challenge = revoked_verifier.prepare(exact_window_request()).unwrap();
    let receipt = signed_receipt(
        &challenge,
        &signer,
        CrossClientTaskAuthorizationDecision::Allow,
    );
    admin.revoke("issuer-key-1", 1).unwrap();
    assert_eq!(
        revoked_verifier.consume(&receipt).unwrap_err(),
        CrossClientTaskAuthorizationError::RevokedKey
    );

    let now = test_now();
    let (unavailable_verifier, _host) =
        cross_client_task_authorization_with_clock_and_target_observer(
            runtime(),
            Arc::new(UnavailableTrust),
            Arc::new(FixedClock(now)),
            Arc::new(FixedTargetObserver),
        )
        .unwrap();
    let challenge = unavailable_verifier
        .prepare(exact_window_request())
        .unwrap();
    let receipt = signed_receipt(
        &challenge,
        &signing_key(),
        CrossClientTaskAuthorizationDecision::Allow,
    );
    assert_eq!(
        unavailable_verifier.consume(&receipt).unwrap_err(),
        CrossClientTaskAuthorizationError::TrustUnavailable
    );
}

#[rstest]
fn concurrent_consumption_has_exactly_one_winner() {
    let (verifier, _admin, _host, signer) = bridge();
    let challenge = verifier.prepare(exact_window_request()).unwrap();
    let receipt = Arc::new(signed_receipt(
        &challenge,
        &signer,
        CrossClientTaskAuthorizationDecision::Allow,
    ));
    let verifier = Arc::new(verifier);
    let barrier = Arc::new(Barrier::new(3));
    let outcomes = std::thread::scope(|scope| {
        let handles = (0..2)
            .map(|_| {
                let verifier = Arc::clone(&verifier);
                let receipt = Arc::clone(&receipt);
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    verifier.consume(receipt.as_slice())
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(
                outcome,
                Ok(CrossClientTaskAuthorizationConsumption::Task(_))
            ))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == Err(CrossClientTaskAuthorizationError::Replay))
            .count(),
        1
    );
}

#[rstest]
fn clock_rollback_latches_a_fail_closed_state() {
    let signer = signing_key();
    let key = TrustedTaskAuthorizationKey {
        key_id: "issuer-key-1".into(),
        audience: audience(),
        verifying_key: signer.verifying_key().to_bytes(),
    };
    let (_admin, trust) = trusted_task_authorization_trust_registry(vec![key]).unwrap();
    let now = test_now();
    let clock = Arc::new(MutableClock(AtomicU64::new(now)));
    let (verifier, _host) = cross_client_task_authorization_with_clock_and_target_observer(
        runtime(),
        trust,
        Arc::clone(&clock) as Arc<dyn CrossClientTaskAuthorizationClock>,
        Arc::new(FixedTargetObserver),
    )
    .unwrap();
    let challenge = verifier.prepare(exact_window_request()).unwrap();
    let receipt = signed_receipt(
        &challenge,
        &signer,
        CrossClientTaskAuthorizationDecision::Allow,
    );
    clock.0.store(now - 1, Ordering::SeqCst);
    assert_eq!(
        verifier.consume(&receipt).unwrap_err(),
        CrossClientTaskAuthorizationError::ClockRollback
    );
    clock.0.store(now, Ordering::SeqCst);
    assert_eq!(
        verifier.consume(&receipt).unwrap_err(),
        CrossClientTaskAuthorizationError::ClockRollback
    );
}

#[rstest]
fn exact_process_identity_is_checked_before_and_after_human_confirmation() {
    let signer = signing_key();
    let key = TrustedTaskAuthorizationKey {
        key_id: "issuer-key-1".into(),
        audience: audience(),
        verifying_key: signer.verifying_key().to_bytes(),
    };
    let (_admin, trust) = trusted_task_authorization_trust_registry(vec![key]).unwrap();
    let observer = Arc::new(MutableTargetObserver(Mutex::new(
        "windows-filetime:654321".into(),
    )));
    let (verifier, _host) = cross_client_task_authorization_with_clock_and_target_observer(
        runtime(),
        trust,
        Arc::new(FixedClock(test_now())),
        Arc::clone(&observer) as Arc<dyn CrossClientTaskAuthorizationTargetObserver>,
    )
    .unwrap();

    assert_eq!(
        verifier.prepare(exact_window_request()).unwrap_err(),
        CrossClientTaskAuthorizationError::TargetUnavailable
    );
    *observer.0.lock().unwrap() = "windows-filetime:123456".into();
    let challenge = verifier.prepare(exact_window_request()).unwrap();
    let receipt = signed_receipt(
        &challenge,
        &signer,
        CrossClientTaskAuthorizationDecision::Allow,
    );
    *observer.0.lock().unwrap() = "windows-filetime:654321".into();
    assert_eq!(
        verifier.consume(&receipt).unwrap_err(),
        CrossClientTaskAuthorizationError::TargetUnavailable
    );
    *observer.0.lock().unwrap() = "windows-filetime:123456".into();
    assert!(matches!(
        verifier.consume(&receipt).unwrap(),
        CrossClientTaskAuthorizationConsumption::Task(_)
    ));
}

#[rstest]
fn expired_confirmation_windows_release_pending_capacity() {
    let signer = signing_key();
    let key = TrustedTaskAuthorizationKey {
        key_id: "issuer-key-1".into(),
        audience: audience(),
        verifying_key: signer.verifying_key().to_bytes(),
    };
    let (_admin, trust) = trusted_task_authorization_trust_registry(vec![key]).unwrap();
    let now = test_now();
    let clock = Arc::new(MutableClock(AtomicU64::new(now)));
    let (verifier, _host) = cross_client_task_authorization_with_clock_and_target_observer(
        runtime(),
        trust,
        Arc::clone(&clock) as Arc<dyn CrossClientTaskAuthorizationClock>,
        Arc::new(FixedTargetObserver),
    )
    .unwrap();
    let mut request = exact_window_request();
    request.expires_at_unix_ms = now + crate::task_authorization::MAX_TASK_AUTHORIZATION_TTL_MS;
    for _ in 0..256 {
        verifier.prepare(request.clone()).unwrap();
    }
    assert_eq!(
        verifier.prepare(request).unwrap_err(),
        CrossClientTaskAuthorizationError::Capacity
    );

    let after_confirmation_window = now + 5 * 60 * 1_000 + 1;
    clock.0.store(after_confirmation_window, Ordering::SeqCst);
    let mut fresh = exact_window_request();
    fresh.expires_at_unix_ms = after_confirmation_window + 60_000;
    verifier.prepare(fresh).unwrap();
}

#[rstest]
#[tokio::test]
async fn browser_bootstrap_and_final_exact_tab_scopes_cannot_be_substituted() {
    let (verifier, _admin, host, signer) = bridge();
    let mut invalid_bootstrap = exact_window_request();
    invalid_bootstrap.target = CrossClientTaskAuthorizationTarget::BrowserBootstrapOwned {
        browser: dcc_cua_core::ComputerUseOwnedBrowserFamily::Chromium,
        profile: dcc_cua_core::ComputerUseOwnedBrowserProfile::IsolatedNew,
    };
    invalid_bootstrap.allowed_host_methods = vec!["launch_owned_browser".into()];
    assert_eq!(
        verifier.prepare(invalid_bootstrap).unwrap_err(),
        CrossClientTaskAuthorizationError::InvalidChallenge
    );

    let mut browser = exact_window_request();
    browser.target = CrossClientTaskAuthorizationTarget::ExactBrowser {
        process_id: 42,
        process_creation_identity: "windows-filetime:123456".into(),
        window_handle: 7,
        host_target_id: "target-1".into(),
        tab_id: "tab-1".into(),
        document_generation: "document-1".into(),
        origin: "https://chromewebstore.google.com".into(),
    };
    browser.task_id = "session-browser".into();
    browser.allowed_actions[0].browser_origin = Some("https://payments.google.com".into());
    browser.allowed_browser_origins = vec!["https://chromewebstore.google.com".into()];
    assert_eq!(
        verifier.prepare(browser).unwrap_err(),
        CrossClientTaskAuthorizationError::InvalidChallenge
    );

    let origin = "https://chromewebstore.google.com";
    let mut browser = exact_window_request();
    browser.target = CrossClientTaskAuthorizationTarget::ExactBrowser {
        process_id: 42,
        process_creation_identity: "windows-filetime:123456".into(),
        window_handle: 7,
        host_target_id: "target-1".into(),
        tab_id: "tab-1".into(),
        document_generation: "snapshot-1".into(),
        origin: origin.into(),
    };
    browser.task_id = "session-browser".into();
    browser.allowed_host_methods = vec!["browser_snapshot".into(), "browser_type".into()];
    browser.allowed_actions = vec![TrustedTaskActionScope {
        action: "browser_type".into(),
        input_kind: "browser".into(),
        secret_input: true,
        authorization_category: "credential".into(),
        browser_origin: Some(origin.into()),
    }];
    browser.allowed_browser_origins = vec![origin.into()];
    browser.secret_handle_policy = CrossClientTaskAuthorizationSecretPolicy::NamedHandlesOnly;
    let challenge = verifier.prepare(browser).unwrap();
    let receipt = signed_receipt(
        &challenge,
        &signer,
        CrossClientTaskAuthorizationDecision::Allow,
    );
    let opaque = match verifier.consume(&receipt).unwrap() {
        CrossClientTaskAuthorizationConsumption::Task(receipt) => receipt,
        other => panic!("expected exact-browser task authorization, got {other:?}"),
    };
    let lease = issue_task_authorization(
        Some(host.as_ref()),
        TaskAuthorizationBinding::window(
            "connection-1",
            &opaque.authorization_id,
            "session-browser",
            "grant-1",
            "Bound publishing task",
            &opaque.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
        ),
    )
    .await
    .unwrap();
    let scope = lease.browser_scope.as_ref().unwrap();
    assert_eq!(scope.host_target_id, "target-1");
    assert_eq!(scope.tab_id, "tab-1");
    assert_eq!(scope.document_generation, "snapshot-1");

    let driver = ComputerUseDriver::create().unwrap();
    let mut session = cached_host_session(&driver);
    session.capability = opaque.window_capability.clone();
    session.browser = task_browser_session(&Some(lease.clone())).unwrap();
    session.task_authorization = Some(lease);
    let mut sessions = ConnectionSessions::default();
    sessions.windows.insert("session-browser".into(), session);
    let exact_snapshot = serde_json::from_value::<Request>(json!({
        "method": "browser_snapshot",
        "params": {
            "session_id": "session-browser",
            "task_grant_id": "grant-1",
            "window_capability": opaque.window_capability,
            "request": {"target_id": "target-1", "tab_id": "tab-1"}
        }
    }))
    .unwrap();
    crate::task_authorization_scope::enforce_task_authorized_method(&mut sessions, &exact_snapshot)
        .unwrap();
    let wrong_tab = serde_json::from_value::<Request>(json!({
        "method": "browser_snapshot",
        "params": {
            "session_id": "session-browser",
            "task_grant_id": "grant-1",
            "window_capability": opaque.window_capability,
            "request": {"target_id": "target-1", "tab_id": "tab-2"}
        }
    }))
    .unwrap();
    assert!(
        crate::task_authorization_scope::enforce_task_authorized_method(&mut sessions, &wrong_tab,)
            .is_err()
    );
}

#[rstest]
fn a_valid_denial_is_terminal_and_browser_bootstrap_cannot_mint_a_task_lease() {
    let (verifier, _admin, _host, signer) = bridge();
    let denied_challenge = verifier.prepare(exact_window_request()).unwrap();
    let denial = signed_receipt(
        &denied_challenge,
        &signer,
        CrossClientTaskAuthorizationDecision::Deny,
    );
    assert_eq!(
        verifier.consume(&denial).unwrap(),
        CrossClientTaskAuthorizationConsumption::Denied
    );
    assert_eq!(
        verifier.consume(&denial).unwrap_err(),
        CrossClientTaskAuthorizationError::Replay
    );

    let mut bootstrap = exact_window_request();
    bootstrap.target = CrossClientTaskAuthorizationTarget::BrowserBootstrapOwned {
        browser: dcc_cua_core::ComputerUseOwnedBrowserFamily::Chromium,
        profile: dcc_cua_core::ComputerUseOwnedBrowserProfile::IsolatedNew,
    };
    bootstrap.allowed_host_methods = vec!["launch_owned_browser".into()];
    bootstrap.allowed_actions.clear();
    let challenge = verifier.prepare(bootstrap).unwrap();
    let receipt = signed_receipt(
        &challenge,
        &signer,
        CrossClientTaskAuthorizationDecision::Allow,
    );
    assert!(matches!(
        verifier.consume(&receipt).unwrap(),
        CrossClientTaskAuthorizationConsumption::BrowserBootstrap(_)
    ));
}

#[rstest]
#[tokio::test]
async fn live_key_revocation_refuses_the_next_scoped_action() {
    let (verifier, admin, host, signer) = bridge();
    let challenge = verifier.prepare(exact_window_request()).unwrap();
    let receipt = signed_receipt(
        &challenge,
        &signer,
        CrossClientTaskAuthorizationDecision::Allow,
    );
    let opaque = match verifier.consume(&receipt).unwrap() {
        CrossClientTaskAuthorizationConsumption::Task(receipt) => receipt,
        other => panic!("expected task authorization, got {other:?}"),
    };
    let lease = issue_task_authorization(
        Some(host.as_ref()),
        TaskAuthorizationBinding::window(
            "connection-1",
            &opaque.authorization_id,
            "task-1",
            "grant-1",
            "Bound publishing task",
            &opaque.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
        ),
    )
    .await
    .unwrap();
    admin.revoke("issuer-key-1", 1).unwrap();

    let mut action = serde_json::to_value(HostAction {
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
        text: Some("bounded text".into()),
        secret_handle: None,
        delay_ms: None,
        type_chars_only: true,
        checked: None,
        keys: Vec::new(),
        modifiers: Vec::new(),
        duration_ms: None,
        steps: None,
    })
    .unwrap();
    action["authorization_category"] = json!("raw_input");
    let confirmation = TrustedActionConfirmationRequest::for_bound_window_action_value(
        ConfirmationBinding::window(
            "task-1",
            "grant-1",
            &opaque.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
            "observation-1",
            Some("accessibility-1"),
        ),
        "ordinary_edit",
        action,
    )
    .unwrap();

    assert_eq!(
        authorize_task_scoped_action(Some(host.as_ref()), Some(&lease), &confirmation).await,
        TaskAuthorizationOutcome::Revoked
    );
}

#[rstest]
#[tokio::test]
async fn live_key_revocation_refuses_the_next_observation_before_driver_access() {
    let (verifier, admin, host, signer) = bridge();
    let mut request = exact_window_request();
    request.target = CrossClientTaskAuthorizationTarget::ExactWindow {
        process_id: 42,
        process_creation_identity: "windows-filetime:123456".into(),
        window_handle: 77,
    };
    let challenge = verifier.prepare(request).unwrap();
    let receipt = signed_receipt(
        &challenge,
        &signer,
        CrossClientTaskAuthorizationDecision::Allow,
    );
    let opaque = match verifier.consume(&receipt).unwrap() {
        CrossClientTaskAuthorizationConsumption::Task(receipt) => receipt,
        other => panic!("expected task authorization, got {other:?}"),
    };
    let lease = issue_task_authorization(
        Some(host.as_ref()),
        TaskAuthorizationBinding::window(
            "connection-1",
            &opaque.authorization_id,
            "task-1",
            "grant-1",
            "Bound publishing task",
            &opaque.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 77,
            },
        ),
    )
    .await
    .unwrap();
    let driver = ComputerUseDriver::create().unwrap();
    let mut session = cached_host_session(&driver);
    session.capability = opaque.window_capability.clone();
    session.task_authorization = Some(lease);
    session.task_authorization_host = Some(Arc::clone(&host));
    session.latest_observation_id = Some("stale-native-observation".into());
    let mut sessions = std::collections::HashMap::from([("task-1".into(), session)]);
    admin.revoke("issuer-key-1", 1).unwrap();

    refresh_connection_session_states(&mut sessions).await;
    assert!(sessions["task-1"].latest_observation_id.is_none());

    let error = match authorized_session(
        &mut sessions,
        "task-1",
        "grant-1",
        &opaque.window_capability,
    )
    .await
    {
        Ok(_) => panic!("revocation must refuse observation access"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        HostError::CodedProtocol {
            code: HostProtocolErrorCode::TaskAuthorizationRevoked,
            ..
        }
    ));
}

#[rstest]
#[tokio::test]
async fn process_instance_drift_refuses_the_next_lease_validation() {
    let signer = signing_key();
    let key = TrustedTaskAuthorizationKey {
        key_id: "issuer-key-1".into(),
        audience: audience(),
        verifying_key: signer.verifying_key().to_bytes(),
    };
    let (_admin, trust) = trusted_task_authorization_trust_registry(vec![key]).unwrap();
    let observer = Arc::new(MutableTargetObserver(Mutex::new(
        "windows-filetime:123456".into(),
    )));
    let (verifier, host) = cross_client_task_authorization_with_clock_and_target_observer(
        runtime(),
        trust,
        Arc::new(FixedClock(test_now())),
        Arc::clone(&observer) as Arc<dyn CrossClientTaskAuthorizationTargetObserver>,
    )
    .unwrap();
    let challenge = verifier.prepare(exact_window_request()).unwrap();
    let receipt = signed_receipt(
        &challenge,
        &signer,
        CrossClientTaskAuthorizationDecision::Allow,
    );
    let opaque = match verifier.consume(&receipt).unwrap() {
        CrossClientTaskAuthorizationConsumption::Task(receipt) => receipt,
        other => panic!("expected task authorization, got {other:?}"),
    };
    let lease = issue_task_authorization(
        Some(host.as_ref()),
        TaskAuthorizationBinding::window(
            "connection-1",
            &opaque.authorization_id,
            "task-1",
            "grant-1",
            "Bound publishing task",
            &opaque.window_capability,
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
        ),
    )
    .await
    .unwrap();
    *observer.0.lock().unwrap() = "windows-filetime:654321".into();

    assert!(
        crate::task_authorization::validate_active_task_authorization(
            Some(host.as_ref()),
            Some(&lease),
        )
        .await
        .is_err()
    );
}
