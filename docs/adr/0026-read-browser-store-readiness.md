# ADR 0026: Read browser-store readiness before publication

## Status

Accepted.

## Context

Browser-store publication already requires the `browser-stores` environment and
`DCC_CUA_BROWSER_STORE_PUBLISH_READY`. First-item onboarding and browser security
challenges are not safe to infer from credentials or a successful package build.
They remain dependent on the exact foreground delivery in PR #223 and the bounded
human challenge handoff in Issue #224.

The environment currently permits protected branches only, while the default
branch is not protected. A job that enters the environment cannot explain that
mismatch because GitHub rejects it before steps run.

## Decision

Keep publication disabled and add a manual, two-phase, read-only readiness check.

1. Outside the environment, recapture the repository numeric identity, latest
   extension release and tag source, workflow run/head, artifact numeric ID,
   server digest, repository ownership, and expiry. Check the environment branch
   policy and all remote Action SHA pins. Emit a redacted receipt even when the
   default branch is ineligible. Every external database identity is valid only
   in the positive signed 64-bit domain (`1` through `2^63 - 1`); JSON booleans,
   zero, negative values, and overflow fail closed.
2. Only after eligibility is proven, enter `browser-stores` and use GET-only
   provider readbacks. Chrome uses its read-only OAuth scope. Firefox uses a
   short-lived JWT to read the fixed manifest GUID. Its authenticated profile identity
   must match the add-on author record, binding control to that same account. The
   documented Edge Update API has no non-mutating product/version
   lookup, so Edge remains `human_action_required` rather than manufacturing an
   upload or publish call.

Receipts expose configuration names and presence only. Provider messages,
credentials, tokens, publisher/item identifiers, and account details are never
serialized. Unknown states, permission denial, missing items, identity drift,
and expired artifacts fail closed. Ordinary pull requests and pushes execute
mock contract tests only and never call external store APIs.

The CI workflow contract freezes the pull-request trigger, top-level permissions
and defaults surface, GitHub-hosted runner, policy-job execution surface, and
ordered receipt-test command. This proves that the receipt suite is executable
in that workflow; it does not claim GitHub branch enforcement. At the time of
this decision the repository has no branch protection, ruleset, or required
check for the default branch, so `branch_required` remains false unless a live
GitHub policy observation explicitly proves otherwise.

## Consequences

An all-three-ready injected dry readback proves the receipt contract, but live
readiness remains false until exact item onboarding and an authoritative Edge
readback are available. This ADR does not authorize browser UI, login, CAPTCHA,
upload, submission, publication, or any configuration mutation.
