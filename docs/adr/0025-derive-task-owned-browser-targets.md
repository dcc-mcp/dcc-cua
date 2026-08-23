# ADR 0025: Derive task-owned browser targets

## Status

Accepted

## Context

ADR 0024 can authorize an existing exact PID/HWND, but browser-store work often
needs a clean browser that does not exist before the user authorizes the task.
Allowing the client to submit an executable, profile directory, PID, HWND, or
CDP endpoint would let it replace or widen the target after authorization.
Attaching to an arbitrary existing browser would also cross account and origin
boundaries that the pre-task card did not establish.

## Decision

- Add a closed owned-browser target with only `browser=chromium` and
  `profile=isolated_new`.
- After one authenticated user input, DCC-CUA starts one upstream session,
  calls CUA `browser_prepare`, and requires the
  `launched_isolated_browser` outcome with a newly created profile.
- Derive the browser PID from that outcome, enumerate its on-screen windows,
  and require exactly bound browser state with mutation permission and one
  active tab before promoting the PID/HWND into the task session.
- Bind the broker lease to the Host-derived PID/HWND exactly once. The client
  cannot nominate, replace, replay, or widen that identity.
- Keep the browser in the same upstream session so task stop, expiry, or Host
  teardown reaps its isolated profile lifecycle.
- Require `start_authorized_task` before any model-visible observation or
  input. Its response supplies `provider=dcc-cua`, runtime version, PID, and
  HWND for reporting before work begins.
- Copy the authorized methods and exact HTTP(S) origins into the Host grant.
  Browser navigation and mutation fail closed outside those origins.
- Deny client `browser_prepare` for an owned target. Hidden file inputs continue
  through `browser_set_input_files`; no native file chooser is opened.
- Preserve exact-window tasks and fail closed for attachment to an existing
  browser unless a separate exact target was authorized.

## Consequences

### Positive

- One pre-task authorization can launch and bind a clean browser without any
  native per-action popup.
- Browser identity and lifecycle are derived from DCC-CUA-owned effects rather
  than model-provided identifiers.
- Store upload methods, origins, expiry, and revocation remain bounded by the
  same task authorization.

### Negative

- Owned-browser startup adds a bounded wait for the native window and exact CDP
  binding.
- The initial release supports Chromium only; other browser families require a
  new typed contract and implementation.
- A Host restart destroys the owned session and requires fresh user input.

## Failure modes and mitigations

- Launch outcome is missing or ambiguous: stop the upstream session and return
  a typed refusal.
- No exact native/CDP binding appears within 20 seconds: stop the session and
  do not expose a partial target.
- The client supplies PID/HWND, launch details, or `browser_prepare` together
  with an owned target: reject the grant before launch.
- Navigation or mutation crosses an unlisted origin: reject it in Host without
  falling back to a modal prompt.
- Authorization expires or is revoked: the existing broker validation remains
  authoritative for sensitive task actions.

## References

- [ADR 0024: Bridge trusted user input to task authorization](0024-bridge-trusted-user-input-to-task-authorization.md)
- [Issue #179](https://github.com/dcc-mcp/dcc-cua/issues/179)
