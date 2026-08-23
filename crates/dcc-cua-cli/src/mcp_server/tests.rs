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
