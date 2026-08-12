# ADR 0005: Profile startup context and seasonal playbooks

## Status

Accepted

## Context

Application control sessions repeatedly rediscover stable rules and layout,
while versioned games also need strategies that expire when their catalog
changes. Putting those facts in the generic Host would couple its safety
contract to application databases. Putting mutable strategy inside
`profile.json` would also force semantic schema churn and package replacement
for every season.

## Decision

Profile packages may declare `startup_context` and ship bounded, declarative
seed documents under `knowledge/`. Stable rules and a relative UI atlas live in
`base-rules.seed.json`. Seasonal playbooks are indexed by exact catalog content
ID and hero. Mutable generated playbooks live under
`~/.dcc-cua/knowledge/<profile-id>/playbooks/` and take precedence over seeds.

`dcc-cua profile context` is a read-only startup gate. It validates package
ownership, schema version, normalized paths, size limits, profile identity, and
the catalog fence. It never fetches the network, launches a generator, or
authorizes input. A missing or mismatched playbook returns base rules with
`requiresRefresh=true`.

The application data tool owns deterministic playbook generation from local
data and reviewed supplements. The Host continues to own authorization,
PID/HWND scope, observation fences, interruption, capture, and input delivery.

## Consequences

- New sessions can load stable rules before spending visual observations.
- Seasonal advice cannot silently cross a catalog boundary.
- Existing profile and package schemas remain compatible.
- Internet research stays in an explicit maintenance workflow with provenance,
  never in gameplay or the generic CUA runtime.
