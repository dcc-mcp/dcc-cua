# ADR 0002: Compose CUA with read-only profile state sources

## Status

Accepted

## Context

Pixels answer where the UI is, but they are a lossy authority for application
state. A DCC scene graph, a game card instance, and a browser download job all
contain identities, attributes, relationships, and state transitions that
cannot be reconstructed reliably from one frame. Higher frame rate improves
acquisition latency but does not create those semantics.

The product must remain CLI-first and usable without MCP. The same profile
package must also compose with DCC-MCP, a local agent controlling a VM, or a
non-DCC agent platform. Profiles are user-distributed data, so they cannot be
allowed to launch arbitrary commands, carry credentials, or create a second
action channel around the Host's TaskGrant and PID/HWND fences.

## Requirements

- Preserve exact-window CUA, recording, interruption, and visible control.
- Read stable application state with instance identity, version, and freshness.
- Work through the CLI; MCP remains an optional transport adapter.
- Keep Core and Host application-neutral and cross-platform.
- Let an unavailable semantic companion degrade to visual CUA.
- Make unchanged state cheap to detect for repeated tasks.
- Treat action execution as a separate, explicitly authorized capability.

## Decision

Profile schema version 2 adds `state_sources`. The first supported source is
`loopback_http_json` with these constraints:

- `mode` is always `read_only`;
- the URL must use plain HTTP, an explicit port, and the literal loopback host
  `127.0.0.1` or `::1`;
- credentials, arbitrary headers, query strings, fragments, redirects, process
  launch commands, and action endpoints are not part of the manifest;
- response time and bytes are bounded;
- the payload exposes an expected schema version through a JSON Pointer;
- the payload exposes a monotonic semantic tick through a JSON Pointer;
- ETag is optional and used with the tick to avoid reprocessing unchanged state.

The CLI command is:

```text
dcc-cua profile-state --profile-file <package>/profile.json [--source ID] [--etag ETAG]
```

It returns the original JSON state with source, schema, tick, and ETag
provenance. An optional unavailable source returns a structured
`degraded: true` result with `fallback: visual_cua`.

The package boundary is:

```text
~/.dcc-cua/profiles/<id>/
  profile.json     deterministic routing and state-source contract
  SKILL.md         agent reasoning, task policy, and domain evaluator
  fixtures/        versioned state and decision regression cases
  README.md        human installation and trust description

~/.dcc-cua/config/profiles/<id>/
  companion.json  mutable user paths, observations, and local policy

~/.dcc-cua/knowledge/<id>/
  ...             mutable learned identities and task history
```

Package replacement owns only `profiles/<id>`. Mutable configuration and
knowledge are separate lifecycle authorities and must survive install,
upgrade, downgrade, and uninstall operations.

Core continues to own observations, shared memory, PID/HWND revalidation,
TaskGrant authorization, action fences, interruption, indicator, theme, and
recording. The semantic-profile crate owns the manifest and its validation.
The CLI owns the read adapter. A profile's Skill or trusted companion owns
application-specific graph construction and candidate scoring.

For DCC applications, a future Host-managed typed-state provider can implement
the same schema/tick/provenance result without teaching profile JSON how to
spawn an adapter. For browser or game companions, the loopback source supplies
state while visible input still goes through DCC CUA unless an independently
authorized typed action provider is registered.

## Alternatives considered

### Faster screenshots only

Rejected as the primary solution. It reduces frame age but still loses stable
instance IDs, hidden attributes, and positional trigger semantics.

### Put application parsers in Core

Rejected. Every DCC, browser, and game would expand the safety runtime and its
release surface.

### Allow profiles to run arbitrary commands or scripts

Rejected. A distributable JSON manifest would become code execution.

### Reuse MCP as the only semantic transport

Rejected. It would force configuration on CLI-only agents and local VM
controllers. MCP should adapt to the same capability, not own it.

### Expose companion actions through the read source

Rejected. Read freshness and action authority have different trust boundaries.
Typed actions may be added only through Host registration, TaskGrant policy,
fresh semantic/visual fences, and explicit postconditions.

## Failure modes and mitigations

- **Companion unavailable:** optional sources degrade to visual CUA.
- **Stale or incompatible payload:** schema mismatch or missing tick is an error;
  no state is silently accepted.
- **Oversized or slow payload:** byte and timeout bounds abort the read.
- **Loopback escape:** literal host validation and disabled redirects prevent a
  profile from reaching remote services.
- **Repeated unchanged work:** ETag and semantic tick let the caller skip graph
  reconstruction.
- **State/action race:** future typed actions must carry the semantic tick and
  still satisfy the Host observation and target fences.

## Consequences

- A profile package can be reused by CLI agents, MCP adapters, and non-DCC agent
  platforms without changing its machine contract.
- Domain quality moves from repeated visual guessing to versioned state plus a
  profile-owned evaluator.
- A companion is optional; visual CUA remains the universal fallback.
- Schema v1 is intentionally unsupported because the project has not shipped a
  stable public profile format.
- Action integration remains a separate ADR and implementation slice.
