# ADR 0028: Delegate task authorization to agent hosts

## Status

Accepted. This supersedes the packaged-runtime confirmation policy in
[ADR 0027](0027-cross-client-task-authorization.md). The immutable task scope,
process-local issuer/validator split, exact-target checks, single-use session
open, expiry, revocation, and per-operation validation remain unchanged.

## Decision

DCC-CUA delegates the human-approval decision to the connected Agent host.
Codex, DSH, Claude, WorkBuddy, and other MCP clients may use their own approval
UI or policy before calling `authorize_task`. The packaged MCP server no longer
opens Windows Hello or a native physical-keyboard prompt, and the same flow is
available on Windows, Linux, and macOS.

The portable flow is:

1. `prepare_task_authorization` retains an immutable proposal and returns its
   exact PID/HWND or owned-browser launch spec, allowed Host methods, final
   action/risk scopes, browser origins, digest, and expiry.
2. The connected Agent host decides whether its user or policy authorizes that
   proposal.
3. `authorize_task` accepts only the server-generated `proposal_id` and issues
   the process-local receipt for that retained scope.
4. `start_authorized_task` consumes the receipt into one task session and
   returns the exact provider/runtime/PID/HWND attestation.
5. Every task call revalidates target identity, method/action/origin scope,
   expiry, and revocation.

`authorize_task` is a normal MCP tool so clients without MCP Apps can use the
same contract. It is marked destructive to give clients an opportunity to
apply their own approval policy. The optional MCP App card remains a rendering
of the retained proposal, not a Windows-specific transport.

## Trust boundary

The owner of the MCP connection is now the authorization authority. DCC-CUA
does not independently prove that a human clicked an Agent-host approval UI.
A client that permits unattended `authorize_task` calls is explicitly choosing
unattended authorization for that connection.

This is a deliberate portability and UX tradeoff. It removes the duplicated
operating-system confirmation and makes behavior consistent across Agent
products, but it is weaker than ADR 0027 against a malicious or
prompt-injected client. Deployments that require independent human-presence
proof must enforce it in the Agent host or keep using the constructor-owned
`TrustedTaskAuthorizationIssuer` API outside the model-accessible MCP channel.

The server still refuses all caller-supplied grant IDs, capabilities, receipt
fields, free-form approval text, scope changes, and target substitution.
Process identity is diagnostic only and cannot widen a proposal. CLI arguments,
environment variables, and redirected stdin cannot authorize a task.

## Compatibility

`authorization_integration_status` now reports schema
`dcc-cua.authorization-integration.v2`, `confirmation_method=client_managed`,
`confirmation_owner=agent_host`, and
`requires_system_user_verification=false` on every supported platform.

Clients should:

- inspect the returned proposal rather than reconstructing it;
- apply their own user/tool approval before `authorize_task`;
- pass only the exact `proposal_id`;
- report provider/runtime/PID/HWND before the first observation or input;
- revoke or abandon the task when the user interrupts it.

Account verification, CAPTCHA/2FA, agreements, payments, and final irreversible
publication remain separate human boundaries. A task authorization never
implies approval for those operations.

## Validation

Regression coverage must prove:

- authorization is available on all supported platforms without Windows APIs;
- `authorize_task` is portable and accepts only a retained proposal ID;
- free-form acknowledgement, secret, grant, receipt, and capability fields are
  rejected;
- an unissued proposal cannot start or execute a task;
- exact target, method/action/origin, expiry, single-use, and revocation checks
  remain unchanged;
- the card and status payload contain no Windows-presence instructions.
