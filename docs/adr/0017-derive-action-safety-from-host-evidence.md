# ADR 0017: Derive action safety from Host evidence

## Status

Accepted

## Context

The Host selected hard-deny and confirmation behavior from the request's free
form `intent`. A caller could label a mutation `ordinary_edit` to bypass action
confirmation or choose a hard-deny label for an otherwise identical action.
The Host already owns fresh accessibility elements with published policy tiers,
but did not use that evidence for authorization.

## Decision

- Treat `intent` as descriptive audit context only. It never selects an
  authorization tier.
- For semantic actions, resolve the exact index/token pair inside the latest
  accessibility state and use its closed `policy_tier` value.
- Fail closed as `hard_deny` when semantic evidence, element identity, or the
  published tier is absent or unknown.
- On Windows, when a combined upstream snapshot lacks closed policy tiers, pair
  its exact-window pixels with a fresh exact-PID/HWND Windows UIA semantic
  snapshot before publishing action evidence. If that adapter cannot publish a
  closed tier, keep semantic actions fail-closed; never infer a tier from labels.
- Derive raw-input policy from the actual action. Pointer movement and scrolling
  stay within the task grant; clicks, drags, text, and keyboard mutations require
  trusted action-time confirmation because raw coordinates contain no semantic
  target evidence.
- Treat both `action_confirmation` and `pre_approval` evidence as requiring the
  existing trusted confirmation boundary. The Host does not invent approval
  from an unverified client assertion.

## Consequences

- Changing `intent` cannot weaken or strengthen the same concrete action.
- Semantic destructive and sensitive controls retain their backend-published
  policy after crossing the Host boundary.
- Ambiguous raw mutations fail closed unless the task explicitly grants and a
  trusted confirmation host approves the exact action/evidence digest.
- Existing callers that relied on `ordinary_edit` to bypass confirmation must
  use semantic task-grant evidence or the trusted confirmation contract.

## Alternatives considered

- Validating a larger vocabulary of client intents was rejected because the
  caller would still be asserting its own authorization tier.
- Treating every raw action as task-granted was rejected because coordinates do
  not identify whether the selected control is destructive or sensitive.
- Reclassifying unknown semantic tiers as task-granted was rejected because it
  would recreate the original fail-open behavior.
