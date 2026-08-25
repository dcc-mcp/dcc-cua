use rstest::rstest;

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
