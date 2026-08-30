//! DCC-CUA owns bounded grant enforcement; the connected agent host owns user approval.

use serde_json::{Value, json};

pub(crate) fn status() -> Value {
    json!({
        "schema": "dcc-cua.authorization-integration.v2",
        "provider": "dcc-cua",
        "runtime_version": env!("CARGO_PKG_VERSION"),
        "status": "available",
        "authorization_available": true,
        "user_confirmation_available": true,
        "card_available": true,
        "confirmation_method": "client_managed",
        "confirmation_owner": "agent_host",
        "requires_system_user_verification": false,
        "client_must_enforce_user_approval": true,
        "trust_boundary": "mcp_connection_owner",
        "process_identity_can_authorize": false,
        "reason": "authorization_is_delegated_to_the_connected_agent_host",
        "next_owners": [],
        "next_action": "Prepare one exact bounded proposal, obtain approval through the connected agent host, then call authorize_task with only the retained proposal_id.",
        "contract": "https://github.com/dcc-mcp/dcc-cua/blob/main/docs/adr/0028-delegate-task-authorization-to-agent-hosts.md",
        "signed_receipt_protocol": {
            "status": "implemented_core",
            "constructor_api_available": true,
            "runtime_accepts_receipts": false
        },
        "fallback": "none"
    })
}

pub(crate) fn tool() -> Value {
    json!({
        "name": "authorization_integration_status",
        "title": "Check DCC-CUA authorization integration",
        "description": "Read the cross-agent task authorization contract. The connected agent host owns user approval; DCC-CUA owns immutable scope, exact-target enforcement, expiry, and revocation.",
        "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
    })
}
