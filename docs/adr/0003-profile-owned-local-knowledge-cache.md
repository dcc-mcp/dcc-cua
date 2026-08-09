# ADR 0003: Profile-owned local knowledge caches

## Status

Accepted

## Context

Repeated application tasks often expose stable domain identities that are expensive to reconstruct from pixels. In The Bazaar, the game log supplies an item instance and the local database supplies its template UUID and current attributes, while community databases provide useful names, history, and aggregate references. Calling an external site for every frame wastes latency and tokens, but treating a remote record as authoritative can apply stale patch data to a live transaction.

The mechanism must remain reusable by CLI agents, MCP adapters, DCC applications, games, and local agents controlling a VM. Core must not learn application-specific databases or acquire ambient network access. A distributable Profile must not overwrite user observations or silently turn a data package into executable code.

## Requirements

- Reuse stable identities across sessions without repeated visual inspection or network lookup.
- Preserve local application state as the authority for versioned behavior and exact instances.
- Keep Core and Host application-neutral and cross-platform.
- Let package authors distribute reviewed seed knowledge without distributing a user's live history.
- Make provenance, staleness, corruption, and cache misses explicit.
- Keep all actions behind the existing TaskGrant, semantic/visual observation, PID, and HWND fences.

## Decision

A Profile companion may read an optional, versioned local knowledge cache. The package may include a reviewed seed under `knowledge/`; mutable user knowledge lives under `~/.dcc-cua/knowledge/<profile-id>/` and is never written into the installed package.

For The Bazaar, the cache key is the local `GameData.db` template UUID. A record contains the external provider, external card ID, canonical URL, canonical name, card type, source patch, verification date, and match basis. At companion startup every record is validated against the read-only local database:

- the template UUID must exist;
- the canonical name must exactly match the local template;
- the provider and URL must satisfy an allowlisted canonical form;
- provenance fields must be present.

The companion exposes a validated hit as an `externalReferences` annotation on the already-resolved local card instance. It loads the cache once and never mutates it. Local `Player.log` and `GameData.db` remain authoritative. External lookup is performed by an agent or maintenance tool only on a cache miss or explicit patch refresh, never on the hot observation path and never through an undocumented API contract.

Core understands only versioned Profile state and provenance. It does not know BazaarDB, game templates, or cache mutation policy. The same boundary permits other Profiles to use application-specific key/value stores, provided they retain exact local keys, explicit schemas, read-only runtime loading, and source provenance.

## Alternatives considered

### Query the external database on every observation

Rejected. It adds network latency and availability to the decision loop, repeats unchanged work, and can leak task context.

### Make the external card ID the primary identity

Rejected. External identifiers and patches do not prove the exact local instance or current behavior.

### Put a generic learned database in Core

Rejected. Cache schemas, validation rules, and retention are domain policy. Moving them into Core would couple the safety runtime to every application ecosystem.

### Let the companion automatically scrape and rewrite the cache

Rejected for the runtime path. Silent mutation obscures provenance, makes packaged behavior non-reproducible, and expands the companion's network and filesystem authority. A separate explicit maintenance command can be designed later.

## Failure modes and mitigations

- **Missing cache:** continue with local identity and visual inspection.
- **Unknown local template or name mismatch:** reject the cache at startup instead of partially trusting it.
- **Stale external patch:** retain the reference as dated metadata and use local attributes for decisions.
- **Provider unavailable:** reuse verified local hits and leave misses unresolved.
- **Corrupt or hostile URL:** schema and provider-specific canonical URL validation reject it.
- **User-history leakage:** packages contain only reviewed seeds; mutable records and combat history stay in the user's knowledge directory.
- **State/action race:** cache references never authorize input; Host fences and postconditions remain mandatory.

## Consequences

- Frequently seen cards become an O(1) local join after the log/database identity is known.
- Agents spend visual tokens on new or changed information instead of rereading stable descriptions.
- Profile packages can accumulate distributable, reviewable domain tuning without expanding Core.
- Cache authoring and refresh are explicit maintenance operations that still need a future CLI contract and merge policy.
