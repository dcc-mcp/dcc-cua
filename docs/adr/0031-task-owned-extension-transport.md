# ADR 0031: Task-owned extension transport and input

## Status

Implementation candidate. Source tests, cross-platform CI, installed runtime,
installed Chrome extension, and actual signed-in workflows are separate gates.

## Problem

The Native Messaging bridge registered the explicitly paired tab only with the
default persistent Host. Each MCP task uses an embedded Host with its own
process-local provider registry, so its extension inventory was empty. Merely
admitting extension methods also failed to enforce final action scopes.

## Decision

- Keep task leases, sessions, secret vaults and window capabilities in their
  existing embedding. Do not forward a task to another client's Host/session.
- Start a process-owned provider-only endpoint when an MCP connection starts an
  extension task. Reuse current-user named-pipe/Unix-socket protections. Its
  constructor-only admission accepts hello/ping/register/poll/complete/unregister;
  it exposes no session, UI or authorization entrypoint. Listener cancellation
  owns and cancels its connection workers.
- Treat live connection diagnostics as bounded discovery hints, not authority.
  Derive endpoint names only from connection UUIDs, verify OS peer PID and current
  user, and recheck the full process birth before and after connecting. No caller
  can nominate an arbitrary endpoint through the task bridge.
  Unix endpoint names retain all UUID bits as lowercase hexadecimal within
  the same private runtime directory; paths that cannot fit the platform's
  native socket address, including its terminating NUL, are refused.
- At explicit pairing, Native Messaging captures its parent browser PID and the
  unique foreground native window. It registers only with one live endpoint whose
  constructor binding matches both. Ambiguous tasks fail closed. Each connection
  admits one extension task; revoke it before replacing it. The selected endpoint
  never migrates or reconnects to a different owner. Listener revocation closes
  its connections. Browser restart/disconnect clears pairing and requires a fresh
  user click. Task-owner hashes remain internal relay metadata.
- Bind click/type to a task-retained snapshot, ref, provider, origin and current
  evidence epoch. Recheck foreground/desktop readiness and validate the exact
  final scope through the existing task-authorization broker before input.
  Every click is conservatively `external_effect`; ordinary non-secret typing
  is `content_change`. Typing text is bound by digest without echoing its value.
- Consume evidence on every attempted call. Bound queued command lifetime by
  both the call timeout and task lease expiry; the native bridge never dispatches
  expired commands. Response mismatch/disconnect is an unknown outcome, never
  permission for automatic retry or provider/session reassignment.
- The tab bridge exports only non-secret metadata, rejects password/file targets,
  and rechecks the live role/name/action/visibility before consuming a ref.
  Unknown sensitive-state metadata fails closed for typing. No password database,
  cookies, tokens, or stored form values are read. Browser autofill, native
  credential prompts, CAPTCHA and 2FA remain separate supported/human boundaries.

## Compatibility and acceptance

The native acknowledgement advertises `task_owned_pairing_v1`; Host advertises
`browser_extension_task_input_v1` and `browser_extension_mcp_transport_v1`.
Old native bridges do not supply this transport. Older extensions lacking the
new non-sensitive metadata cannot receive type actions. There is no silent
fallback to CDP or another browser.

Navigation retains the existing explicit re-pair requirement. Do not silently
reuse the former document's pairing after navigation or after task termination.
An input acknowledgement reports an attempt, not verified page completion;
take a fresh snapshot and inspect the destination state.

Local protocol, lease, IPC and DOM-fixture tests do not establish Chrome-native
autofill, real account login, organization access, or project submission. Those
require a compatible installed route, a fresh exact target/task binding, and
actual extension observations. Runtime/native-host installation or restart is
not part of this source change.
