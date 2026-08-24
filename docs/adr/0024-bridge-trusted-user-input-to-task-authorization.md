# ADR 0024: Bridge trusted user input to task authorization

## Status

Accepted

## Context

ADR 0023 defines a constructor-owned task-authorization host, but embeddings
still need a concrete way to turn one authenticated user input into a bounded
authorization. CLI arguments, environment variables, ordinary stdin, and Host
IPC are model-visible or client-controlled, so treating any of them as proof of
user presence would let an agent authorize itself.

The bridge must support long-running automation without native modal prompts,
while retaining exact target and browser-origin fences, expiry, live
revocation, and the existing no-retry rules.

## Decision

- Provide an in-process split-capability broker from
  `trusted_task_authorization_broker`.
- Give the authenticated embedding user-input surface the move-only
  `TrustedTaskAuthorizationIssuer`. It is not serializable and never crosses
  Host IPC.
- Give the Host only an `Arc<dyn TrustedTaskAuthorizationHost>` validator and
  install it through `HostSecurityServices`.
- After explicit user input, register one exact PID/HWND, task grant,
  application label, closed action/risk scopes, browser
  origins, and an expiry no more than 24 hours away.
- Generate both a random authorization ID and the one-time window capability
  inside the broker. The embedding places both opaque values in the task grant
  so the Host can consume the pre-task registration while opening the session;
  possession of either value cannot register, widen, or revoke scope.
- Consume a registration exactly once at session open and bind its in-memory
  lease to the Host-generated session and request digest. A second session
  cannot replay it.
- Revalidate the exact lease and action scope through the broker before every
  otherwise-confirmed action. Revocation takes effect without a popup fallback.
- Bound the broker to 256 live registrations and purge expired registrations
  when new user authorizations are registered.

## Non-functional requirements

- **Security:** fail closed on missing state, poisoned locks, target changes,
  scope changes, replay, expiry, and revocation. Never store input text, secret
  values, clipboard contents, or form values.
- **Compatibility:** sessions without a task authorization keep the existing
  per-action confirmation behavior.
- **Operations:** the broker is process-local and needs no daemon, database,
  secret distribution, or cleanup service.
- **Performance:** registration and per-action validation are bounded in-memory
  map lookups; no network or disk I/O is added to the action path.

## Consequences

### Positive

- One authenticated pre-task user input can authorize a bounded, exact-target
  workflow without repeated native prompts.
- The model-visible Host client can reference an authorization but cannot mint
  or widen it.
- The same broker works for DCC-MCP Core, desktop embeddings, and tests without
  coupling Host policy to one UI toolkit.

### Negative

- The embedding remains responsible for authenticating its user-input surface
  and calling the issuer only after explicit user input.
- A Host restart loses process-local registrations and requires new user input.
- A workflow that changes PID/HWND or browser origin needs a separate
  authorization.

## Alternatives considered

**CLI flag, environment variable, or redirected stdin**

Rejected because the agent controls those channels and could self-authorize.

**Persist reusable bearer tokens**

Rejected because theft or replay would outlive the exact task and process.

**Keep one native modal prompt at task start**

Rejected because it still blocks unattended long-running automation and does
not use the embedding's existing authenticated user-input surface.

**Add a broker daemon or database**

Rejected because the current single-user local Host needs neither distributed
coordination nor durable authorization. Process-local state has a smaller
attack and operational surface.

## Failure modes and mitigations

- Embedding disappears: the Host keeps validating existing in-memory state;
  expiry still ends the lease.
- Host restarts: the registration is lost and session open returns a typed task
  authorization refusal.
- Concurrent replay: the first exact session consumes the registration; later
  attempts are denied.
- Target or origin changes: Host evidence no longer matches the registered
  scope and the action fails closed.
- Revocation races with an action: broker validation is serialized by its
  state lock; the action observes either active state before revocation or the
  revoked state, never widened scope.

## References

- [ADR 0023: Authorize bounded tasks without action popups](0023-authorize-bounded-tasks-without-action-popups.md)
- [ADR 0026: Attest trusted desktop embeddings](0026-attest-trusted-desktop-embeddings.md)
- [Issue #170](https://github.com/dcc-mcp/dcc-cua/issues/170)
