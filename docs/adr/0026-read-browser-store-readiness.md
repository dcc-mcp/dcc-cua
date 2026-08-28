# ADR 0026: Read browser-store readiness before publication

## Status

Accepted.

## Context

Browser-store publication already requires the `browser-stores` environment and
`DCC_CUA_BROWSER_STORE_PUBLISH_READY`. First-item onboarding and browser security
challenges are not safe to infer from credentials or a successful package build.
They remain dependent on the exact foreground delivery in PR #223 and the bounded
human challenge handoff in Issue #224.

Environment eligibility includes both branch scope and the human approval
boundary. A branch-only receipt can remain green after required reviewers are
removed or changed, self-review is enabled, or administrator bypass is allowed.

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

Environment eligibility is bound to the artifact producer's exact `main`
branch. The repository default branch must be exactly `main`, the `main` branch
must be freshly observed, and the `browser-stores` deployment policy must be a
custom policy containing exactly one branch entry whose exact name is `main`.
The policy query is bounded and its declared count must equal the returned
entries. Missing or renamed defaults, missing `main`, all-branch policy,
protected-branch-only policy, non-`main` entries, tags or patterns, multiple or
truncated entries, malformed responses, and unknown policy state fail closed.
Protected-branch-only policy is not accepted because it cannot prove that only
`main` is eligible and leaves protected-branch scope and administrator bypass
semantics ambiguous.

The environment protection contract is version 1 and is closed. The fresh
Environment response must contain exactly one `required_reviewers` rule and one
`branch_policy` rule, with no other rule types or rule fields. Reviewer wrappers
contain exactly `type` and `reviewer`; every reviewer is a unique GitHub `User`
or `Team` with a positive signed 64-bit numeric ID and a non-empty node ID.
`prevent_self_review` must be exactly `true`, and the Environment's
`can_admins_bypass` field must be exactly `false`. Missing, malformed,
truncated, duplicated, additional, or unknown rules and fields fail closed.

Expected reviewer identities are repository configuration, not public source.
The `browser-stores` Environment owns the
`DCC_CUA_BROWSER_STORE_ENVIRONMENT_PROTECTION_V1` Actions variable. Its value is
`sha256:` followed by the SHA-256 of compact, key-sorted JSON containing
`version: 1`, the exact reviewer `type` and numeric `id` pairs sorted by type and
ID, `prevent_self_review: true`, and `can_admins_bypass: false`. The variables
query is bounded to one complete 30-entry page and declared count must equal the
returned entries. Missing, duplicate, malformed, or truncated contract state
fails closed. Receipts expose only the contract version, validity, stable reason,
reviewer count, and the two protection booleans; reviewer identities and contract
digests are never serialized.

Receipts expose configuration names and presence only. Provider messages,
credentials, tokens, publisher/item identifiers, and account details are never
serialized. Store states are classified independently per provider: Chrome
accepts only its `published` readback, Edge accepts only `in_store`, and AMO
accepts only `public` or `unlisted`. Known transitional states are not ready,
and a state from another provider's vocabulary is unknown and fails closed.
Permission denial, missing items, identity drift, and expired artifacts also
fail closed. Ordinary pull requests and pushes execute mock contract tests only
and never call external store APIs.

Action-pin evidence is read from the parsed YAML job and step mappings, never
from text matching. Remote actions and reusable workflows require a full commit
SHA, Docker actions require a SHA-256 digest, and repository-local actions remain
source-bound to the checked-out commit. Local and remote repository paths reject
empty, `.` and `..` segments, while valid local actions use the documented
`./.github/actions/<action>` form. Each local action must exist with exactly one
regular `action.yml` or `action.yaml`. The audit rejects symbolic links, Windows
junctions or reparse points in every repository-local component, requires strict
canonical containment in the checkout, and compares physical path identities
before and after inspection so replacement drift fails closed. Duplicate
mappings, aliases, anchors, dynamic expressions, missing parser support, and
malformed workflow structures fail closed; comments and scalar decoys are not
executable action evidence.

The CI workflow contract freezes the pull-request trigger, top-level permissions
and defaults surface, GitHub-hosted runner, policy-job execution surface, and
ordered receipt-test command. This proves that the receipt suite is executable
in that workflow. The policy job exact-compares every complete parsed step
mapping, including normalized multiline command bodies, and binds the exact
checkout, Rust toolchain, component, and installer inputs. Added, removed,
reordered, decoyed, or modified steps, command bodies, execution modifiers, and
action inputs fail the receipt contract. This does not claim GitHub branch
enforcement. At the time of this decision the repository has no branch
protection, ruleset, or required check for the default branch, so
`branch_required` remains false unless a live GitHub policy observation
explicitly proves otherwise.

## Consequences

An all-three-ready injected dry readback proves the receipt contract, but live
readiness remains false until exact item onboarding and an authoritative Edge
readback are available. This ADR does not authorize browser UI, login, CAPTCHA,
upload, submission, publication, or any configuration mutation.
