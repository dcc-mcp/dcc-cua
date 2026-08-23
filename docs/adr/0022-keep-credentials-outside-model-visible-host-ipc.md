# ADR 0022: Keep credentials outside model-visible Host IPC

## Status

Accepted

## Context

ADR 0020 made sensitive actions eligible for an exact, constructor-owned user
confirmation. It intentionally did not make model-visible password, API-key,
authentication-code, or generated-secret transport acceptable. Passing those
values as `text`, command-line arguments, Host responses, or semantic snapshot
names would still expose them to callers, logs, receipts, or conversation
history.

Publishing portals also create new secrets that an authorized workflow must
move into another service. Returning a generated value to the agent before it
can be stored would defeat the same boundary.

## Decision

- Accept an opaque, bounded `secret_handle` as the alternative to plaintext
  `text` for Host window, desktop, and browser text input. The two fields are
  mutually exclusive.
- Keep secret resolution in a constructor-owned `HostSecretVault`. IPC can
  select a handle but cannot install, replace, or call the vault directly.
- Bind the handle, exact PID/HWND, capability, observation, intent, and complete
  action into the trusted-confirmation digest. Resolve the value only after the
  exact confirmation succeeds.
- Install a platform-keyring implementation in the packaged CLI Host. Use the
  service name `dcc-cua` and the validated handle as the account identifier.
- Add `clipboard_capture_secret` as an exact-session output sink. It requires
  the latest observation, clipboard read and write grants, and trusted user
  confirmation. It accepts only CUA's structured privacy-sensitive clipboard
  result, stores the value directly in the vault, then clears the clipboard.
- Return only the handle and clipboard-clear status. Never return the captured
  or resolved value through Host IPC.
- Redact resolved values from debug formatting, zeroize owned short-lived
  buffers where the downstream contract permits, and map vault failures to
  typed messages that contain no provider error or secret value.
- Never use a form control's current value as browser-extension semantic
  snapshot naming metadata.

## Consequences

### Positive

- An authorized agent can reuse existing or newly generated credentials without
  placing their values in its request, response, or conversation context.
- A task grant still cannot approve its own credential use or capture.
- Generated portal secrets can move from an exact user-approved browser flow to
  the operating-system credential store without a model-visible round trip.
- Unlabeled form controls no longer leak their current values in extension
  snapshots.

### Negative

- The platform keyring must be available and unlocked; otherwise secret
  operations fail closed.
- The local CUA driver still receives the resolved text at the final dispatch
  boundary because its existing input contract requires text. The Host does not
  expose that value over its public IPC and clears its owned buffers as soon as
  the dispatch completes.
- Clipboard capture depends on a portal providing a copyable text value. If a
  site exposes a secret only through a protected or non-text surface, the human
  must complete that step or a future exact-ref sink must define a separate
  contract.

### Neutral

- Plaintext input remains compatible for non-secret text.
- Existing task grants default to no trusted confirmation and therefore cannot
  use either secret input or the clipboard sink without an explicit change.
- CAPTCHA and equivalent human-verification challenges remain human-only.

## References

- [ADR 0020: Authorize sensitive actions with exact user confirmation](0020-authorize-sensitive-actions-with-exact-user-confirmation.md)
