//! Discovery is not authority. The packaged CLI has no trusted human-input port.

use serde_json::{Value, json};

pub(crate) const REQUIRED: &str = "integration_required";

pub(crate) fn status() -> Value {
    json!({
        "schema": "dcc-cua.authorization-integration.v1",
        "provider": "dcc-cua",
        "runtime_version": env!("CARGO_PKG_VERSION"),
        "status": REQUIRED,
        "authorization_available": false,
        "user_confirmation_available": false,
        "card_available": false,
        "process_identity_can_authorize": false,
        "reason": "trusted_human_confirmation_transport_not_configured",
        "next_owner": "client_embedding_integration",
        "next_action": "Install a constructor-owned human confirmation transport with independently provisioned trust, then request a fresh exact-target task authorization. Do not fill task_grant_id or approve through model-visible input.",
        "contract": "https://github.com/dcc-mcp/dcc-cua/blob/main/docs/adr/0027-cross-client-task-authorization.md",
        "signed_receipt_protocol": {"status": "proposed", "runtime_accepts_receipts": false},
        "fallback": "none"
    })
}

pub(crate) fn tools() -> Value {
    json!([{
        "name": "authorization_integration_status",
        "title": "Check DCC-CUA authorization integration",
        "description": "Read whether this DCC-CUA connection has a trusted human confirmation surface. Report integration_required to the user; do not invent a card, grant, or alternative provider.",
        "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
    }])
}
