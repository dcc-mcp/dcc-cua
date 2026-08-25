use rstest::rstest;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use cua_driver_sdk::TrustedSessionOptions;
use cua_driver_sdk::remote::{
    DRIVER_ENVELOPE_VERSION, DriverChannelCapabilities, DriverEnvelopeChannel,
    DriverRequestEnvelope, DriverResponseEnvelope,
};
use dcc_cua_core::ComputerUseObservation;

use super::*;
use crate::action_confirmation::ConfirmationBinding;

fn raw_input_action(action: &str, intent: &str) -> HostAction {
    HostAction {
        action: action.into(),
        element_index: None,
        element_token: None,
        delivery_mode: Some("foreground".into()),
        input_backend_id: None,
        input_kind: "raw_input".into(),
        intent: intent.into(),
        x: Some(10.0),
        y: Some(10.0),
        button: Some("left".into()),
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: vec![
            ComputerUsePoint { x: 10.0, y: 10.0 },
            ComputerUsePoint { x: 20.0, y: 20.0 },
        ],
        text: None,
        secret_handle: None,
        delay_ms: None,
        type_chars_only: false,
        checked: None,
        keys: Vec::new(),
        modifiers: Vec::new(),
        duration_ms: Some(100),
        steps: Some(4),
    }
}

fn keyboard_action(action: &str, intent: &str, keys: &[&str], modifiers: &[&str]) -> HostAction {
    let mut action = raw_input_action(action, intent);
    action.x = None;
    action.y = None;
    action.button = None;
    action.path.clear();
    action.duration_ms = None;
    action.steps = None;
    action.keys = keys.iter().map(|key| (*key).to_owned()).collect();
    action.modifiers = modifiers
        .iter()
        .map(|modifier| (*modifier).to_owned())
        .collect();
    action
}

#[derive(Clone, Default)]
struct RecordingInputChannel {
    tool_names: Arc<Mutex<Vec<String>>>,
}

impl DriverEnvelopeChannel for RecordingInputChannel {
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
            self.tool_names
                .lock()
                .expect("record synthetic remote input call")
                .push(request.name.clone().unwrap_or_default());
            Ok(DriverResponseEnvelope {
                envelope_version: DRIVER_ENVELOPE_VERSION,
                request_id: request.request_id,
                ok: true,
                result: Some(json!({
                    "content": [{"type": "text", "text": "ok"}],
                    "structuredContent": {"success": true},
                    "isError": false
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
        "host-action-policy-test"
    }

    fn connection_generation(&self) -> &str {
        "generation-1"
    }
}

fn synthetic_confirmation_session(
    allow_raw_input: bool,
) -> (ComputerUseDriver, ConnectionSessions, RecordingInputChannel) {
    let channel = RecordingInputChannel::default();
    let driver = ComputerUseDriver::from_test_remote_channel(Arc::new(channel.clone())).unwrap();
    let mut host = cached_host_session(&driver);
    host.allow_raw_input = allow_raw_input;
    host.allow_trusted_confirmation = true;
    host.latest_accessibility_root = Some(json!({
        "elements": [{
            "element_index": 7,
            "element_token": "task-granted-control",
            "policy_tier": "task_grant",
            "policy_category": "content_change"
        }]
    }));
    host.session.seed_test_observation(ComputerUseObservation {
        observation_id: "observation-before-transition".into(),
        window_handle: 77,
        process_id: 42,
        window_title: "Synthetic test target".into(),
        width: 800,
        height: 600,
        source_rect: [0, 0, 800, 600],
        capture_backend: "synthetic-test".into(),
        capture_provenance: json!({"source": "test-only"}),
        session_id: "runtime-session-1".into(),
    });
    let mut sessions = ConnectionSessions::default();
    sessions.windows.insert("session-1".into(), host);
    (driver, sessions, channel)
}

fn semantic_keyboard_request(
    action: &str,
    intent: &str,
    keys: &[&str],
    modifiers: &[&str],
    input_backend_id: Option<&str>,
) -> Request {
    serde_json::from_value(json!({
        "method": "execute_action",
        "params": {
            "session_id": "session-1",
            "task_grant_id": "grant-1",
            "window_capability": "capability-1",
            "observation_id": "observation-before-transition",
            "accessibility_state_id": "accessibility-before-transition",
            "action": {
                "action": action,
                "input_kind": "semantic",
                "intent": intent,
                "element_index": 7,
                "element_token": "task-granted-control",
                "delivery_mode": "foreground",
                "keys": keys,
                "modifiers": modifiers,
                "input_backend_id": input_backend_id
            }
        }
    }))
    .unwrap()
}

fn assert_no_input_calls(channel: &RecordingInputChannel) {
    assert!(
        channel
            .tool_names
            .lock()
            .expect("read synthetic remote calls")
            .is_empty(),
        "a refused request must not reach the driver session"
    );
}

async fn execute_with_default_security(
    driver: &ComputerUseDriver,
    sessions: &mut ConnectionSessions,
    request: Request,
) -> Result<Value, HostError> {
    let mut snapshot_transport = Some(SnapshotTransport::BinaryFrame);
    let mut desktop_shared_image = None;
    let cancellation_registry = Arc::new(Mutex::new(HashMap::new()));
    handle_request(
        driver,
        sessions,
        &mut snapshot_transport,
        &mut desktop_shared_image,
        &cancellation_registry,
        request,
    )
    .await
    .map(|(response, _)| response)
}

#[rstest]
#[tokio::test]
async fn semantic_keyboard_shapes_cannot_borrow_a_task_granted_element() {
    let cases: &[(&str, &[&str], &[&str], &str)] = &[
        ("hotkey", &["F4"], &["ALT"], "ordinary_edit"),
        ("hotkey", &["W"], &["CTRL"], "navigate"),
        ("hotkey", &["W"], &["CONTROL"], "ordinary_edit"),
        ("hotkey", &["W"], &["CMD"], "navigate"),
        ("hotkey", &["Q"], &["COMMAND"], "ordinary_edit"),
        ("hotkey", &["Q"], &["META"], "navigate"),
        ("keypress", &["DELETE"], &[], "ordinary_edit"),
        ("press_key", &["F4"], &[], "navigate"),
        ("keypress", &["PRINTSCREEN"], &[], "ordinary_edit"),
        ("click", &["W"], &["CTRL"], "ordinary_edit"),
        ("toggle", &[], &["ALT"], "navigate"),
    ];
    for &(action, keys, modifiers, intent) in cases {
        let (driver, mut sessions, channel) = synthetic_confirmation_session(false);
        let response = execute_with_default_security(
            &driver,
            &mut sessions,
            semantic_keyboard_request(action, intent, keys, modifiers, None),
        )
        .await
        .unwrap();

        assert_eq!(response["success"], false, "action={action}");
        assert_eq!(
            response["policy_tier"], "action_confirmation",
            "action={action}"
        );
        assert_eq!(response["error"], "approval_required", "action={action}");
        assert_no_input_calls(&channel);
    }
}

#[rstest]
#[tokio::test]
async fn backend_selector_is_outside_the_task_granted_keyboard_envelope() {
    let (driver, mut sessions, channel) = synthetic_confirmation_session(true);
    let request = serde_json::from_value(json!({
        "method": "execute_action",
        "params": {
            "session_id": "session-1",
            "task_grant_id": "grant-1",
            "window_capability": "capability-1",
            "observation_id": "observation-before-transition",
            "accessibility_state_id": "accessibility-before-transition",
            "action": {
                "action": "keypress",
                "input_kind": "raw_input",
                "intent": "ordinary_edit",
                "delivery_mode": "foreground",
                "keys": ["W"],
                "input_backend_id": "windows.send_input.v1"
            }
        }
    }))
    .unwrap();
    let response = execute_with_default_security(&driver, &mut sessions, request)
        .await
        .unwrap();

    assert_eq!(response["success"], false);
    assert_eq!(response["policy_tier"], "action_confirmation");
    assert_eq!(response["error"], "approval_required");
    assert_no_input_calls(&channel);
}

#[rstest]
#[tokio::test]
async fn approved_semantic_keyboard_shape_still_requires_the_raw_input_grant() {
    let (driver, mut sessions, channel) = synthetic_confirmation_session(false);
    let mut snapshot_transport = Some(SnapshotTransport::BinaryFrame);
    let mut desktop_shared_image = None;
    let cancellation_registry = Arc::new(Mutex::new(HashMap::new()));
    let security_services =
        HostSecurityServices::default().with_confirmation_host(Arc::new(EchoingConfirmationHost));
    let error = handle_request_with_security_services(
        &driver,
        &security_services,
        &mut sessions,
        &mut snapshot_transport,
        &mut desktop_shared_image,
        &cancellation_registry,
        semantic_keyboard_request("hotkey", "ordinary_edit", &["F4"], &["ALT"], None),
    )
    .await
    .unwrap_err();

    assert_eq!(error_code(&error), "raw_input_not_granted");
    assert_no_input_calls(&channel);
}

#[rstest]
fn legal_semantic_non_keyboard_action_keeps_its_element_policy() {
    let action = confirmation_action();
    let root = json!({
        "elements": [{
            "element_index": 7,
            "element_token": "submit-button",
            "policy_tier": "task_grant",
            "policy_category": "content_change"
        }]
    });

    assert_eq!(
        action.safety_tier(Some(&root)),
        HostActionSafetyTier::TaskGrant
    );
}

#[rstest]
#[case("click")]
#[case("double_click")]
#[case("right_click")]
#[case("toggle")]
#[case("drag")]
fn exact_window_navigation_pointer_actions_use_the_existing_task_grant(#[case] action: &str) {
    let action = raw_input_action(action, "navigate");

    assert_eq!(action.safety_tier(None), HostActionSafetyTier::TaskGrant);
}

#[rstest]
#[case("click")]
#[case("double_click")]
#[case("right_click")]
#[case("toggle")]
#[case("drag")]
fn exact_window_ordinary_pointer_actions_use_the_existing_task_grant(#[case] action: &str) {
    let action = raw_input_action(action, "ordinary_edit");

    assert_eq!(action.safety_tier(None), HostActionSafetyTier::TaskGrant);
}

#[rstest]
fn exact_window_safe_keyboard_actions_use_the_existing_task_grant() {
    for (action_name, key) in [
        ("keypress", "F"),
        ("press", "ENTER"),
        ("press", "SPACE"),
        ("press", "TAB"),
        ("press", "ESC"),
        ("press_key", "LEFT"),
        ("press_key", "PAGEUP"),
        ("keyboard_shortcut", "S"),
        ("hotkey", "W"),
    ] {
        let action = keyboard_action(action_name, "ordinary_edit", &[key], &[]);
        assert_eq!(
            action.safety_tier(None),
            HostActionSafetyTier::TaskGrant,
            "{action_name} with {:?} must stay inside the closed safe-key envelope",
            action.keys
        );
    }
}

#[rstest]
#[case("keypress", &["DELETE"], &[], "navigate")]
#[case("press", &["BACKSPACE"], &[], "ordinary_edit")]
#[case("press_key", &["F4"], &[], "navigate")]
#[case("keypress", &["F12"], &[], "ordinary_edit")]
#[case("keyboard_shortcut", &["W"], &["CONTROL"], "navigate")]
#[case("hotkey", &["F4"], &["ALT"], "ordinary_edit")]
#[case("hotkey", &["W"], &["CTRL"], "ordinary_edit")]
#[case("hotkey", &["Q"], &["COMMAND"], "navigate")]
#[case("hotkey", &["Q"], &["META"], "ordinary_edit")]
#[case("hotkey", &["CTRL", "W"], &[], "navigate")]
#[case("hotkey", &["CMD", "Q"], &[], "ordinary_edit")]
fn dangerous_keyboard_chords_ignore_hostile_intent_relabeling(
    #[case] action_name: &str,
    #[case] keys: &[&str],
    #[case] modifiers: &[&str],
    #[case] intent: &str,
) {
    let action = keyboard_action(action_name, intent, keys, modifiers);

    assert_eq!(
        action.safety_tier(None),
        HostActionSafetyTier::ActionConfirmation,
        "{action_name} with keys={keys:?}, modifiers={modifiers:?}, intent={intent} must require confirmation"
    );
}

#[rstest]
#[case(&[])]
#[case(&["W", "Q"])]
#[case(&["LAUNCH_MAIL"])]
#[case(&["PRINTSCREEN"])]
#[case(&["F24"])]
#[case(&[" "])]
fn unknown_or_malformed_keyboard_input_fails_closed(#[case] keys: &[&str]) {
    for intent in ["navigate", "ordinary_edit"] {
        let action = keyboard_action("keypress", intent, keys, &[]);
        assert_eq!(
            action.safety_tier(None),
            HostActionSafetyTier::ActionConfirmation,
            "keys={keys:?}, intent={intent} must not escape the confirmation boundary"
        );
    }
}

#[rstest]
fn bounded_unmodified_movement_keypress_stays_inside_the_task_grant() {
    let mut action = keyboard_action("keypress", "navigate", &["W", "D"], &[]);
    action.duration_ms = Some(1_000);

    assert_eq!(action.safety_tier(None), HostActionSafetyTier::TaskGrant);
}

#[rstest]
#[case(&["W"], 0)]
#[case(&["W"], 10_001)]
#[case(&["W", " w "], 1_000)]
#[case(&["W", "Q"], 1_000)]
fn malformed_held_keyboard_input_fails_closed(#[case] keys: &[&str], #[case] duration_ms: u64) {
    let mut action = keyboard_action("keypress", "navigate", keys, &[]);
    action.duration_ms = Some(duration_ms);

    assert_eq!(
        action.safety_tier(None),
        HostActionSafetyTier::ActionConfirmation
    );
}

#[rstest]
fn backend_selector_exits_both_safe_key_envelopes() {
    let mut tap = keyboard_action("keypress", "ordinary_edit", &["W"], &[]);
    tap.input_backend_id = Some("windows.send_input.v1".into());
    let mut held = keyboard_action("keypress", "navigate", &["W"], &[]);
    held.duration_ms = Some(1_000);
    held.input_backend_id = Some("windows.send_input.v1".into());

    assert_eq!(
        tap.safety_tier(None),
        HostActionSafetyTier::ActionConfirmation
    );
    assert_eq!(
        held.safety_tier(None),
        HostActionSafetyTier::ActionConfirmation
    );
}

#[rstest]
#[tokio::test]
async fn hostile_hotkey_relabel_reaches_the_real_confirmation_boundary() {
    let action = keyboard_action("hotkey", "ordinary_edit", &["F4"], &["ALT"]);
    let tier = action.safety_tier(None);
    assert_eq!(tier, HostActionSafetyTier::ActionConfirmation);
    assert!(tier.requires_confirmation());

    let request = window_confirmation(
        "session-1",
        "grant-1",
        "capability-1",
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 7,
        },
        "observation-1",
        "accessibility-1",
        &action,
    );
    let outcome = authorize_action_confirmation(None, true, request).await;
    assert_eq!(outcome, ActionConfirmationOutcome::Required);

    let response = crate::request_contract::action_confirmation_refusal(outcome).0;
    assert_eq!(response["success"], false);
    assert_eq!(response["policy_tier"], "action_confirmation");
    assert_eq!(response["error"], "approval_required");
}

#[rstest]
fn background_keyboard_input_still_requires_action_confirmation() {
    let mut action = raw_input_action("hotkey", "ordinary_edit");
    action.x = None;
    action.y = None;
    action.button = None;
    action.path.clear();
    action.keys = vec!["F4".into()];
    action.modifiers = vec!["ALT".into()];
    action.delivery_mode = Some("background".into());

    assert_eq!(
        action.safety_tier(None),
        HostActionSafetyTier::ActionConfirmation
    );
}

#[rstest]
fn keyboard_input_with_secret_still_requires_action_confirmation() {
    let mut action = raw_input_action("press", "ordinary_edit");
    action.x = None;
    action.y = None;
    action.button = None;
    action.path.clear();
    action.keys = vec!["F".into()];
    action.secret_handle = Some("secret-1".into());

    assert_eq!(
        action.safety_tier(None),
        HostActionSafetyTier::ActionConfirmation
    );
}

#[rstest]
fn background_ordinary_pointer_input_still_requires_action_confirmation() {
    let mut action = raw_input_action("drag", "ordinary_edit");
    action.delivery_mode = Some("background".into());

    assert_eq!(
        action.safety_tier(None),
        HostActionSafetyTier::ActionConfirmation
    );
}

#[rstest]
fn modified_ordinary_pointer_input_still_requires_action_confirmation() {
    let mut action = raw_input_action("click", "ordinary_edit");
    action.modifiers = vec!["SHIFT".into()];

    assert_eq!(
        action.safety_tier(None),
        HostActionSafetyTier::ActionConfirmation
    );
}

#[rstest]
fn ordinary_pointer_input_with_a_secret_still_requires_action_confirmation() {
    let mut action = raw_input_action("click", "ordinary_edit");
    action.secret_handle = Some("secret-1".into());

    assert_eq!(
        action.safety_tier(None),
        HostActionSafetyTier::ActionConfirmation
    );
}

#[rstest]
fn background_navigation_input_still_requires_action_confirmation() {
    let mut action = raw_input_action("drag", "navigate");
    action.delivery_mode = Some("background".into());

    assert_eq!(
        action.safety_tier(None),
        HostActionSafetyTier::ActionConfirmation
    );
}

#[rstest]
fn incomplete_navigation_pointer_shape_still_requires_action_confirmation() {
    let mut action = raw_input_action("click", "navigate");
    action.x = None;

    assert_eq!(
        action.safety_tier(None),
        HostActionSafetyTier::ActionConfirmation
    );
}

#[rstest]
fn modified_navigation_pointer_input_still_requires_action_confirmation() {
    let mut action = raw_input_action("click", "navigate");
    action.modifiers = vec!["SHIFT".into()];

    assert_eq!(
        action.safety_tier(None),
        HostActionSafetyTier::ActionConfirmation
    );
}

#[rstest]
fn navigation_pointer_input_with_a_secret_still_requires_action_confirmation() {
    let mut action = raw_input_action("click", "navigate");
    action.secret_handle = Some("secret-1".into());

    assert_eq!(
        action.safety_tier(None),
        HostActionSafetyTier::ActionConfirmation
    );
}

#[rstest]
#[case("keypress")]
#[case("press")]
#[case("press_key")]
#[case("keyboard_shortcut")]
#[case("hotkey")]
fn exact_window_navigation_keyboard_input_uses_the_existing_task_grant(#[case] action: &str) {
    let action = keyboard_action(action, "navigate", &["W"], &[]);

    assert_eq!(action.safety_tier(None), HostActionSafetyTier::TaskGrant);
}

#[rstest]
#[case("type")]
#[case("type_chars")]
#[case("set_text")]
#[case("set_value")]
fn navigation_text_actions_still_require_action_confirmation(#[case] action: &str) {
    let mut action = raw_input_action(action, "navigate");
    action.text = Some("redacted".into());

    assert_eq!(
        action.safety_tier(None),
        HostActionSafetyTier::ActionConfirmation
    );
}

#[rstest]
#[case("windows_security_or_privacy")]
#[case("human_verification")]
fn trusted_confirmation_intents_require_the_explicit_grant(#[case] intent: &str) {
    let action = raw_input_action("click", intent);
    let tier = action.safety_tier(None);
    assert!(tier.rejection().is_none());
    assert!(tier.requires_confirmation());
}

struct EchoingConfirmationHost;

#[async_trait::async_trait]
impl TrustedActionConfirmationHost for EchoingConfirmationHost {
    async fn confirm(
        &self,
        request: TrustedActionConfirmationRequest,
    ) -> Result<TrustedActionConfirmationDecision, TrustedActionConfirmationHostError> {
        Ok(TrustedActionConfirmationDecision {
            action: TrustedActionConfirmationAction::Allow,
            request_digest: request.request_digest,
        })
    }
}

struct ReplayingConfirmationHost {
    request_digest: String,
}

struct UnexpectedConfirmationHost;

#[async_trait::async_trait]
impl TrustedActionConfirmationHost for UnexpectedConfirmationHost {
    async fn confirm(
        &self,
        _request: TrustedActionConfirmationRequest,
    ) -> Result<TrustedActionConfirmationDecision, TrustedActionConfirmationHostError> {
        panic!("the constructor-owned host must not run without the task-grant gate")
    }
}

#[async_trait::async_trait]
impl TrustedActionConfirmationHost for ReplayingConfirmationHost {
    async fn confirm(
        &self,
        _request: TrustedActionConfirmationRequest,
    ) -> Result<TrustedActionConfirmationDecision, TrustedActionConfirmationHostError> {
        Ok(TrustedActionConfirmationDecision {
            action: TrustedActionConfirmationAction::Allow,
            request_digest: self.request_digest.clone(),
        })
    }
}

fn confirmation_action() -> HostAction {
    HostAction {
        action: "click".into(),
        element_index: Some(7),
        element_token: Some("submit-button".into()),
        delivery_mode: Some("foreground".into()),
        input_backend_id: None,
        input_kind: "semantic".into(),
        intent: "confirm".into(),
        x: None,
        y: None,
        button: Some("left".into()),
        scroll_x: None,
        scroll_y: None,
        scroll_by: None,
        path: Vec::new(),
        text: None,
        secret_handle: None,
        delay_ms: None,
        type_chars_only: false,
        checked: None,
        keys: Vec::new(),
        modifiers: Vec::new(),
        duration_ms: None,
        steps: None,
    }
}

fn window_confirmation(
    session_id: &str,
    task_grant_id: &str,
    window_capability: &str,
    target: ConfirmationWindowIdentity,
    observation_id: &str,
    accessibility_state_id: &str,
    action: &HostAction,
) -> TrustedActionConfirmationRequest {
    TrustedActionConfirmationRequest::for_bound_window_action_value(
        ConfirmationBinding::window(
            session_id,
            task_grant_id,
            window_capability,
            target,
            observation_id,
            Some(accessibility_state_id),
        ),
        &action.intent,
        serde_json::to_value(action).unwrap(),
    )
    .unwrap()
}

#[rstest]
fn trusted_confirmation_request_exposes_the_exact_window_identity() {
    let request = window_confirmation(
        "session-1",
        "grant-1",
        "capability-1",
        ConfirmationWindowIdentity {
            process_id: 4242,
            window_handle: 0x1234,
        },
        "observation-1",
        "accessibility-1",
        &confirmation_action(),
    );

    assert_eq!(request.target_process_id, Some(4242));
    assert_eq!(request.target_window_handle, Some(0x1234));
}

#[rstest]
#[tokio::test]
async fn trusted_confirmation_requires_a_constructor_owned_host() {
    let outcome = authorize_action_confirmation(
        None,
        true,
        window_confirmation(
            "session-1",
            "grant-1",
            "capability-1",
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
            "observation-1",
            "accessibility-1",
            &confirmation_action(),
        ),
    )
    .await;

    assert_eq!(outcome, ActionConfirmationOutcome::Required);
}

#[rstest]
#[tokio::test]
async fn trusted_confirmation_task_grant_gate_cannot_be_bypassed_by_the_host() {
    let host: Arc<dyn TrustedActionConfirmationHost> = Arc::new(UnexpectedConfirmationHost);
    let outcome = authorize_action_confirmation(
        Some(host.as_ref()),
        false,
        window_confirmation(
            "session-1",
            "grant-1",
            "capability-1",
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
            "observation-1",
            "accessibility-1",
            &confirmation_action(),
        ),
    )
    .await;

    assert_eq!(outcome, ActionConfirmationOutcome::Required);
}

#[rstest]
#[tokio::test]
async fn trusted_confirmation_accepts_an_exact_action_bound_decision() {
    let host: Arc<dyn TrustedActionConfirmationHost> = Arc::new(EchoingConfirmationHost);
    let outcome = authorize_action_confirmation(
        Some(host.as_ref()),
        true,
        window_confirmation(
            "session-1",
            "grant-1",
            "capability-1",
            ConfirmationWindowIdentity {
                process_id: 42,
                window_handle: 7,
            },
            "observation-1",
            "accessibility-1",
            &confirmation_action(),
        ),
    )
    .await;

    assert_eq!(outcome, ActionConfirmationOutcome::Allowed);
}

#[rstest]
#[tokio::test]
async fn trusted_confirmation_rejects_a_replayed_decision_for_new_evidence() {
    let first = window_confirmation(
        "session-1",
        "grant-1",
        "capability-1",
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 7,
        },
        "observation-1",
        "accessibility-1",
        &confirmation_action(),
    );
    let host: Arc<dyn TrustedActionConfirmationHost> = Arc::new(ReplayingConfirmationHost {
        request_digest: first.request_digest,
    });
    let second = window_confirmation(
        "session-1",
        "grant-1",
        "capability-1",
        ConfirmationWindowIdentity {
            process_id: 42,
            window_handle: 7,
        },
        "observation-2",
        "accessibility-2",
        &confirmation_action(),
    );

    let outcome = authorize_action_confirmation(Some(host.as_ref()), true, second).await;

    assert_eq!(outcome, ActionConfirmationOutcome::Required);
}
