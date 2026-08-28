use rstest::rstest;
use serde_json::{Value, json};

use super::*;

fn test_server() -> TaskAuthorizationServer {
    let embedding = crate::trusted_embedding::validate_codex_identity(
        "codex.exe",
        "OpenAI.Codex_2p2nqsd0c76g0",
    )
    .unwrap();
    // Only this separated test fixture can construct model-facing issuer
    // authority. Production process attestation is diagnostic, never consent.
    let (issuer, authorization_host) = dcc_cua_host::trusted_task_authorization_broker();
    TaskAuthorizationServer {
        authority: Some(TaskAuthorizationAuthority {
            embedding,
            issuer,
            authorization_host,
        }),
        parent_identity_available: true,
        proposals: BTreeMap::new(),
    }
}

#[rstest]
#[tokio::test]
async fn process_attestation_never_constructs_human_authority() {
    for parent_attested in [true, false] {
        let mut server = TaskAuthorizationServer::diagnostic_only(parent_attested);
        assert!(server.authority.is_none());
        for method in [
            "prepare_task_authorization",
            "authorize_task",
            "start_authorized_task",
            "dcc_cua_task_call",
        ] {
            let result = server
                .call_tool(json!({"name": method, "arguments": browser_task()}))
                .await
                .unwrap();
            assert_eq!(result["structuredContent"]["code"], "integration_required");
            assert_eq!(result["isError"], true);
        }
        assert_eq!(
            server.prepare_task(browser_task()).unwrap_err(),
            "integration_required"
        );
        assert_eq!(
            server.authorize_task(json!({})).unwrap_err(),
            "integration_required"
        );
        assert_eq!(
            server.start_task(json!({})).await.unwrap_err(),
            "integration_required"
        );
        assert!(server.proposals.is_empty());
        let resource = server
            .handle_rpc(json!({"jsonrpc":"2.0","id":1,"method":"resources/read",
        "params":{"uri":AUTHORIZATION_CARD_URI}}))
            .await
            .unwrap();
        assert_eq!(resource["error"]["message"], "integration_required");
    }
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

fn driver_authorization_request(public_session: &str) -> DriverAuthorizationRequest {
    DriverAuthorizationRequest {
        schema: "cua-driver-authorization-request-v1".into(),
        nonce: "nonce-1".into(),
        generation: 1,
        daemon_instance: "daemon-1".into(),
        permission_mode: "standard".into(),
        managed_policy_sha256: None,
        user_policy_sha256: None,
        adapter_id: "browser_prepare.existing_profile".into(),
        risk_class: "r2".into(),
        public_session: public_session.into(),
        transport_session: "transport-1".into(),
        resource_json: "{}".into(),
        human_summary: "Attach to the authorized browser profile".into(),
        expires_unix_ms: u64::MAX,
        request_digest: "digest-1".into(),
    }
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
fn browser_prepare_is_granted_only_when_the_authorization_card_includes_the_method() {
    let mut server = test_server();
    let prepared = server.prepare_task(browser_task()).unwrap();
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

    assert_eq!(grant["allow_browser_prepare"], false);
}

#[rstest]
#[tokio::test]
async fn authorized_browser_prepare_accepts_the_exact_logical_task_session() {
    let mut server = test_server();
    let mut task = browser_task();
    task["allowed_methods"] = json!(["browser_snapshot", "browser_prepare"]);
    let prepared = server.prepare_task(task).unwrap();
    let proposal_id = prepared["proposal_id"].as_str().unwrap();
    server
        .authorize_task(json!({
            "proposal_id": proposal_id,
            "acknowledgement": "授权"
        }))
        .unwrap();
    let proposal = server.proposals.get(proposal_id).unwrap();
    let host = browser_prepare_authorization_host(proposal_id, proposal).unwrap();
    let public_session = task_session_id(proposal_id);

    let decision = host
        .authorize(driver_authorization_request(&public_session))
        .await
        .unwrap();

    assert_eq!(decision.action, DriverAuthorizationAction::Allow);
    assert_eq!(decision.request_digest, "digest-1");
}

#[rstest]
#[tokio::test]
async fn authorized_browser_prepare_accepts_the_exact_namespaced_logical_task_session() {
    let host = TaskBrowserPrepareAuthorizationHost::new("mcp-task-exact");
    let namespaced = "__cua_runtime_00112233445566778899aabbccddeeff:mcp-task-exact";

    let decision = host
        .authorize(driver_authorization_request(namespaced))
        .await
        .unwrap();

    assert_eq!(decision.action, DriverAuthorizationAction::Allow);
    assert_eq!(decision.request_digest, "digest-1");
}

#[rstest]
#[tokio::test]
async fn authorized_browser_prepare_accepts_and_binds_the_real_runtime_window_session() {
    let host = TaskBrowserPrepareAuthorizationHost::new("mcp-task-exact");
    let runtime_session = concat!(
        "__cua_runtime_00112233445566778899aabbccddeeff:",
        "dcc-cua-window-123e4567-e89b-42d3-a456-426614174000"
    );
    let mut first = driver_authorization_request(runtime_session);
    first.transport_session = "transport-exact".into();

    let decision = host.authorize(first).await.unwrap();

    assert_eq!(decision.action, DriverAuthorizationAction::Allow);
    let mut repeated = driver_authorization_request(runtime_session);
    repeated.transport_session = "transport-exact".into();
    assert_eq!(
        host.authorize(repeated).await.unwrap().action,
        DriverAuthorizationAction::Allow
    );

    let mut different_public = driver_authorization_request(concat!(
        "__cua_runtime_00112233445566778899aabbccddeeff:",
        "dcc-cua-window-223e4567-e89b-42d3-a456-426614174000"
    ));
    different_public.transport_session = "transport-exact".into();
    assert_eq!(
        host.authorize(different_public).await.unwrap().action,
        DriverAuthorizationAction::Deny
    );

    let mut different_transport = driver_authorization_request(runtime_session);
    different_transport.transport_session = "transport-other".into();
    assert_eq!(
        host.authorize(different_transport).await.unwrap().action,
        DriverAuthorizationAction::Deny
    );
}

#[rstest]
#[tokio::test]
async fn authorized_browser_prepare_rejects_a_different_logical_task_session() {
    let host = TaskBrowserPrepareAuthorizationHost::new("mcp-task-exact");

    let decision = host
        .authorize(driver_authorization_request("mcp-task-other"))
        .await
        .unwrap();

    assert_eq!(decision.action, DriverAuthorizationAction::Deny);
    assert_eq!(decision.request_digest, "digest-1");
}

#[rstest]
#[tokio::test]
async fn authorized_browser_prepare_rejects_namespaced_session_lookalikes() {
    let host = TaskBrowserPrepareAuthorizationHost::new("mcp-task-exact");
    for observed_session in [
        "__cua_runtime_00112233445566778899aabbccddee:mcp-task-exact",
        "__cua_runtime_00112233445566778899AABBCCDDEEFF:mcp-task-exact",
        "__cua_runtime_00112233445566778899aabbccddeeff:mcp-task-other",
        "__cua_runtime_00112233445566778899aabbccddeeff:other:mcp-task-exact",
        "__cua_runtime_00112233445566778899aabbccddeeff:dcc-cua-window-not-a-uuid",
        "__cua_runtime_00112233445566778899aabbccddeeff:dcc-cua-window-123e4567-e89b-12d3-a456-426614174000",
        "dcc-cua-window-123e4567-e89b-42d3-a456-426614174000",
        "prefix:mcp-task-exact",
    ] {
        let decision = host
            .authorize(driver_authorization_request(observed_session))
            .await
            .unwrap();

        assert_eq!(decision.action, DriverAuthorizationAction::Deny);
        assert_eq!(decision.request_digest, "digest-1");
    }
}

#[rstest]
#[tokio::test]
async fn authorized_browser_prepare_rejects_every_non_exact_driver_request() {
    let host = TaskBrowserPrepareAuthorizationHost::new("mcp-task-exact");
    let mut requests = Vec::new();
    let mut wrong_schema = driver_authorization_request("mcp-task-exact");
    wrong_schema.schema = "cua-driver-authorization-request-v2".into();
    requests.push(wrong_schema);
    let mut wrong_mode = driver_authorization_request("mcp-task-exact");
    wrong_mode.permission_mode = "unrestricted".into();
    requests.push(wrong_mode);
    let mut wrong_adapter = driver_authorization_request("mcp-task-exact");
    wrong_adapter.adapter_id = "browser_prepare.isolated_profile".into();
    requests.push(wrong_adapter);
    let mut wrong_risk = driver_authorization_request("mcp-task-exact");
    wrong_risk.risk_class = "r1".into();
    requests.push(wrong_risk);

    for request in requests {
        let decision = host.authorize(request).await.unwrap();
        assert_eq!(decision.action, DriverAuthorizationAction::Deny);
        assert_eq!(decision.request_digest, "digest-1");
    }
}

#[rstest]
fn browser_prepare_driver_authorization_requires_the_card_method_and_user_receipt() {
    let mut server = test_server();
    let mut task = browser_task();
    task["allowed_methods"] = json!(["browser_snapshot", "browser_prepare"]);
    let prepared = server.prepare_task(task).unwrap();
    let proposal_id = prepared["proposal_id"].as_str().unwrap();
    let awaiting_input = server.proposals.get(proposal_id).unwrap();
    assert!(browser_prepare_authorization_host(proposal_id, awaiting_input).is_none());

    let prepared = server.prepare_task(browser_task()).unwrap();
    let proposal_id = prepared["proposal_id"].as_str().unwrap();
    server
        .authorize_task(json!({
            "proposal_id": proposal_id,
            "acknowledgement": "授权"
        }))
        .unwrap();
    let method_omitted = server.proposals.get(proposal_id).unwrap();
    assert!(browser_prepare_authorization_host(proposal_id, method_omitted).is_none());
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
