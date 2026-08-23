# ADR 0023: Authorize bounded tasks without action popups

## Status

Accepted

## Context

Per-action confirmation preserves a strong user-presence boundary, but modal
prompts block long-running automation. They can also form a confirmation loop
when action evidence or the upstream semantic session expires while the user is
responding.

The operator wants to authorize a bounded task once, before it starts, and then
allow in-scope actions to run without additional popups. A caller-provided
Boolean or reusable bearer token still cannot prove that the operator supplied
that authorization.

## Decision

- Add an optional constructor-owned trusted task-authorization host beside the
  existing action-confirmation host. It is never reachable through Host IPC.
- The embedding collects explicit user input before the task and registers a
  bounded authorization with that host. An IPC task grant may reference an
  authorization ID, but it cannot mint or widen the authorization.
- At session open, the trusted host returns an in-memory lease bound to the
  authorization ID, task grant, application label, exact PID/HWND when
  window-scoped, allowed action and input kinds, Host-evidenced risk categories,
  browser origins where applicable, issuance time, and expiry.
- Revalidate the lease with the constructor-owned host before every action that
  would otherwise require confirmation. This makes revocation effective during
  a running task.
- An active lease suppresses the modal prompt only when the exact target and
  action kind remain inside its scope. Hard-denied controls, missing evidence,
  completion-unknown behavior, browser origin fences, and no-retry rules remain
  unchanged.
- When no task authorization is configured, existing clients retain the current
  per-action confirmation path. Once a session references a task authorization,
  an expired, revoked, mismatched, unverifiable, or out-of-scope lease stops the
  action with a typed task-authorization error and never falls back to a popup.
- Never include input text, secret values, clipboard contents, or form values in
  a task-authorization request, lease, receipt, or diagnostic.

## Consequences

### Positive

- One explicit user authorization can cover a bounded long-running task without
  interrupting each action.
- A model or raw Host client cannot self-authorize with a Boolean or replayable
  bearer token.
- Exact target, evidence, expiry, revocation, and action-scope checks remain
  enforceable at every mutation boundary.

### Negative

- Embeddings must implement the trusted task-authorization host and collect the
  user's input before opening the task session.
- The packaged CLI remains on per-action confirmation until it is paired with a
  trusted non-modal user-input broker; silently trusting command-line flags or
  redirected stdin would weaken the boundary.
- One authorization may require separate exact-target leases when a workflow
  opens a new process or modal window.

### Neutral

- Existing callers and grants remain compatible and keep the current action-time
  prompt behavior.
- The initial contract uses closed Host-evidenced risk categories, action kinds,
  browser origins, and exact session targets rather than client-provided intent
  text.

## Alternatives Considered

**Treat `allow_trusted_confirmation` as task approval**

Rejected because the Host client supplies the field and could approve itself.

**Accept a reusable authorization token over Host IPC**

Rejected because an agent could replay or widen a bearer token outside the
original user-approved scope.

**Show one native modal prompt at task start**

Rejected as the default because it still blocks unattended startup and does not
meet the no-popup requirement. Embeddings may provide their own explicit user
input surface before constructing the Host authorization.

**Remove confirmation for all exact-window actions**

Rejected because an exact window can still contain payment, credential,
destructive, terminal, or protected controls.

## References

- [ADR 0017: Derive action safety from Host evidence](0017-derive-action-safety-from-host-evidence.md)
- [ADR 0020: Authorize sensitive actions with exact user confirmation](0020-authorize-sensitive-actions-with-exact-user-confirmation.md)
- [Issue #168](https://github.com/dcc-mcp/dcc-cua/issues/168)
