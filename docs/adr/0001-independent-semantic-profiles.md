# ADR 0001: Keep deep DCC semantics in independent profiles

> Amended by [ADR 0002](0002-compose-cua-with-read-only-profile-state-sources.md):
> schema v3 profiles may declare bounded read-only state sources. Core remains
> application-neutral and actions remain fenced by the Host.

## Status

Accepted

## Context

The generic CUA Host already owns exact-window identity, observations, action
fencing, permissions, and transport. Unreal, Maya, and Fab need richer
application vocabulary, but those semantics have different authorities: typed
Unreal APIs, accessibility/UI Automation, and browser DOM routes respectively.
Putting those rules in Core would couple the safety runtime to application
details and make every new DCC a host-protocol change.

## Decision

Create `dcc-cua-semantic-profiles` as a separate workspace crate. Profiles
are validated JSON data with selectors, semantic surfaces, target aliases,
preferred routes, and dialog policy. Adapters choose a profile and then use the
existing Host contracts for exact scope, fresh observations, and actions.

The Maya profile sets `dialog_style` to `os_native`; the Maya dialog surface
therefore uses the operating system native-dialog route.

Profile schema v3 separates three identities that must not be conflated:

- `schema_version` changes only when the JSON contract changes;
- `profile_version` is the SemVer release of the declarative Profile and must
  equal its package manifest version;
- `application.family` and `application.versions` describe the host family and
  versions matched by that Profile.

An optional `extends` reference contains one parent Profile ID and a SemVer
requirement. Resolution is deterministic and single-parent: child selectors
replace inherited selectors when present; state sources and surfaces merge by
stable ID; targets inside an overridden surface also merge by stable ID. Child
values win. Runtime consumers receive the resolved, flattened Profile so an
Agent never spends tokens merging inheritance. Inheritance never carries task
grants, credentials, PID/HWND scope, observations, or execution authority.

`maya-2024` demonstrates the contract: it matches only Maya 2024 and inherits
the reusable Maya selectors, surfaces, and targets from `maya@^1.0`.

Profile selection is deterministic but conservative. Given an observed native
application name and window title, a Profile whose `application.versions` is
non-empty ranks above a matching family Profile. A narrower version set ranks
above a broader set. If more than one candidate remains equally specific, the
resolver reports ambiguity and requires the Agent or user to choose an ID; it
does not infer authority from display names or installation order.

## Consequences

- Core and Host remain application-neutral and retain one safety boundary.
- Profile data can evolve independently and can be extended without changing
  the wire protocol.
- Common application vocabulary is reused across host-version Profiles without
  copying it, while a parent SemVer constraint prevents silent drift.
- BCP-47 keyed window-title and target-name aliases extend one stable profile
  across UI languages without duplicating its routes or target IDs.
- A profile is a routing and vocabulary contract, not proof of authoritative
  scene state; Unreal/Fab adapters still need their native APIs and explicit
  confirmation for destructive account or download actions.
