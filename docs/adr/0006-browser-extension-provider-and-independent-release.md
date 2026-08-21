# ADR-0006: Add a browser extension provider with an independent release

## Status

Accepted

## Context

The existing-profile Chromium route currently depends on a browser-level CDP
socket. Modern Chrome versions may require native consent for each genuinely
new socket. Reconnects, stale consent surfaces, and repeated target
revalidation make that route slower and less reliable than a browser-owned
integration for signed-in tabs.

The browser integration and the native dcc-cua runtime still share security
contracts: exact native-window identity, task grants, origin approval,
observation fencing, interruption, action confirmation, and audit evidence.
They therefore need one source repository and one cross-component test suite,
but they cannot assume lock-step delivery. Browser store review and update
timing are independent from GitHub binary releases.

## Decision

- Keep the browser extension, native bridge, protocol schemas, and dcc-cua Host
  implementation in this repository.
- Publish `dcc-cua` and `dcc-cua-browser-extension` as separate release-please manifest
  components with independent versions, tags, changelogs, release PRs, and
  artifacts.
- Use WXT and TypeScript to build Chrome, Edge, and Firefox Manifest V3
  packages from one source tree. Distribute them through Chrome Web Store,
  Edge Add-ons, and Firefox Add-ons. GitHub release artifacts are
  reviewable/testable packages, not a silent sideload mechanism.
- Ship Native Messaging registration support with the native dcc-cua release.
  Chromium native host manifests must allow the exact Chrome and Edge origins;
  Firefox manifests must allow the exact Firefox extension id. Wildcards are
  forbidden.
- Keep CDP as the default provider. The CLI planner selects the extension only
  when CDP is unavailable, reports missing registration/pairing as an explicit
  next action, and never claims that Native Messaging registration installed a
  store extension.
- Register an explicitly paired extension with the persistent Host, bind it to
  the browser process that launched the Native Messaging executable, and allow
  calls only from an exact window session with the same process id and paired
  origin. Provider secrets and pairing nonces are never exposed by status.
- Negotiate a versioned protocol range. Host and extension SemVer versions are
  informational; compatibility is decided by protocol overlap and advertised
  capabilities.
- Use the extension provider for explicitly paired signed-in tabs. Retain the
  native provider for browser chrome and operating-system surfaces, and retain
  CDP for driver-owned isolated profiles. Do not silently fall back between
  providers after a policy, permission, identity, or confirmation failure.
- Start with `activeTab`, `scripting`, `storage`, and `nativeMessaging`. Do not
  request blanket host permissions. Pairing a tab is an explicit user action.

## Consequences

### Positive

- Signed-in web sessions no longer require an existing-profile CDP socket.
- Tab, frame, origin, and document identity come from the browser itself.
- Extension-only changes do not force a native binary version bump.
- Shared protocol fixtures and E2E tests can evolve atomically in one repo.
- Browser stores supply signed installation and automatic updates.

### Negative

- Release automation must handle two independent components and tag formats.
- Store rollout can lag the native release, so both sides must support a
  compatibility window.
- Native Messaging registration differs by operating system and browser
  family.
- Firefox requires accurate built-in disclosure for website data transmitted
  to the local Native Messaging host.

### Neutral

- Development and CI use unpacked or ZIP artifacts, while production users
  install the signed Web Store package.
- Enterprise installations may use managed extension policies and their own
  update source without changing the public distribution contract.

## Alternatives Considered

**Bundle and sideload the extension in every dcc-cua archive**

- Rejected because ordinary Windows and macOS Chrome installations cannot
  directly install arbitrary self-hosted extensions, and silent installation
  would bypass the browser's permission review.

**Put the extension in a separate repository**

- Deferred because the provider protocol and security invariants are still
  changing together. A split would add cross-repository drift without a
  separate ownership or compliance boundary.

**Keep exact version lock-step between Host and extension**

- Rejected because browser stores control extension update timing.

**Replace all browser routes with the extension**

- Rejected because extensions do not own native browser chrome, operating
  system dialogs, or non-browser DCC applications.

## References

- https://developer.chrome.com/docs/extensions/how-to/distribute
- https://developer.chrome.com/docs/extensions/develop/concepts/extensions-update-lifecycle
- https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging
- https://wxt.dev/guide/essentials/target-different-browsers
- https://extensionworkshop.com/documentation/develop/firefox-builtin-data-consent/
- https://github.com/googleapis/release-please/blob/main/docs/manifest-releaser.md
