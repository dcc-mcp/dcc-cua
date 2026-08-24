use rstest::rstest;
use serde_json::{Value, json};

use super::*;

fn test_server() -> TaskAuthorizationServer {
    let embedding = crate::trusted_embedding::validate_codex_identity(
        "codex.exe",
        "OpenAI.Codex_2p2nqsd0c76g0",
    )
    .unwrap();
    TaskAuthorizationServer::new(embedding)
}

fn browser_task() -> Value {
    json!({
        "application_label": "Chrome Web Store credentials",
        "target_process_id": 42,
        "target_window_handle": 7,
        "surface": "browser",
        "allowed_methods": ["browser_snapshot", "browser_type"],
        "allowed_actions": [{
            "action": "browser_type",
            "input_kind": "browser",
            "secret_input": true,
            "authorization_category": "credential",
            "browser_origin": "https://chromewebstore.google.com"
        }],
        "ttl_minutes": 15
    })
}

fn owned_browser_task() -> Value {
    json!({
        "application_label": "Firefox add-on upload",
        "owned_browser_launch": {
            "browser": "chromium",
            "profile": "isolated_new"
        },
        "surface": "browser",
        "allowed_methods": ["browser_snapshot", "browser_navigate", "browser_set_input_files"],
        "allowed_actions": [{
            "action": "click",
            "input_kind": "semantic",
            "secret_input": false,
            "authorization_category": "publishing"
        }],
        "allowed_browser_origins": ["https://addons.mozilla.org"],
        "ttl_minutes": 15
    })
}

#[rstest]
fn issuer_tools_are_app_only_and_task_calls_cannot_mint_authority() {
    let tools = tool_definitions();
    let authorize = tools
        .iter()
        .find(|tool| tool["name"] == "authorize_task")
        .unwrap();
    let task_call = tools
        .iter()
        .find(|tool| tool["name"] == "dcc_cua_task_call")
        .unwrap();
    assert_eq!(authorize["_meta"]["ui"]["visibility"], json!(["app"]));
    assert!(
        task_call["inputSchema"]["properties"]
            .get("authorization_id")
            .is_none()
    );
    assert!(
        task_call["inputSchema"]["properties"]
            .get("window_capability")
            .is_none()
    );
}

#[rstest]
fn owned_browser_proposal_exposes_only_a_closed_launch_spec_until_start() {
    let mut server = test_server();
    let prepared = server.prepare_task(owned_browser_task()).unwrap();

    assert_eq!(prepared["target"]["kind"], "owned_browser");
    assert_eq!(prepared["target"]["browser"], "chromium");
    assert_eq!(prepared["target"]["profile"], "isolated_new");
    assert!(prepared["target"]["process_id"].is_null());
    assert!(prepared["target"]["window_handle"].is_null());

    let proposal_id = prepared["proposal_id"].as_str().unwrap();
    server
        .authorize_task(json!({
            "proposal_id": proposal_id,
            "acknowledgement": "授权"
        }))
        .unwrap();
    let proposal = server.proposals.get(proposal_id).unwrap();
    let grant = task_session_grant(proposal, proposal.receipt.as_ref().unwrap());
    assert!(grant["process_id"].is_null());
    assert!(grant["window_handle"].is_null());
    assert_eq!(grant["allow_browser_prepare"], false);
    assert_eq!(
        grant["allowed_browser_origins"],
        json!(["https://addons.mozilla.org"])
    );
}

#[rstest]
fn owned_browser_proposal_rejects_target_substitution_and_prepare_reentry() {
    let mut server = test_server();
    let mut substituted = owned_browser_task();
    substituted["target_process_id"] = json!(42);
    substituted["target_window_handle"] = json!(7);
    assert!(server.prepare_task(substituted).is_err());

    let mut reentry = owned_browser_task();
    reentry["allowed_methods"] = json!(["browser_prepare"]);
    assert!(
        server
            .prepare_task(reentry)
            .unwrap_err()
            .contains("cannot grant browser_prepare")
    );
}

#[rstest]
#[tokio::test]
async fn authorized_task_call_requires_start_attestation_before_observation() {
    let mut server = test_server();
    let prepared = server.prepare_task(browser_task()).unwrap();
    let proposal_id = prepared["proposal_id"].as_str().unwrap();
    server
        .authorize_task(json!({
            "proposal_id": proposal_id,
            "acknowledgement": "授权"
        }))
        .unwrap();

    let error = server
        .task_call(json!({
            "proposal_id": proposal_id,
            "method": "browser_snapshot",
            "params": {}
        }))
        .await
        .unwrap_err();

    assert!(error.contains("provider/runtime/PID/HWND"));
}

#[rstest]
fn explicit_card_text_authorizes_once_without_exposing_broker_receipts() {
    let mut server = test_server();
    let prepared = server.prepare_task(browser_task()).unwrap();
    let proposal_id = prepared["proposal_id"].as_str().unwrap();
    assert_eq!(prepared["status"], "awaiting_user_input");
    assert_eq!(
        prepared["allowed_methods"],
        json!(["browser_snapshot", "browser_type"])
    );
    assert!(
        server
            .authorize_task(json!({
                "proposal_id": proposal_id,
                "acknowledgement": "yes"
            }))
            .is_err()
    );

    let authorized = server
        .authorize_task(json!({
            "proposal_id": proposal_id,
            "acknowledgement": "授权"
        }))
        .unwrap();

    assert_eq!(authorized["status"], "authorized");
    assert!(authorized.get("authorization_id").is_none());
    assert!(authorized.get("window_capability").is_none());
}

#[rstest]
fn clipboard_capture_grants_read_and_clear_as_one_authorized_capability() {
    let mut server = test_server();
    let mut task = browser_task();
    task["allowed_methods"] = json!(["clipboard_capture_secret"]);
    task["allowed_actions"] = json!([{
        "action": "clipboard_capture_secret",
        "input_kind": "clipboard",
        "secret_input": true,
        "authorization_category": "credential"
    }]);
    let prepared = server.prepare_task(task).unwrap();
    let proposal_id = prepared["proposal_id"].as_str().unwrap();
    server
        .authorize_task(json!({
            "proposal_id": proposal_id,
            "acknowledgement": "授权"
        }))
        .unwrap();
    let proposal = server.proposals.get(proposal_id).unwrap();
    let receipt = proposal.receipt.as_ref().unwrap();

    let grant = task_session_grant(proposal, receipt);

    assert_eq!(grant["allow_clipboard_read"], true);
    assert_eq!(grant["allow_clipboard_write"], true);
}

#[rstest]
#[tokio::test]
async fn task_call_fails_closed_before_user_input_and_never_requests_a_popup() {
    let mut server = test_server();
    let prepared = server.prepare_task(browser_task()).unwrap();
    let proposal_id = prepared["proposal_id"].as_str().unwrap();

    let error = server
        .task_call(json!({
            "proposal_id": proposal_id,
            "method": "browser_snapshot",
            "params": {}
        }))
        .await
        .unwrap_err();

    assert!(error.contains("explicit user input"));
    assert_eq!(prepared["native_action_popups"], false);
}

#[rstest]
fn authorization_card_accepts_no_secret_fields() {
    let prepare = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "prepare_task_authorization")
        .unwrap();
    let properties = prepare["inputSchema"]["properties"].as_object().unwrap();
    for forbidden in ["text", "secret", "password", "token", "credential"] {
        assert!(!properties.contains_key(forbidden));
    }
    assert!(AUTHORIZATION_CARD_HTML.contains("type=\"text\""));
    assert!(!AUTHORIZATION_CARD_HTML.contains("type=\"password\""));
}

#[rstest]
fn task_action_schema_matches_the_closed_runtime_contract() {
    let schema = task_action_scope_schema();
    let variants = schema["oneOf"].as_array().unwrap();
    assert_eq!(variants.len(), 4);

    let variant = |input_kind: &str| {
        variants
            .iter()
            .find(|variant| {
                variant["properties"]["input_kind"]["const"].as_str() == Some(input_kind)
            })
            .unwrap()
    };
    let semantic = variant("semantic");
    assert_eq!(
        semantic["properties"]["action"]["enum"],
        json!(TrustedTaskActionScope::NATIVE_ACTIONS)
    );
    assert_eq!(
        semantic["properties"]["authorization_category"]["enum"],
        json!(TrustedTaskActionScope::SEMANTIC_CATEGORIES)
    );
    assert!(
        !semantic["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("browser_click"))
    );

    let browser = variant("browser");
    assert_eq!(browser["properties"]["action"]["const"], "browser_type");
    assert_eq!(browser["properties"]["secret_input"]["const"], true);
    assert_eq!(
        browser["properties"]["authorization_category"]["const"],
        "credential"
    );
    assert!(
        browser["required"]
            .as_array()
            .unwrap()
            .contains(&json!("browser_origin"))
    );

    let clipboard = variant("clipboard");
    assert_eq!(
        clipboard["properties"]["action"]["const"],
        "clipboard_capture_secret"
    );
    assert_eq!(clipboard["properties"]["secret_input"]["const"], true);

    let prepare = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "prepare_task_authorization")
        .unwrap();
    assert_eq!(
        prepare["inputSchema"]["properties"]["allowed_actions"]["uniqueItems"],
        true
    );
}

#[rstest]
fn method_style_action_name_is_rejected_before_a_proposal_is_created() {
    let mut server = test_server();
    let mut task = browser_task();
    task["allowed_actions"][0]["action"] = json!("browser_click");

    assert!(server.prepare_task(task).is_err());
    assert!(server.proposals.is_empty());
}

#[rstest]
#[tokio::test]
async fn task_call_scope_rejects_methods_not_shown_to_the_user() {
    let mut server = test_server();
    let prepared = server.prepare_task(browser_task()).unwrap();
    let proposal_id = prepared["proposal_id"].as_str().unwrap();
    server
        .authorize_task(json!({
            "proposal_id": proposal_id,
            "acknowledgement": "授权"
        }))
        .unwrap();
    let error = server
        .task_call(json!({
            "proposal_id": proposal_id,
            "method": "browser_navigate",
            "params": {}
        }))
        .await
        .unwrap_err();
    assert!(error.contains("user-authorized method scope"));
}

#[rstest]
fn task_proposal_rejects_surface_mismatches_and_duplicate_methods() {
    let mut server = test_server();
    let mut mismatch = browser_task();
    mismatch["surface"] = json!("window");
    assert!(
        server
            .prepare_task(mismatch)
            .unwrap_err()
            .contains("closed window")
    );

    let mut duplicate = browser_task();
    duplicate["allowed_methods"] = json!(["browser_snapshot", "browser_snapshot"]);
    assert!(
        server
            .prepare_task(duplicate)
            .unwrap_err()
            .contains("duplicates")
    );
}
