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
        "next_owners": ["client_embedding_integration", "deployment_trust_provisioning"],
        "next_action": "Integrate a protected human confirmation transport and constructor-provisioned issuer trust, then validate the actual client launch chain before requesting fresh exact-target authorization. Do not fill task_grant_id or approve through model-visible input.",
        "contract": "https://github.com/dcc-mcp/dcc-cua/blob/main/docs/adr/0027-cross-client-task-authorization.md",
        "signed_receipt_protocol": {
            "status": "implemented_core",
            "constructor_api_available": true,
            "runtime_accepts_receipts": false
        },
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
