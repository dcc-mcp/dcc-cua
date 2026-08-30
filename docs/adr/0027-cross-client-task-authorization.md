# ADR 0027: Cross-client task authorization

## Status

Accepted for the DCC-CUA core boundary. The runtime now owns immutable bounded
challenges, RFC 8785 JCS encoding, strict Ed25519 receipt verification,
constructor-provisioned issuer trust, atomic single consumption, browser
bootstrap separation, clock-rollback refusal, and live lease/key revocation.

The packaged Windows MCP server now has a process-local protected user-presence
transport. It reports `available`, exposes a bounded authorization card, and
uses Windows `UserConsentVerifier` when configured. Otherwise it requires the
physical F12, F11, F10 sequence through a low-level keyboard hook that rejects
injected and lower-integrity-injected events. The card and MCP Apps visibility
metadata carry scope and routing only; neither is authority. Non-Windows
packaged servers remain diagnostic-only and report `integration_required`.

This ADR does not certify cloud/local routing, non-Windows confirmation,
browser-store publication, or a particular client deployment. Those still
require real launch-chain, exact-target, revocation, and user acceptance.

Implementation and client acceptance are tracked in
[Issue #237](https://github.com/dcc-mcp/dcc-cua/issues/237).

This revises the process-identity assumption in
[ADR 0026](0026-attest-trusted-desktop-embeddings.md), not the constructor-owned
issuer/validator boundary in [ADR 0024](0024-bridge-trusted-user-input-to-task-authorization.md).

## Problem and evidence

A desktop embedding can launch MCP through an unpackaged signed helper with
neither a package identity nor a version resource. The old bridge refused
startup, so neither the authorization card nor a useful status tool appeared.
An independent Host request then failed parsing `task_grant_id`, masking the
earlier integration failure. The card implementation already existed.

Relaxing executable metadata checks does not solve the trust problem. A process
signature authenticates code, not an individual human decision. MCP clientInfo,
tool visibility metadata, chat text, CLI arguments, environment variables, and
stdio acknowledgements cannot independently prove human presence. A signed
receipt is also insufficient if the model can invoke its signer or choose the
trusted public key.

## Implemented boundary

On Windows, the packaged `mcp-server` creates the split issuer/validator broker
inside the process, but retains the issuer behind the native user-presence
host. `authorize_task` accepts only the server-generated proposal ID. It
displays the retained application, proposal, PID/HWND, SHA-256 scope digest,
and expiry, then invokes the private issuer only after Windows verifies user
presence. A click on the native prompt never authorizes the physical-sequence
fallback. Chat/card text, forged signature or grant fields, environment,
stdin, process identity, and injected keyboard events cannot register scope.

The status document has schema `dcc-cua.authorization-integration.v1`. On
Windows it reports the selected `confirmation_method` and makes the card plus
private issuer tools available; on other platforms it exposes only the status
tool and reports `integration_required`. In both modes,
`process_identity_can_authorize` is false. Parent attestation is optional
diagnostic evidence and caller-selected client names cannot change behavior.
The packaged runtime still does not accept a model-relayed receipt merely
because the core constructor API exists.

Authorization is single-use for session open and binds the exact retained
target, closed Host methods, final action/risk scopes, browser origins, expiry,
and revocation state. The runtime rechecks expiry after the blocking verifier
returns and the Host revalidates the resulting lease on every operation.

Each Host transport connection now receives a random `connection_id` in its
`hello` response. A cross-client challenge retains that exact value; broker
authorization, the lease digest and every live validation compare it. An
opaque receipt relayed to another Host connection therefore cannot win a
first-consumer race. The signed `task_id` must also equal the later Host logical
session ID; a receipt cannot be rebound to another task on the same connection.
For signed exact-window work, the Host also consumes and
validates the authorization against the nominated nonzero PID/HWND before it
starts the CUA session. Authorization failure cannot activate the window,
start showcase recording, launch an owned browser or call a global Host route.

## Common protocol

```mermaid
sequenceDiagram
    participant Agent as Model client
    participant Runtime as DCC-CUA runtime
    participant UI as Trusted confirmation host
    participant Human as User
    Agent->>Runtime: Propose bounded work (no authority)
    Runtime->>UI: Authenticated immutable challenge
    UI->>Human: Exact task, target, scopes, expiry
    Human->>UI: Confirm or deny
    UI->>Runtime: Signed decision for that challenge
    Runtime->>Runtime: Verify and consume once
    Agent->>Runtime: Action with opaque session reference
    Runtime->>Runtime: Check target, evidence, scope, expiry, revocation
    Runtime-->>Agent: Verified result or refusal
```

### Trust provisioning

The runtime's trust registry is installed through a constructor-owned service
by the embedding/deployment owner. It binds issuer keys to an authenticated
human-confirmation service and exact client/tenant/user audience. Agent-readable
files, environment variables, command arguments, requests, and self-signed
receipts cannot enroll keys. The signing key and the operation that uses it
must be inaccessible to model-controlled tools, shells, page scripts, and
generic UI automation. An unrestricted local `sign` endpoint is not trusted.

The confirmation host must authenticate the runtime and challenge delivery too;
it must not sign a challenge supplied by a page or substituted by the model.
Key rotation/revocation uses the same independent administrative channel.
This proposal adds no signing secret to the CLI and no bearer-token file.

### Immutable challenge

The runtime generates a cryptographically random nonce and retains the exact
challenge in a bounded, process-local pending registry. Bind all of:

- Protocol version, provider `dcc-cua`, runtime version, runtime instance and
  boot generation, client/tenant/user audience, task ID and connection/session.
- Exact process ID and creation identity, platform-native window identity
  (Windows HWND), and for browser work the Host-observed target ID, tab ID,
  document generation, and canonical HTTP(S) origin.
- Closed Host methods, final action/input kinds, risk categories, secret-handle
  policy, task purpose and any exact irreversible operation being authorized.
- Issuance time, confirmation deadline, task expiry, nonce and challenge ID.

Do not include passwords, tokens, input text, clipboard values, unrelated page
contents, local paths, or cookies. Human labels are bounded data, not authority;
display machine-bound identities and effective scopes too. The UI displays the
exact retained challenge, not a model-written summary. Changed scope or target
requires a new challenge. A title is not a tab ID. A cloud task cannot nominate
local PID/HWND as identity evidence.

For the current generic browser route, `document_generation` is the exact
snapshot generation observed after bootstrap and before the final decision.
The final lease pre-binds the retained Host target, requires that exact tab on
the first snapshot, and refuses target, tab, generation or origin drift before
publishing evidence or dispatching a browser mutation.

Browser setup needs a separate minimal bootstrap authorization when exact tab
identity is not yet observable. It may authorize only fixed exact-window
attachment or a closed isolated-browser launch spec, not DOM read/write,
arbitrary input, uploads, or publication. After deriving the actual browser,
tab and origin, request the final task decision. Do not fabricate a tab ID to
avoid this two-stage boundary.

### Signed decision

Use a closed v2 decision record with schema, issuer key ID, audience, runtime
instance/generation, challenge ID, nonce, SHA-256 challenge digest, decision
(`allow` or `deny`), issuance time and expiry. No receipt-supplied scope can
replace the pending challenge. No receipt supplies a trust root or algorithm.

The algorithm is Ed25519 over the ASCII domain separator
`dcc-cua.task-authorization-receipt.v2`, a NUL byte, and the UTF-8 JCS
serialization of the unsigned decision. Digest the JCS serialization of the
retained challenge. Reject duplicate JSON keys, unknown fields, noncanonical
encodings and out-of-range integers. Use a reviewed library and published test
vectors, not application cryptography. See
[RFC 8032](https://www.rfc-editor.org/rfc/rfc8032.html) and
[RFC 8785](https://www.rfc-editor.org/rfc/rfc8785.html).

Validate issuer trust/revocation, signature, audience and generation bindings,
digest, nonce, deadline and expiry against retained state. Under one lock, a
matching decision consumes the pending challenge at most once. Denial is
terminal too. Verification failure cannot create a task, retry an action,
widen a grant, open a popup, or change provider.

Only successful verification may call the private issuer with the immutable
scope. The model receives an opaque reference, never signing authority. A
receipt may be relayed through an untrusted client, but cannot be minted there.
A valid signature does not prove correct human UI or target-binding integration.

### Lease and revocation

Every call, including observations, rechecks current runtime/connection/target
identity, exact tab/origin, document evidence, method/action/risk scope, expiry
and live revocation epoch. The default Windows target observer compares the
live HWND owner and process creation FILETIME; another platform must inject an
equivalent constructor-owned observer or authorization stays unavailable.
Receipt expiry cannot exceed challenge expiry. Cap pending confirmations at
five minutes and leases at the existing 24-hour maximum; deployments may
require shorter limits.

The trusted UI provides an authenticated, sequence-bound revocation path.
Restart, connection loss, unavailable validation, clock rollback, revoked keys,
target replacement, and stale evidence fail closed. Do not persist reusable
authorizations across runtime generations. Escape/user stops interrupt
immediately and require fresh authorization.

Account verification, CAPTCHA/2FA, payments, agreements, and final irreversible
publication remain separate human boundaries. A broad task lease must not
silently authorize them. Operation-specific confirmation names the exact
effect, target item and artifact/version where applicable.

## Compatibility and next owners

| Client | Current packaged MCP behavior | Required host work |
| --- | --- | --- |
| Codex desktop on Windows | Packaged protected user-presence verifier available; helper metadata cannot authorize | Install matching binary/plugin, test the actual prompt, exact target, effect and revocation chain |
| Codex cloud | Diagnostic only; no implied local browser access | Authenticated local companion or cloud-owned target, audience-bound confirmation and revocation |
| Windows desktop/CLI clients | Same packaged verifier; client name, stdin and parent process are not authority | Verify each actual launch chain and card rendering; never substitute client metadata for user presence |
| Cursor on non-Windows | Diagnostic only; client name is not identity | Verified human UI/signing integration and lifecycle tests |
| WorkBuddy on non-Windows | Diagnostic only; signed product identity is not user presence | Verify the deployed product and integrate its real human-input boundary |
| CodeBuddy CLI on non-Windows | Diagnostic only; no inherited desktop trust | Independent confirmation service; do not infer CLI trust from a desktop executable |
| Unknown/noninteractive client | `integration_required` | May relay an independently signed exact challenge only after verifier integration; otherwise stop |
| Trusted in-process embedding | Existing constructor-owned broker API | Keep issuer on authenticated user-input side; never expose registration as a model tool |

DCC-CUA owns challenge types, strict decoding, the verifier adapter, atomic
consumption, exact-target lease enforcement and diagnostics. Client owners own
human presence, signer/key provisioning, trusted display, audience binding and
revocation transport. Cloud/local routing requires both owners. The Windows
transport is implemented here, but each installed client and real target still
needs an acceptance run; source tests do not prove its UI.

## Validation and client rollout

Implemented core regressions cover published signature/canonicalization
vectors; malformed, duplicate, unknown, and noncanonical fields; unsafe JSON
integers; wrong/unknown/revoked/unavailable trust; forged signatures; replay and
concurrent consumption; runtime/audience/nonce/digest substitution; scope and
browser bootstrap/origin widening; wrong Host connection; process-instance,
PID/HWND, target/tab/document drift; expiry; clock rollback; and live refusal
of the next protected action or observation after revocation.

The fixed interoperability vector in
`crates/dcc-cua-host/src/tests/cross_client_task_authorization.rs` publishes the
canonical v2 decision payload and its deterministic Ed25519 signature using the
RFC 8032 test key; client implementations must reproduce both byte-for-byte.

Client rollout still requires:

1. Core and integration: substituted runtime/task/audience/PID/HWND/tab/origin;
   scope or expiry widening; restart and lost revocation state against the real
   client transport.
2. Client isolation: model calls, forged clientInfo/metadata, hostile pages,
   shell/stdio/environment and synthetic UI events cannot sign or enroll
   trust. Cancellation/denial never yields a grant.
3. Real client: show the actual bounded confirmation, obtain genuine human
   input, attest provider/runtime/PID/HWND before observation, verify one scoped
   effect, then revoke and prove the next call is refused.
4. Store workflow: bind a fresh exact tab, verify every transition and upload
   receipt, and stop at the exact required human-only final step. Tests, package
   builds and successful API responses do not prove publication.

Packaged-server regressions exercise real subprocess MCP handshakes, all six
client labels, absent/recognized parent identity, forged authorization fields,
app-only tools, rejected human-presence decisions, and physical-key transition
logic including injected events. Core regressions execute the cryptographic
protocol; neither test group certifies a live client UI or physical input path.

## Alternatives and costs

Publisher-only trust, ancestor traversal, caller-supplied keys, magic flags,
ordinary stdin approval and auto-clickable local consent pages are rejected:
none proves an independent human decision. MCP elicitation or Apps may carry
the display, but the host still owns isolation and authentication. See the
[MCP elicitation specification](https://modelcontextprotocol.io/specification/2025-11-25/client/elicitation).

A common receipt permits untrusted relays and avoids per-client policy forks,
but introduces key lifecycle and local/cloud routing costs. Retain the existing
process-local broker; do not add a public authorization daemon, database, or
general signing API in this repair. Review and test each protected client
deployment before claiming live acceptance.
