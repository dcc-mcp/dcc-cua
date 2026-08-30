//! Discovery is not authority. Availability requires a protected native user-presence verifier.

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

pub(crate) fn available_status() -> Value {
    json!({
        "schema": "dcc-cua.authorization-integration.v1",
        "provider": "dcc-cua",
        "runtime_version": env!("CARGO_PKG_VERSION"),
        "status": "available",
        "authorization_available": true,
        "user_confirmation_available": true,
        "card_available": true,
        "process_identity_can_authorize": false,
        "confirmation_method": "windows_protected_user_presence",
        "reason": "protected_user_presence_verifier_available",
        "next_owners": [],
        "next_action": "Prepare one exact-window task card. The private authorization tool must complete protected Windows user presence verification before the process-local issuer can register the immutable scope.",
        "contract": "https://github.com/dcc-mcp/dcc-cua/blob/main/docs/adr/0027-cross-client-task-authorization.md",
        "signed_receipt_protocol": {
            "status": "implemented_core",
            "constructor_api_available": true,
            "runtime_accepts_receipts": false
        },
        "fallback": "windows_non_injected_keyboard_sequence_when_user_consent_is_unavailable"
    })
}

pub(crate) fn tool() -> Value {
    json!({
        "name": "authorization_integration_status",
        "title": "Check DCC-CUA authorization integration",
        "description": "Read whether this DCC-CUA connection has a protected human confirmation surface. If it reports integration_required, do not invent a card, grant, or alternative provider.",
        "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
    })
}

pub(crate) fn tools() -> Value {
    json!([tool()])
}
