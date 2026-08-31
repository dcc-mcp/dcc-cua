# ADR 0028: Delegate MCP task authorization to Agent Hosts

## Status

Accepted. This supersedes the packaged MCP confirmation policy in
[ADR 0027](0027-cross-client-task-authorization.md) and the earlier three-step
client-managed proposal flow. The internal immutable scope, process-local
issuer/validator split, exact-target checks, expiry, revocation, and
per-operation validation remain unchanged.

## Decision

The connected Agent Host is the user-authorization authority for the packaged
MCP server. Its sandbox, tool approval, and permission policy decide whether a
task may start. DCC-CUA does not repeat that decision with an authorization
card, operating-system user-presence check, physical-key sequence, or separate
model-visible authorize call.

The public lifecycle is:

1. `start_task` accepts one exact PID/HWND or closed owned-browser launch spec,
   allowed Host methods, final action/risk scopes, browser origins, and expiry.
2. The server validates and retains that request, creates a random internal
   grant and process-local lease, opens the exact task session, and returns the
   `task_id`, provider, runtime version, PID, and HWND.
3. `dcc_cua_task_call` performs bounded work in that task and revalidates the
   target, method, action, origin, expiry, and stop state on every call.
4. `task_status` reports lifecycle state; `stop_task` closes the session and
   revokes the internal lease.

The MCP server exposes no resources and no `authorization_integration_status`,
`prepare_task_authorization`, `authorize_task`, `start_authorized_task`, or
`revoke_task_authorization` tools. The internal lease is an enforcement detail,
not a second user-approval protocol.

## Trust boundary

An Agent Host that permits unattended `start_task` calls is explicitly choosing
unattended automation for that connection. This matches IDE and Agent systems
that already gate tools through sandboxes and permission policy, and avoids a
duplicated prompt that blocks end-to-end automation.

The server still rejects caller-supplied grant IDs, authorization IDs,
capabilities, receipts, target substitution, scope widening, and unknown
fields. Exact PID/HWND or Host-derived browser identity, closed method/action
scopes, expiry, stop/revocation, fresh observations, interruption, and
post-action verification continue to fail closed.

Deployments that require an independent human-presence service may use the
constructor-owned `TrustedTaskAuthorizationIssuer` and signed receipt APIs from
ADR 0027 in a custom in-process embedding. Those APIs are not exposed through
the packaged MCP transport.

## Compatibility

This replaces the unreleased client-managed MCP surface with a one-call task
lifecycle. Clients must call `start_task`, retain its opaque `task_id`, report
the returned provider/runtime/PID/HWND before work, use `dcc_cua_task_call`, and
call `stop_task` when done. The behavior is identical on Windows, Linux, and
macOS; no platform-specific confirmation integration is required.

Account verification, CAPTCHA/2FA, agreements, payments, and final irreversible
publication remain separate human boundaries. A DCC-CUA task never implies
approval for those external operations.

## Validation

Regression coverage must prove:

- all supported Agent Host labels receive the same four-tool MCP surface;
- MCP resources are empty and removed authorization tools remain unavailable;
- `start_task` creates the internal lease without user confirmation fields;
- exact target, method/action/origin, expiry, stop, and revocation checks remain;
- caller-supplied grant, receipt, capability, and widening fields are rejected;
- subprocess behavior and the executable manifest match the source contract on
  every supported platform.
