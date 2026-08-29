use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::task_authorization::{
    MAX_TASK_AUTHORIZATION_ACTIONS, MAX_TASK_AUTHORIZATION_TTL_MS, TrustedTaskAuthorizationHost,
    TrustedTaskAuthorizationHostError, TrustedTaskAuthorizationLease,
    TrustedTaskAuthorizationLeaseValidationRequest, TrustedTaskAuthorizationRequest,
    TrustedTaskAuthorizationStatus, TrustedTaskAuthorizationValidationDecision,
    TrustedTaskAuthorizationValidationRequest,
};
use crate::{
    TrustedTaskActionScope, TrustedTaskAuthorizationBrokerError,
    TrustedTaskAuthorizationBrowserScope, TrustedTaskAuthorizationIssuer,
    TrustedTaskAuthorizationReceipt, TrustedTaskAuthorizationRegistration,
    TrustedTaskAuthorizationTarget, trusted_task_authorization_broker,
};

pub const CROSS_CLIENT_TASK_AUTHORIZATION_CHALLENGE_SCHEMA: &str =
    "dcc-cua.task-authorization-challenge.v2";
pub const CROSS_CLIENT_TASK_AUTHORIZATION_RECEIPT_SCHEMA: &str =
    "dcc-cua.task-authorization-receipt.v2";
const SIGNING_DOMAIN: &[u8] = b"dcc-cua.task-authorization-receipt.v2";
const PROVIDER: &str = "dcc-cua";
const MAX_PENDING_CHALLENGES: usize = 256;
const MAX_PENDING_TTL_MS: u64 = 5 * 60 * 1_000;
const MAX_SAFE_JSON_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_IDENTITY_CHARS: usize = 256;
const MAX_LABEL_CHARS: usize = 512;
const MAX_HOST_METHODS: usize = 64;
const MAX_SIGNED_RECEIPT_BYTES: usize = 16 * 1_024;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossClientTaskAuthorizationAudience {
    pub client_id: String,
    pub tenant_id: String,
    pub user_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossClientTaskAuthorizationRuntime {
    pub runtime_version: String,
    pub runtime_instance_id: String,
    pub boot_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossClientTaskAuthorizationSecretPolicy {
    Forbidden,
    NamedHandlesOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CrossClientTaskAuthorizationTarget {
    ExactWindow {
        process_id: u32,
        process_creation_identity: String,
        window_handle: u64,
    },
    ExactBrowser {
        process_id: u32,
        process_creation_identity: String,
        window_handle: u64,
        host_target_id: String,
        tab_id: String,
        document_generation: String,
        origin: String,
    },
    BrowserBootstrapExactWindow {
        process_id: u32,
        process_creation_identity: String,
        window_handle: u64,
    },
    BrowserBootstrapOwned {
        browser: dcc_cua_core::ComputerUseOwnedBrowserFamily,
        profile: dcc_cua_core::ComputerUseOwnedBrowserProfile,
    },
}

impl CrossClientTaskAuthorizationTarget {
    fn is_bootstrap(&self) -> bool {
        matches!(
            self,
            Self::BrowserBootstrapExactWindow { .. } | Self::BrowserBootstrapOwned { .. }
        )
    }

    fn exact_window(&self) -> Option<(u32, u64)> {
        match self {
            Self::ExactWindow {
                process_id,
                window_handle,
                ..
            }
            | Self::ExactBrowser {
                process_id,
                window_handle,
                ..
            } => Some((*process_id, *window_handle)),
            Self::BrowserBootstrapExactWindow { .. } | Self::BrowserBootstrapOwned { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossClientTaskAuthorizationChallengeRequest {
    pub audience: CrossClientTaskAuthorizationAudience,
    pub task_id: String,
    pub connection_id: String,
    pub task_grant_id: String,
    pub application_label: String,
    pub purpose: String,
    pub target: CrossClientTaskAuthorizationTarget,
    pub allowed_host_methods: Vec<String>,
    pub allowed_actions: Vec<TrustedTaskActionScope>,
    pub allowed_browser_origins: Vec<String>,
    pub secret_handle_policy: CrossClientTaskAuthorizationSecretPolicy,
    pub irreversible_operation: Option<String>,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CrossClientTaskAuthorizationChallenge {
    pub schema: String,
    pub provider: String,
    pub runtime_version: String,
    pub runtime_instance_id: String,
    pub boot_generation: u64,
    pub audience: CrossClientTaskAuthorizationAudience,
    pub challenge_id: String,
    pub nonce: String,
    pub task_id: String,
    pub connection_id: String,
    pub task_grant_id: String,
    pub application_label: String,
    pub purpose: String,
    pub target: CrossClientTaskAuthorizationTarget,
    pub allowed_host_methods: Vec<String>,
    pub allowed_actions: Vec<TrustedTaskActionScope>,
    pub allowed_browser_origins: Vec<String>,
    pub secret_handle_policy: CrossClientTaskAuthorizationSecretPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub irreversible_operation: Option<String>,
    pub issued_at_unix_ms: u64,
    pub confirmation_deadline_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    #[serde(skip_serializing)]
    pub challenge_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossClientTaskAuthorizationDecision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossClientTaskAuthorizationUnsignedDecision {
    pub schema: String,
    pub issuer_key_id: String,
    pub audience: CrossClientTaskAuthorizationAudience,
    pub runtime_instance_id: String,
    pub boot_generation: u64,
    pub challenge_id: String,
    pub nonce: String,
    pub challenge_sha256: String,
    pub decision: CrossClientTaskAuthorizationDecision,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

impl CrossClientTaskAuthorizationUnsignedDecision {
    pub fn for_challenge(
        challenge: &CrossClientTaskAuthorizationChallenge,
        issuer_key_id: impl Into<String>,
        decision: CrossClientTaskAuthorizationDecision,
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
    ) -> Result<Self, CrossClientTaskAuthorizationError> {
        let challenge_sha256 = challenge_digest(challenge)?;
        if challenge_sha256 != challenge.challenge_sha256 {
            return Err(CrossClientTaskAuthorizationError::InvalidChallenge);
        }
        Ok(Self {
            schema: CROSS_CLIENT_TASK_AUTHORIZATION_RECEIPT_SCHEMA.into(),
            issuer_key_id: issuer_key_id.into(),
            audience: challenge.audience.clone(),
            runtime_instance_id: challenge.runtime_instance_id.clone(),
            boot_generation: challenge.boot_generation,
            challenge_id: challenge.challenge_id.clone(),
            nonce: challenge.nonce.clone(),
            challenge_sha256,
            decision,
            issued_at_unix_ms,
            expires_at_unix_ms,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossClientTaskAuthorizationSignedDecision {
    pub decision: CrossClientTaskAuthorizationUnsignedDecision,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedTaskAuthorizationKey {
    pub key_id: String,
    pub audience: CrossClientTaskAuthorizationAudience,
    pub verifying_key: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossClientTaskAuthorizationTrustState {
    Active(TrustedTaskAuthorizationKey),
    Revoked { revocation_epoch: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the constructor-provisioned task authorization trust registry is unavailable")]
pub struct CrossClientTaskAuthorizationTrustError;

pub trait CrossClientTaskAuthorizationTrustStore: Send + Sync {
    fn lookup(
        &self,
        key_id: &str,
    ) -> Result<
        Option<CrossClientTaskAuthorizationTrustState>,
        CrossClientTaskAuthorizationTrustError,
    >;
}

pub trait CrossClientTaskAuthorizationClock: Send + Sync {
    fn unix_time_millis(&self) -> Result<u64, CrossClientTaskAuthorizationError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the exact target process creation identity is unavailable")]
pub struct CrossClientTaskAuthorizationTargetError;

pub trait CrossClientTaskAuthorizationTargetObserver: Send + Sync {
    fn process_creation_identity(
        &self,
        process_id: u32,
        window_handle: u64,
    ) -> Result<String, CrossClientTaskAuthorizationTargetError>;
}

struct SystemTargetObserver;

#[cfg(windows)]
impl CrossClientTaskAuthorizationTargetObserver for SystemTargetObserver {
    fn process_creation_identity(
        &self,
        process_id: u32,
        window_handle: u64,
    ) -> Result<String, CrossClientTaskAuthorizationTargetError> {
        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
        use windows_sys::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

        let window_handle =
            usize::try_from(window_handle).map_err(|_| CrossClientTaskAuthorizationTargetError)?;
        let mut observed_process_id = 0_u32;
        // SAFETY: the integer handle was range-checked for this pointer width and
        // the output points to a live local `u32` for the duration of the call.
        let thread_id = unsafe {
            GetWindowThreadProcessId(
                window_handle as *mut core::ffi::c_void,
                &mut observed_process_id,
            )
        };
        if thread_id == 0 || observed_process_id != process_id {
            return Err(CrossClientTaskAuthorizationTargetError);
        }
        // SAFETY: `OpenProcess` accepts the value-only PID and returns an owned
        // handle whose null result is checked immediately.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if process.is_null() {
            return Err(CrossClientTaskAuthorizationTargetError);
        }
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: `process` is a live non-null handle and every FILETIME output
        // remains valid and exclusively borrowed for this call.
        let succeeded = unsafe {
            GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) != 0
        };
        // SAFETY: this closes exactly the owned non-null handle returned above.
        unsafe { CloseHandle(process) };
        if !succeeded {
            return Err(CrossClientTaskAuthorizationTargetError);
        }
        let created =
            (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
        Ok(format!("windows-filetime:{created}"))
    }
}

#[cfg(not(windows))]
impl CrossClientTaskAuthorizationTargetObserver for SystemTargetObserver {
    fn process_creation_identity(
        &self,
        _process_id: u32,
        _window_handle: u64,
    ) -> Result<String, CrossClientTaskAuthorizationTargetError> {
        Err(CrossClientTaskAuthorizationTargetError)
    }
}

struct SystemClock;

impl CrossClientTaskAuthorizationClock for SystemClock {
    fn unix_time_millis(&self) -> Result<u64, CrossClientTaskAuthorizationError> {
        Ok(crate::task_authorization::unix_time_millis())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CrossClientTaskAuthorizationError {
    #[error("the cross-client task authorization configuration is invalid")]
    InvalidConfiguration,
    #[error("the task authorization challenge is invalid")]
    InvalidChallenge,
    #[error("the pending task authorization registry is full")]
    Capacity,
    #[error("the trusted task authorization clock is unavailable")]
    ClockUnavailable,
    #[error("the trusted task authorization clock moved backwards")]
    ClockRollback,
    #[error("the exact target process instance is unavailable or changed")]
    TargetUnavailable,
    #[error("the signed task authorization receipt is malformed")]
    MalformedReceipt,
    #[error("the signed task authorization receipt is not canonical JCS")]
    NoncanonicalReceipt,
    #[error("the receipt issuer key is unknown")]
    UnknownKey,
    #[error("the receipt issuer key is revoked")]
    RevokedKey,
    #[error("the receipt issuer trust state is unavailable")]
    TrustUnavailable,
    #[error("the task authorization signature is invalid")]
    InvalidSignature,
    #[error("the signed decision does not match the retained challenge")]
    BindingMismatch,
    #[error("the task authorization decision is expired or outside the challenge deadline")]
    Expired,
    #[error("the task authorization challenge was already consumed")]
    Replay,
    #[error("the constructor-owned task authorization broker is unavailable")]
    BrokerUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossClientTaskAuthorizationBootstrap {
    pub challenge_id: String,
    pub target: CrossClientTaskAuthorizationTarget,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossClientTaskAuthorizationConsumption {
    Task(TrustedTaskAuthorizationReceipt),
    BrowserBootstrap(CrossClientTaskAuthorizationBootstrap),
    Denied,
}

#[derive(Clone)]
struct AuthorizationProvenance {
    key: TrustedTaskAuthorizationKey,
    process_creation_identity: String,
    expires_at_unix_ms: u64,
}

struct ChallengeRecord {
    challenge: CrossClientTaskAuthorizationChallenge,
    consumed: bool,
}

#[derive(Default)]
struct VerifierState {
    challenges: BTreeMap<String, ChallengeRecord>,
    authorizations: BTreeMap<String, AuthorizationProvenance>,
    last_observed_unix_ms: Option<u64>,
    clock_failed: bool,
}

pub struct CrossClientTaskAuthorizationVerifier {
    runtime: CrossClientTaskAuthorizationRuntime,
    trust: Arc<dyn CrossClientTaskAuthorizationTrustStore>,
    clock: Arc<dyn CrossClientTaskAuthorizationClock>,
    target_observer: Arc<dyn CrossClientTaskAuthorizationTargetObserver>,
    state: Arc<Mutex<VerifierState>>,
    issuer: TrustedTaskAuthorizationIssuer,
}

impl CrossClientTaskAuthorizationVerifier {
    pub fn prepare(
        &self,
        request: CrossClientTaskAuthorizationChallengeRequest,
    ) -> Result<CrossClientTaskAuthorizationChallenge, CrossClientTaskAuthorizationError> {
        let now = self.clock.unix_time_millis()?;
        validate_challenge_request(&request, now)?;
        validate_live_target(self.target_observer.as_ref(), &request.target)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| CrossClientTaskAuthorizationError::TrustUnavailable)?;
        observe_clock(&mut state, now)?;
        state
            .challenges
            .retain(|_, record| record.challenge.confirmation_deadline_unix_ms >= now);
        if state.challenges.len() >= MAX_PENDING_CHALLENGES {
            return Err(CrossClientTaskAuthorizationError::Capacity);
        }
        let challenge_id = format!("task-challenge-{}", Uuid::new_v4());
        let mut nonce_bytes = [0_u8; 32];
        nonce_bytes[..16].copy_from_slice(Uuid::new_v4().as_bytes());
        nonce_bytes[16..].copy_from_slice(Uuid::new_v4().as_bytes());
        let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(nonce_bytes);
        let mut challenge = CrossClientTaskAuthorizationChallenge {
            schema: CROSS_CLIENT_TASK_AUTHORIZATION_CHALLENGE_SCHEMA.into(),
            provider: PROVIDER.into(),
            runtime_version: self.runtime.runtime_version.clone(),
            runtime_instance_id: self.runtime.runtime_instance_id.clone(),
            boot_generation: self.runtime.boot_generation,
            audience: request.audience,
            challenge_id: challenge_id.clone(),
            nonce,
            task_id: request.task_id,
            connection_id: request.connection_id,
            task_grant_id: request.task_grant_id,
            application_label: request.application_label,
            purpose: request.purpose,
            target: request.target,
            allowed_host_methods: request.allowed_host_methods,
            allowed_actions: request.allowed_actions,
            allowed_browser_origins: request.allowed_browser_origins,
            secret_handle_policy: request.secret_handle_policy,
            irreversible_operation: request.irreversible_operation,
            issued_at_unix_ms: now,
            confirmation_deadline_unix_ms: request
                .expires_at_unix_ms
                .min(now.saturating_add(MAX_PENDING_TTL_MS)),
            expires_at_unix_ms: request.expires_at_unix_ms,
            challenge_sha256: String::new(),
        };
        challenge.challenge_sha256 = challenge_digest(&challenge)?;
        state.challenges.insert(
            challenge_id,
            ChallengeRecord {
                challenge: challenge.clone(),
                consumed: false,
            },
        );
        Ok(challenge)
    }

    pub fn consume(
        &self,
        encoded_receipt: &[u8],
    ) -> Result<CrossClientTaskAuthorizationConsumption, CrossClientTaskAuthorizationError> {
        let receipt = decode_receipt(encoded_receipt)?;
        let now = self.clock.unix_time_millis()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| CrossClientTaskAuthorizationError::TrustUnavailable)?;
        observe_clock(&mut state, now)?;
        let record = state
            .challenges
            .get(&receipt.decision.challenge_id)
            .ok_or(CrossClientTaskAuthorizationError::BindingMismatch)?;
        if record.consumed {
            return Err(CrossClientTaskAuthorizationError::Replay);
        }
        let challenge = record.challenge.clone();
        validate_decision_binding(&self.runtime, &challenge, &receipt.decision, now)?;
        let key = match self
            .trust
            .lookup(&receipt.decision.issuer_key_id)
            .map_err(|_| CrossClientTaskAuthorizationError::TrustUnavailable)?
        {
            Some(CrossClientTaskAuthorizationTrustState::Active(key)) => key,
            Some(CrossClientTaskAuthorizationTrustState::Revoked { .. }) => {
                return Err(CrossClientTaskAuthorizationError::RevokedKey);
            }
            None => return Err(CrossClientTaskAuthorizationError::UnknownKey),
        };
        if key.audience != challenge.audience {
            return Err(CrossClientTaskAuthorizationError::BindingMismatch);
        }
        verify_signature(&key, &receipt)?;

        if receipt.decision.decision == CrossClientTaskAuthorizationDecision::Allow {
            validate_live_target(self.target_observer.as_ref(), &challenge.target)?;
        }

        state
            .challenges
            .get_mut(&challenge.challenge_id)
            .ok_or(CrossClientTaskAuthorizationError::BindingMismatch)?
            .consumed = true;
        if receipt.decision.decision == CrossClientTaskAuthorizationDecision::Deny {
            return Ok(CrossClientTaskAuthorizationConsumption::Denied);
        }
        if challenge.target.is_bootstrap() {
            return Ok(CrossClientTaskAuthorizationConsumption::BrowserBootstrap(
                CrossClientTaskAuthorizationBootstrap {
                    challenge_id: challenge.challenge_id,
                    target: challenge.target,
                    expires_at_unix_ms: receipt.decision.expires_at_unix_ms,
                },
            ));
        }
        let (process_id, window_handle) = challenge
            .target
            .exact_window()
            .ok_or(CrossClientTaskAuthorizationError::InvalidChallenge)?;
        let process_creation_identity = match &challenge.target {
            CrossClientTaskAuthorizationTarget::ExactWindow {
                process_creation_identity,
                ..
            }
            | CrossClientTaskAuthorizationTarget::ExactBrowser {
                process_creation_identity,
                ..
            } => process_creation_identity.clone(),
            CrossClientTaskAuthorizationTarget::BrowserBootstrapExactWindow { .. }
            | CrossClientTaskAuthorizationTarget::BrowserBootstrapOwned { .. } => {
                return Err(CrossClientTaskAuthorizationError::InvalidChallenge);
            }
        };
        let browser_scope = match &challenge.target {
            CrossClientTaskAuthorizationTarget::ExactBrowser {
                host_target_id,
                tab_id,
                document_generation,
                origin,
                ..
            } => Some(TrustedTaskAuthorizationBrowserScope {
                host_target_id: host_target_id.clone(),
                tab_id: tab_id.clone(),
                document_generation: document_generation.clone(),
                origin: origin.clone(),
            }),
            CrossClientTaskAuthorizationTarget::ExactWindow { .. }
            | CrossClientTaskAuthorizationTarget::BrowserBootstrapExactWindow { .. }
            | CrossClientTaskAuthorizationTarget::BrowserBootstrapOwned { .. } => None,
        };
        let opaque = self
            .issuer
            .register(TrustedTaskAuthorizationRegistration {
                connection_id: Some(challenge.connection_id),
                task_id: Some(challenge.task_id),
                task_grant_id: challenge.task_grant_id,
                application_label: challenge.application_label,
                target: TrustedTaskAuthorizationTarget::ExactWindow {
                    process_id,
                    window_handle,
                },
                allowed_host_methods: challenge.allowed_host_methods,
                allowed_actions: challenge.allowed_actions,
                allowed_browser_origins: challenge.allowed_browser_origins,
                browser_scope,
                expires_at_unix_ms: receipt.decision.expires_at_unix_ms,
            })
            .map_err(map_broker_error)?;
        state
            .authorizations
            .retain(|_, provenance| provenance.expires_at_unix_ms > now);
        state.authorizations.insert(
            opaque.authorization_id.clone(),
            AuthorizationProvenance {
                key,
                process_creation_identity,
                expires_at_unix_ms: receipt.decision.expires_at_unix_ms,
            },
        );
        Ok(CrossClientTaskAuthorizationConsumption::Task(opaque))
    }
}

pub fn cross_client_task_authorization(
    runtime: CrossClientTaskAuthorizationRuntime,
    trust: Arc<dyn CrossClientTaskAuthorizationTrustStore>,
) -> Result<
    (
        CrossClientTaskAuthorizationVerifier,
        Arc<dyn TrustedTaskAuthorizationHost>,
    ),
    CrossClientTaskAuthorizationError,
> {
    cross_client_task_authorization_with_clock_and_target_observer(
        runtime,
        trust,
        Arc::new(SystemClock),
        Arc::new(SystemTargetObserver),
    )
}

pub fn cross_client_task_authorization_with_clock(
    runtime: CrossClientTaskAuthorizationRuntime,
    trust: Arc<dyn CrossClientTaskAuthorizationTrustStore>,
    clock: Arc<dyn CrossClientTaskAuthorizationClock>,
) -> Result<
    (
        CrossClientTaskAuthorizationVerifier,
        Arc<dyn TrustedTaskAuthorizationHost>,
    ),
    CrossClientTaskAuthorizationError,
> {
    cross_client_task_authorization_with_clock_and_target_observer(
        runtime,
        trust,
        clock,
        Arc::new(SystemTargetObserver),
    )
}

pub fn cross_client_task_authorization_with_clock_and_target_observer(
    runtime: CrossClientTaskAuthorizationRuntime,
    trust: Arc<dyn CrossClientTaskAuthorizationTrustStore>,
    clock: Arc<dyn CrossClientTaskAuthorizationClock>,
    target_observer: Arc<dyn CrossClientTaskAuthorizationTargetObserver>,
) -> Result<
    (
        CrossClientTaskAuthorizationVerifier,
        Arc<dyn TrustedTaskAuthorizationHost>,
    ),
    CrossClientTaskAuthorizationError,
> {
    validate_runtime(&runtime)?;
    let (issuer, inner) = trusted_task_authorization_broker();
    let state = Arc::new(Mutex::new(VerifierState::default()));
    let host_target_observer = Arc::clone(&target_observer);
    let host: Arc<dyn TrustedTaskAuthorizationHost> = Arc::new(CrossClientAuthorizationHost {
        inner,
        trust: Arc::clone(&trust),
        state: Arc::clone(&state),
        clock: Arc::clone(&clock),
        target_observer: host_target_observer,
    });
    Ok((
        CrossClientTaskAuthorizationVerifier {
            runtime,
            trust,
            clock,
            target_observer,
            state,
            issuer,
        },
        host,
    ))
}

pub fn cross_client_task_authorization_signing_payload(
    decision: &CrossClientTaskAuthorizationUnsignedDecision,
) -> Result<Vec<u8>, CrossClientTaskAuthorizationError> {
    let canonical = serde_jcs::to_vec(decision)
        .map_err(|_| CrossClientTaskAuthorizationError::MalformedReceipt)?;
    let mut payload = Vec::with_capacity(SIGNING_DOMAIN.len() + 1 + canonical.len());
    payload.extend_from_slice(SIGNING_DOMAIN);
    payload.push(0);
    payload.extend_from_slice(&canonical);
    Ok(payload)
}

pub fn canonical_cross_client_task_authorization_receipt(
    receipt: &CrossClientTaskAuthorizationSignedDecision,
) -> Result<Vec<u8>, CrossClientTaskAuthorizationError> {
    serde_jcs::to_vec(receipt).map_err(|_| CrossClientTaskAuthorizationError::MalformedReceipt)
}

pub fn canonical_cross_client_task_authorization_challenge(
    challenge: &CrossClientTaskAuthorizationChallenge,
) -> Result<Vec<u8>, CrossClientTaskAuthorizationError> {
    serde_jcs::to_vec(challenge).map_err(|_| CrossClientTaskAuthorizationError::InvalidChallenge)
}

fn decode_receipt(
    encoded: &[u8],
) -> Result<CrossClientTaskAuthorizationSignedDecision, CrossClientTaskAuthorizationError> {
    if encoded.is_empty() || encoded.len() > MAX_SIGNED_RECEIPT_BYTES {
        return Err(CrossClientTaskAuthorizationError::MalformedReceipt);
    }
    let receipt = serde_json::from_slice::<CrossClientTaskAuthorizationSignedDecision>(encoded)
        .map_err(|_| CrossClientTaskAuthorizationError::MalformedReceipt)?;
    let canonical = canonical_cross_client_task_authorization_receipt(&receipt)?;
    if canonical != encoded {
        return Err(CrossClientTaskAuthorizationError::NoncanonicalReceipt);
    }
    validate_safe_integer(receipt.decision.boot_generation)?;
    validate_safe_integer(receipt.decision.issued_at_unix_ms)?;
    validate_safe_integer(receipt.decision.expires_at_unix_ms)?;
    for value in [
        &receipt.decision.schema,
        &receipt.decision.issuer_key_id,
        &receipt.decision.runtime_instance_id,
        &receipt.decision.challenge_id,
        &receipt.decision.nonce,
        &receipt.decision.challenge_sha256,
        &receipt.signature,
    ] {
        validate_identity(value, MAX_LABEL_CHARS)
            .map_err(|_| CrossClientTaskAuthorizationError::MalformedReceipt)?;
    }
    validate_audience(&receipt.decision.audience)
        .map_err(|_| CrossClientTaskAuthorizationError::MalformedReceipt)?;
    Ok(receipt)
}

fn verify_signature(
    key: &TrustedTaskAuthorizationKey,
    receipt: &CrossClientTaskAuthorizationSignedDecision,
) -> Result<(), CrossClientTaskAuthorizationError> {
    let verifying_key = VerifyingKey::from_bytes(&key.verifying_key)
        .map_err(|_| CrossClientTaskAuthorizationError::InvalidConfiguration)?;
    let signature_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&receipt.signature)
        .map_err(|_| CrossClientTaskAuthorizationError::MalformedReceipt)?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| CrossClientTaskAuthorizationError::MalformedReceipt)?;
    let payload = cross_client_task_authorization_signing_payload(&receipt.decision)?;
    verifying_key
        .verify_strict(&payload, &signature)
        .map_err(|_| CrossClientTaskAuthorizationError::InvalidSignature)
}

fn validate_decision_binding(
    runtime: &CrossClientTaskAuthorizationRuntime,
    challenge: &CrossClientTaskAuthorizationChallenge,
    decision: &CrossClientTaskAuthorizationUnsignedDecision,
    now: u64,
) -> Result<(), CrossClientTaskAuthorizationError> {
    if decision.schema != CROSS_CLIENT_TASK_AUTHORIZATION_RECEIPT_SCHEMA
        || decision.audience != challenge.audience
        || decision.runtime_instance_id != runtime.runtime_instance_id
        || decision.boot_generation != runtime.boot_generation
        || decision.challenge_id != challenge.challenge_id
        || decision.nonce != challenge.nonce
        || decision.challenge_sha256 != challenge.challenge_sha256
    {
        return Err(CrossClientTaskAuthorizationError::BindingMismatch);
    }
    if decision.issued_at_unix_ms < challenge.issued_at_unix_ms
        || decision.issued_at_unix_ms > now
        || decision.issued_at_unix_ms > challenge.confirmation_deadline_unix_ms
        || now > challenge.confirmation_deadline_unix_ms
        || decision.expires_at_unix_ms <= now
        || decision.expires_at_unix_ms > challenge.expires_at_unix_ms
        || decision.expires_at_unix_ms < decision.issued_at_unix_ms
    {
        return Err(CrossClientTaskAuthorizationError::Expired);
    }
    Ok(())
}

fn validate_runtime(
    runtime: &CrossClientTaskAuthorizationRuntime,
) -> Result<(), CrossClientTaskAuthorizationError> {
    validate_identity(&runtime.runtime_version, MAX_IDENTITY_CHARS)
        .map_err(|_| CrossClientTaskAuthorizationError::InvalidConfiguration)?;
    validate_identity(&runtime.runtime_instance_id, MAX_IDENTITY_CHARS)
        .map_err(|_| CrossClientTaskAuthorizationError::InvalidConfiguration)?;
    validate_safe_integer(runtime.boot_generation)
        .map_err(|_| CrossClientTaskAuthorizationError::InvalidConfiguration)?;
    Ok(())
}

fn validate_challenge_request(
    request: &CrossClientTaskAuthorizationChallengeRequest,
    now: u64,
) -> Result<(), CrossClientTaskAuthorizationError> {
    validate_audience(&request.audience)?;
    validate_identity(&request.task_id, MAX_IDENTITY_CHARS)?;
    validate_identity(&request.connection_id, MAX_IDENTITY_CHARS)?;
    validate_identity(&request.task_grant_id, crate::MAX_TASK_GRANT_ID_CHARS)?;
    validate_identity(
        &request.application_label,
        crate::MAX_APPLICATION_LABEL_CHARS,
    )?;
    validate_label(&request.purpose)?;
    if let Some(operation) = request.irreversible_operation.as_deref() {
        validate_label(operation)?;
    }
    validate_safe_integer(now).map_err(|_| CrossClientTaskAuthorizationError::InvalidChallenge)?;
    validate_safe_integer(request.expires_at_unix_ms)
        .map_err(|_| CrossClientTaskAuthorizationError::InvalidChallenge)?;
    if request.expires_at_unix_ms <= now
        || request.expires_at_unix_ms.saturating_sub(now) > MAX_TASK_AUTHORIZATION_TTL_MS
    {
        return Err(CrossClientTaskAuthorizationError::InvalidChallenge);
    }
    validate_target(&request.target)?;
    let methods = request.allowed_host_methods.iter().collect::<BTreeSet<_>>();
    if methods.is_empty()
        || methods.len() != request.allowed_host_methods.len()
        || methods.len() > MAX_HOST_METHODS
        || methods
            .iter()
            .any(|method| validate_identity(method, 80).is_err())
    {
        return Err(CrossClientTaskAuthorizationError::InvalidChallenge);
    }
    if request.target.is_bootstrap() {
        let expected = match request.target {
            CrossClientTaskAuthorizationTarget::BrowserBootstrapExactWindow { .. } => {
                "attach_exact_window"
            }
            CrossClientTaskAuthorizationTarget::BrowserBootstrapOwned { .. } => {
                "launch_owned_browser"
            }
            _ => unreachable!(),
        };
        if request.allowed_host_methods != [expected]
            || !request.allowed_actions.is_empty()
            || !request.allowed_browser_origins.is_empty()
            || request.secret_handle_policy != CrossClientTaskAuthorizationSecretPolicy::Forbidden
            || request.irreversible_operation.is_some()
        {
            return Err(CrossClientTaskAuthorizationError::InvalidChallenge);
        }
        return Ok(());
    }
    if request
        .allowed_host_methods
        .iter()
        .any(|method| !crate::task_authorization_scope::is_task_authorizable_host_method(method))
    {
        return Err(CrossClientTaskAuthorizationError::InvalidChallenge);
    }
    let actions = request
        .allowed_actions
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if actions.is_empty()
        || actions.len() != request.allowed_actions.len()
        || actions.len() > MAX_TASK_AUTHORIZATION_ACTIONS
        || !actions.iter().all(TrustedTaskActionScope::validate)
        || (request.secret_handle_policy == CrossClientTaskAuthorizationSecretPolicy::Forbidden
            && actions.iter().any(|scope| scope.secret_input))
    {
        return Err(CrossClientTaskAuthorizationError::InvalidChallenge);
    }
    let origins = request
        .allowed_browser_origins
        .iter()
        .collect::<BTreeSet<_>>();
    if origins.len() != request.allowed_browser_origins.len()
        || origins.len() > MAX_TASK_AUTHORIZATION_ACTIONS
        || origins
            .iter()
            .any(|origin| !crate::task_authorization::valid_browser_origin(origin))
    {
        return Err(CrossClientTaskAuthorizationError::InvalidChallenge);
    }
    match &request.target {
        CrossClientTaskAuthorizationTarget::ExactWindow { .. } => {
            if !request.allowed_browser_origins.is_empty()
                || request
                    .allowed_actions
                    .iter()
                    .any(|scope| scope.browser_origin.is_some())
                || request.allowed_host_methods.iter().any(|method| {
                    method.starts_with("browser_") || method == "browser_extension_status"
                })
            {
                return Err(CrossClientTaskAuthorizationError::InvalidChallenge);
            }
        }
        CrossClientTaskAuthorizationTarget::ExactBrowser { origin, .. } => {
            if request.allowed_browser_origins.as_slice() != [origin.as_str()]
                || !request
                    .allowed_host_methods
                    .iter()
                    .any(|method| method == "browser_snapshot")
                || request.allowed_host_methods.iter().any(|method| {
                    matches!(
                        method.as_str(),
                        "browser_prepare" | "browser_extension_status" | "browser_extension_call"
                    )
                })
                || request
                    .allowed_actions
                    .iter()
                    .any(|scope| scope.browser_origin.as_deref() != Some(origin.as_str()))
            {
                return Err(CrossClientTaskAuthorizationError::InvalidChallenge);
            }
        }
        CrossClientTaskAuthorizationTarget::BrowserBootstrapExactWindow { .. }
        | CrossClientTaskAuthorizationTarget::BrowserBootstrapOwned { .. } => unreachable!(),
    }
    Ok(())
}

fn validate_target(
    target: &CrossClientTaskAuthorizationTarget,
) -> Result<(), CrossClientTaskAuthorizationError> {
    match target {
        CrossClientTaskAuthorizationTarget::ExactWindow {
            process_id,
            process_creation_identity,
            window_handle,
        }
        | CrossClientTaskAuthorizationTarget::BrowserBootstrapExactWindow {
            process_id,
            process_creation_identity,
            window_handle,
        } => {
            if *process_id == 0 || *window_handle == 0 {
                return Err(CrossClientTaskAuthorizationError::InvalidChallenge);
            }
            validate_safe_integer(*window_handle)
                .map_err(|_| CrossClientTaskAuthorizationError::InvalidChallenge)?;
            validate_identity(process_creation_identity, MAX_IDENTITY_CHARS)?;
        }
        CrossClientTaskAuthorizationTarget::ExactBrowser {
            process_id,
            process_creation_identity,
            window_handle,
            host_target_id,
            tab_id,
            document_generation,
            origin,
        } => {
            if *process_id == 0 || *window_handle == 0 {
                return Err(CrossClientTaskAuthorizationError::InvalidChallenge);
            }
            validate_safe_integer(*window_handle)
                .map_err(|_| CrossClientTaskAuthorizationError::InvalidChallenge)?;
            for value in [
                process_creation_identity,
                host_target_id,
                tab_id,
                document_generation,
            ] {
                validate_identity(value, MAX_IDENTITY_CHARS)?;
            }
            if !crate::task_authorization::valid_browser_origin(origin) {
                return Err(CrossClientTaskAuthorizationError::InvalidChallenge);
            }
        }
        CrossClientTaskAuthorizationTarget::BrowserBootstrapOwned { .. } => {}
    }
    Ok(())
}

fn validate_live_target(
    observer: &dyn CrossClientTaskAuthorizationTargetObserver,
    target: &CrossClientTaskAuthorizationTarget,
) -> Result<(), CrossClientTaskAuthorizationError> {
    let binding = match target {
        CrossClientTaskAuthorizationTarget::ExactWindow {
            process_id,
            process_creation_identity,
            window_handle,
        }
        | CrossClientTaskAuthorizationTarget::ExactBrowser {
            process_id,
            process_creation_identity,
            window_handle,
            ..
        }
        | CrossClientTaskAuthorizationTarget::BrowserBootstrapExactWindow {
            process_id,
            process_creation_identity,
            window_handle,
        } => Some((*process_id, *window_handle, process_creation_identity)),
        CrossClientTaskAuthorizationTarget::BrowserBootstrapOwned { .. } => None,
    };
    let Some((process_id, window_handle, expected_identity)) = binding else {
        return Ok(());
    };
    let observed = observer
        .process_creation_identity(process_id, window_handle)
        .map_err(|_| CrossClientTaskAuthorizationError::TargetUnavailable)?;
    if observed == *expected_identity {
        Ok(())
    } else {
        Err(CrossClientTaskAuthorizationError::TargetUnavailable)
    }
}

fn validate_audience(
    audience: &CrossClientTaskAuthorizationAudience,
) -> Result<(), CrossClientTaskAuthorizationError> {
    for value in [&audience.client_id, &audience.tenant_id, &audience.user_id] {
        validate_identity(value, MAX_IDENTITY_CHARS)?;
    }
    Ok(())
}

fn validate_identity(
    value: &str,
    max_chars: usize,
) -> Result<(), CrossClientTaskAuthorizationError> {
    if value.is_empty()
        || value != value.trim()
        || value.chars().count() > max_chars
        || value.chars().any(char::is_control)
    {
        return Err(CrossClientTaskAuthorizationError::InvalidChallenge);
    }
    Ok(())
}

fn validate_label(value: &str) -> Result<(), CrossClientTaskAuthorizationError> {
    validate_identity(value, MAX_LABEL_CHARS)
}

fn validate_safe_integer(value: u64) -> Result<(), CrossClientTaskAuthorizationError> {
    if value > MAX_SAFE_JSON_INTEGER {
        return Err(CrossClientTaskAuthorizationError::MalformedReceipt);
    }
    Ok(())
}

fn challenge_digest(
    challenge: &CrossClientTaskAuthorizationChallenge,
) -> Result<String, CrossClientTaskAuthorizationError> {
    let canonical = canonical_cross_client_task_authorization_challenge(challenge)?;
    Ok(format!("sha256:{:x}", Sha256::digest(canonical)))
}

fn observe_clock(
    state: &mut VerifierState,
    now: u64,
) -> Result<(), CrossClientTaskAuthorizationError> {
    if state.clock_failed {
        return Err(CrossClientTaskAuthorizationError::ClockRollback);
    }
    validate_safe_integer(now).map_err(|_| CrossClientTaskAuthorizationError::ClockUnavailable)?;
    if state
        .last_observed_unix_ms
        .is_some_and(|previous| now < previous)
    {
        state.clock_failed = true;
        return Err(CrossClientTaskAuthorizationError::ClockRollback);
    }
    state.last_observed_unix_ms = Some(now);
    Ok(())
}

fn map_broker_error(_: TrustedTaskAuthorizationBrokerError) -> CrossClientTaskAuthorizationError {
    CrossClientTaskAuthorizationError::BrokerUnavailable
}

struct CrossClientAuthorizationHost {
    inner: Arc<dyn TrustedTaskAuthorizationHost>,
    trust: Arc<dyn CrossClientTaskAuthorizationTrustStore>,
    state: Arc<Mutex<VerifierState>>,
    clock: Arc<dyn CrossClientTaskAuthorizationClock>,
    target_observer: Arc<dyn CrossClientTaskAuthorizationTargetObserver>,
}

impl CrossClientAuthorizationHost {
    fn validate_clock(&self) -> Result<(), TrustedTaskAuthorizationHostError> {
        let now = self
            .clock
            .unix_time_millis()
            .map_err(|_| unavailable_host())?;
        let mut state = self.state.lock().map_err(|_| unavailable_host())?;
        observe_clock(&mut state, now).map_err(|_| unavailable_host())
    }

    fn provenance(
        &self,
        authorization_id: &str,
    ) -> Result<AuthorizationProvenance, TrustedTaskAuthorizationHostError> {
        self.state
            .lock()
            .map_err(|_| unavailable_host())?
            .authorizations
            .get(authorization_id)
            .cloned()
            .ok_or(TrustedTaskAuthorizationHostError::Denied)
    }

    fn live_key(
        &self,
        provenance: &AuthorizationProvenance,
    ) -> Result<TrustedTaskAuthorizationStatus, TrustedTaskAuthorizationHostError> {
        match self
            .trust
            .lookup(&provenance.key.key_id)
            .map_err(|_| unavailable_host())?
        {
            Some(CrossClientTaskAuthorizationTrustState::Active(key)) if key == provenance.key => {
                Ok(TrustedTaskAuthorizationStatus::Active)
            }
            Some(CrossClientTaskAuthorizationTrustState::Revoked { .. }) => {
                Ok(TrustedTaskAuthorizationStatus::Revoked)
            }
            Some(CrossClientTaskAuthorizationTrustState::Active(_)) | None => {
                Err(unavailable_host())
            }
        }
    }

    fn live_target(
        &self,
        provenance: &AuthorizationProvenance,
        process_id: u32,
        window_handle: u64,
    ) -> Result<(), TrustedTaskAuthorizationHostError> {
        let observed = self
            .target_observer
            .process_creation_identity(process_id, window_handle)
            .map_err(|_| unavailable_host())?;
        if observed == provenance.process_creation_identity {
            Ok(())
        } else {
            Err(TrustedTaskAuthorizationHostError::Denied)
        }
    }
}

#[async_trait]
impl TrustedTaskAuthorizationHost for CrossClientAuthorizationHost {
    async fn authorize(
        &self,
        request: TrustedTaskAuthorizationRequest,
    ) -> Result<TrustedTaskAuthorizationLease, TrustedTaskAuthorizationHostError> {
        self.validate_clock()?;
        let provenance = self.provenance(&request.authorization_id)?;
        self.live_target(
            &provenance,
            request.target_process_id,
            request.target_window_handle,
        )?;
        if self.live_key(&provenance)? != TrustedTaskAuthorizationStatus::Active {
            return Err(TrustedTaskAuthorizationHostError::Denied);
        }
        self.inner.authorize(request).await
    }

    async fn validate(
        &self,
        request: TrustedTaskAuthorizationValidationRequest,
    ) -> Result<TrustedTaskAuthorizationValidationDecision, TrustedTaskAuthorizationHostError> {
        self.validate_clock()?;
        let provenance = self.provenance(&request.authorization_id)?;
        self.live_target(
            &provenance,
            request.target_process_id,
            request.target_window_handle,
        )?;
        let status = self.live_key(&provenance)?;
        if status == TrustedTaskAuthorizationStatus::Revoked {
            return Ok(TrustedTaskAuthorizationValidationDecision {
                status,
                request_digest: request.request_digest,
            });
        }
        self.inner.validate(request).await
    }

    async fn validate_lease(
        &self,
        request: TrustedTaskAuthorizationLeaseValidationRequest,
    ) -> Result<TrustedTaskAuthorizationValidationDecision, TrustedTaskAuthorizationHostError> {
        self.validate_clock()?;
        let provenance = self.provenance(&request.authorization_id)?;
        self.live_target(
            &provenance,
            request.target_process_id,
            request.target_window_handle,
        )?;
        let status = self.live_key(&provenance)?;
        if status == TrustedTaskAuthorizationStatus::Revoked {
            return Ok(TrustedTaskAuthorizationValidationDecision {
                status,
                request_digest: request.request_digest,
            });
        }
        self.inner.validate_lease(request).await
    }
}

fn unavailable_host() -> TrustedTaskAuthorizationHostError {
    TrustedTaskAuthorizationHostError::Unavailable {
        reason: "the constructor-provisioned cross-client trust state is unavailable".into(),
    }
}

#[derive(Clone)]
struct TrustRegistry {
    state: Arc<Mutex<BTreeMap<String, RegistryKey>>>,
}

#[derive(Clone)]
struct RegistryKey {
    key: TrustedTaskAuthorizationKey,
    revoked_at_epoch: Option<u64>,
}

pub struct TrustedTaskAuthorizationTrustAdmin {
    state: Arc<Mutex<BTreeMap<String, RegistryKey>>>,
}

impl TrustedTaskAuthorizationTrustAdmin {
    pub fn revoke(
        &self,
        key_id: &str,
        revocation_epoch: u64,
    ) -> Result<(), CrossClientTaskAuthorizationError> {
        if revocation_epoch == 0 || revocation_epoch > MAX_SAFE_JSON_INTEGER {
            return Err(CrossClientTaskAuthorizationError::InvalidConfiguration);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| CrossClientTaskAuthorizationError::TrustUnavailable)?;
        let record = state
            .get_mut(key_id)
            .ok_or(CrossClientTaskAuthorizationError::UnknownKey)?;
        if record
            .revoked_at_epoch
            .is_some_and(|previous| revocation_epoch <= previous)
        {
            return Err(CrossClientTaskAuthorizationError::InvalidConfiguration);
        }
        record.revoked_at_epoch = Some(revocation_epoch);
        Ok(())
    }
}

impl CrossClientTaskAuthorizationTrustStore for TrustRegistry {
    fn lookup(
        &self,
        key_id: &str,
    ) -> Result<
        Option<CrossClientTaskAuthorizationTrustState>,
        CrossClientTaskAuthorizationTrustError,
    > {
        let state = self
            .state
            .lock()
            .map_err(|_| CrossClientTaskAuthorizationTrustError)?;
        Ok(state.get(key_id).map(|record| {
            record.revoked_at_epoch.map_or_else(
                || CrossClientTaskAuthorizationTrustState::Active(record.key.clone()),
                |revocation_epoch| CrossClientTaskAuthorizationTrustState::Revoked {
                    revocation_epoch,
                },
            )
        }))
    }
}

pub fn trusted_task_authorization_trust_registry(
    keys: Vec<TrustedTaskAuthorizationKey>,
) -> Result<
    (
        TrustedTaskAuthorizationTrustAdmin,
        Arc<dyn CrossClientTaskAuthorizationTrustStore>,
    ),
    CrossClientTaskAuthorizationError,
> {
    if keys.is_empty() || keys.len() > 64 {
        return Err(CrossClientTaskAuthorizationError::InvalidConfiguration);
    }
    let mut records = BTreeMap::new();
    for key in keys {
        validate_identity(&key.key_id, MAX_IDENTITY_CHARS)
            .map_err(|_| CrossClientTaskAuthorizationError::InvalidConfiguration)?;
        validate_audience(&key.audience)
            .map_err(|_| CrossClientTaskAuthorizationError::InvalidConfiguration)?;
        VerifyingKey::from_bytes(&key.verifying_key)
            .map_err(|_| CrossClientTaskAuthorizationError::InvalidConfiguration)?;
        let key_id = key.key_id.clone();
        if records
            .insert(
                key_id,
                RegistryKey {
                    key,
                    revoked_at_epoch: None,
                },
            )
            .is_some()
        {
            return Err(CrossClientTaskAuthorizationError::InvalidConfiguration);
        }
    }
    let state = Arc::new(Mutex::new(records));
    Ok((
        TrustedTaskAuthorizationTrustAdmin {
            state: Arc::clone(&state),
        },
        Arc::new(TrustRegistry { state }),
    ))
}
