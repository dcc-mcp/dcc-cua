# ADR 0021: Gate browser store publishing and rebuild release PRs

## Status

Accepted

## Context

The native runtime and browser extension have independent release-please
components, but both release PRs update `.release-please-manifest.json`. Merging
either PR can therefore make the other branch conflict with `main`. Merging
`main` into generated release branches retains avoidable history and can also
combine one component's pending version with stale state from the other.

GitHub Releases already contain reviewable Chrome, Edge, and Firefox packages,
but uploading them to browser stores was manual. Store submission is an
external mutation and must remain visibly authorized by a user without putting
long-lived credentials in command arguments or logs.

## Decision

- After release-please runs, rebuild every open generated release branch from
  current `origin/main` instead of merging `main` into it.
- Restore only the generated files owned by that component. Merge only that
  component's version into the shared release manifest, preserve the other
  component's value from `main`, and update with an exact force-with-lease.
- Publish a newly tagged browser extension to Chrome Web Store, Edge Add-ons,
  and Firefox Add-ons only when `DCC_CUA_BROWSER_STORE_PUBLISH_READY` is true.
- Require the protected `browser-stores` GitHub Environment for every store
  job. Required reviewers provide the explicit user-authorization gate.
- Use GitHub OIDC and Google Workload Identity Federation for a short-lived
  Chrome Web Store token. Keep Edge and Firefox credentials in Environment
  secrets and pass them only through process environment variables.
- Validate the package and exact release version before Chrome or Edge upload.
  Submit Firefox's deterministic extension and source archives with the
  already-pinned WXT toolchain.
- Keep initial store ownership, listing creation, policy declarations, and
  Environment reviewer configuration as one-time human setup. Automation only
  updates the exact pre-authorized listings.
- Keep Safari outside this workflow. App Store Connect packaging and review
  have a separate Apple application, signing, and platform contract.

## Consequences

### Positive

- Independent generated release PRs remain reviewable after either component
  merges, without shared-manifest conflict commits.
- A release can reach all supported public browser stores after one visible,
  repository-owned approval gate.
- Chrome publishing does not require a stored Google private key, and no store
  secret appears in a command line.

### Negative

- Repository administrators must configure the Environment, store identifiers,
  credentials, and first listings before enabling automatic publishing.
- Store review remains asynchronous and can finish after the GitHub release.

### Neutral

- GitHub release artifacts remain available for review and deterministic
  verification even when store publishing is disabled or awaiting approval.

## References

- https://developer.chrome.com/docs/webstore/using-api
- https://developer.chrome.com/docs/webstore/service-accounts
- https://learn.microsoft.com/en-us/microsoft-edge/extensions/update/api/using-addons-api
- https://wxt.dev/guide/essentials/publishing.html
- https://developer.apple.com/documentation/safariservices/packaging-and-distributing-safari-web-extensions-with-app-store-connect
