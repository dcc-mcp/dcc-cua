# ADR 0020: Authorize sensitive actions with exact user confirmation

## Status

Accepted

## Context

DCC-CUA permanently rejected password, credential, authentication-code, and
security-setting controls even when an operator wanted the agent to complete a
specific account workflow. This made trusted confirmation ineffective for
credential creation, sign-in, publishing, payment, and similar browser or
application flows.

A caller-provided Boolean cannot prove user intent. At the same time, a blanket
hard deny conflates an authorized account mutation with command execution,
scope escape, or circumvention of third-party human verification.

## Decision

- Keep `allow_trusted_confirmation` as a permission to ask, not an approval.
- Classify password, credential, authentication-code, security-setting, and
  privacy-setting controls in otherwise eligible applications as
  `action_confirmation`.
- Keep terminal/run-dialog controls, unverifiable targets, protected operating
  system authentication and password-manager surfaces, scope escape, safety
  bypass, and automated human-verification circumvention hard-denied.
- Extend the trusted confirmation request to schema v2. Window requests carry
  the exact PID and HWND, and those fields participate in the request digest
  with the session, grant, capability, observation, accessibility state,
  intent, and complete action.
- Install a constructor-owned native confirmation host in the packaged Windows
  CLI Host. It serializes prompts, defaults to denial, identifies the exact
  target and action type, and never echoes action text or secrets.
- Continue to require an embedding-owned callback on platforms without a
  packaged native prompt. The callback remains unreachable through Host IPC.

## Consequences

### Positive

- Explicitly approved account and publishing workflows can continue without a
  permanent policy dead end.
- A task grant or model-supplied field cannot approve its own action.
- Approval is bound to one exact target, evidence state, and action and cannot
  be replayed after any of them changes.
- Concurrent actions cannot present overlapping prompts, and prompt rendering
  does not disclose submitted text.

### Negative

- Windows users may see more action-time prompts during sensitive workflows.
- Other platforms still depend on their embedding to provide trusted user
  confirmation.
- Secure secret-handle input and a secret-output sink remain separate contracts;
  this decision does not make model-visible secret transport acceptable.

### Neutral

- Existing task grants default to no trusted confirmation and retain their
  current behavior.
- CAPTCHA and equivalent human-verification steps still require the human to
  complete the challenge directly.

## Alternatives Considered

Allowing `allow_trusted_confirmation` to authorize every matching action was
rejected because the caller can submit that field and it is not a user gesture.

Keeping every authentication-related control hard-denied was rejected because
it prevents legitimate, explicitly approved account workflows without reducing
the separate command-execution or scope-escape risks.

Accepting reusable approval tokens over Host IPC was rejected because an agent
could replay or broaden them and the confirmation boundary would no longer be
constructor-owned.

## References

- [ADR 0017: Derive action safety from Host evidence](0017-derive-action-safety-from-host-evidence.md)
- [ADR 0022: Keep credentials outside model-visible Host IPC](0022-keep-credentials-outside-model-visible-host-ipc.md)
