# ADR 0015: Separate interrupt state from the safety indicator

## Status

Accepted

## Context

The process-wide cooperative stop generation was implemented and exported by
the safety-indicator crate. Core observation loops and Host request handling
therefore depended on a presentation component to coordinate runtime control.
The Windows UI Automation fallback also accepted a bare element index for
mutation even though indexes are meaningful only inside one current snapshot.

These boundaries made a UI component the owner of non-visual process state and
allowed callers to bypass the snapshot token that proves element freshness.

## Decision

- Own the process-local interrupt generation in a dependency-free
  `dcc-cua-interrupt` crate. The Host, core runtime, and safety indicator use
  that contract directly.
- Keep the interrupt primitive process-local and cooperative. It is not a wire
  protocol, durable event log, or cross-process cancellation mechanism.
- Require a current snapshot-scoped element token for Windows UI Automation
  mutation. A bare numeric index is descriptive output only and is never an
  authorization-capable locator.

## Consequences

- Runtime control no longer depends on indicator rendering or platform UI.
- Headless and visual producers share the same cooperative stop semantics.
- Windows semantic mutations fail closed when token freshness is absent, even
  if an index happens to be in range.
- Existing callers may continue to include an index for diagnostics, but the
  token is the sole mutation locator.

## Alternatives considered

- Re-exporting the generation from the indicator was rejected because it would
  preserve the misleading ownership boundary.
- Moving the generation into the wire protocol crate was rejected because the
  state is process-local runtime infrastructure, not serialized protocol data.
- Accepting an index when a token is omitted was rejected because it cannot
  prove that the caller observed the current snapshot.
