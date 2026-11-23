# ADR 0032: Separate DCC profile discovery from exact runtime binding

## Status

Proposed

## Context

Semantic profiles now describe DCC families, versions, surfaces, and fail-closed
binding requirements. The CLI historically used selector-only matching both for
discovery (`profile match`) and for live action targets. That conflation has two
failure modes:

- discovery needs to rank candidates from native process/title/version
  evidence and may legitimately return a base profile without Host-owned exact
  PID/HWND evidence;
- executing a semantic action must not rely on a display-name or title match
  when the profile declares exact process, window, or version requirements.

## Decision

`dcc-cua-semantic-profiles` exposes two distinct predicates:

- `matches_discovery_window` uses native selectors and optional version
  evidence for catalog ranking;
- `matches_bound_window` additionally requires every Host-owned capability
  declared by `ProfileBinding`.

`ProfileCatalog::match_window` consumes discovery evidence. The CLI profile
runtime uses bound matching when live target evidence is present; selector-only
target evidence remains suitable for read-only profile inspection but cannot
claim an exact bound target. Browser URL profiles retain their exact host match.

## Consequences

- `profile match --version 2024` can select a versioned DCC profile before a
  session exists.
- Live DCC actions fail closed when the session target lacks any binding field
  required by the selected profile.
- Base profiles without versions remain compatible with discovery; versioned
  children require agreeing version evidence for both discovery and actions.
- The Host remains the only source of PID/HWND/version authority; no profile
  data grants execution authority.

## Alternatives considered

- Reusing `matches_bound_window` everywhere: safer for actions but breaks
  pre-session discovery and makes installed profiles unusable from `profile
  match`.
- Reusing `matches_window` everywhere: preserves discovery but silently weakens
  the binding contract already published in schema v3. Rejected as fail-open.

## Validation

- Profile, catalog, and CLI tests cover versioned Maya/3ds Max discovery,
  exact-window action binding, and refusal on missing or conflicting evidence.
