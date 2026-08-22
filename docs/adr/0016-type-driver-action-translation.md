# ADR 0016: Type driver action translation

## Status

Accepted

## Context

Window and desktop routes independently translated the same public action into
driver JSON. Both smuggled the selected tool through a reserved `_tool` field
and removed it immediately before dispatch. Drag translation kept only the
first and last points even though the public action can carry a waypoint path.

Duplicated translation allowed the routes to drift, the sentinel weakened the
boundary between dispatch metadata and driver arguments, and intermediate drag
points could be discarded without an error.

## Decision

- Translate both scopes through one function that returns a typed
  `DriverActionCommand` containing a tool name and a JSON argument object.
- Keep window-only identity, delivery, semantic locator, and coordinate focus
  fields explicit inside the shared translator; desktop scope remains explicit
  in the emitted arguments.
- Reject a driver-routed drag with more than two path points because the pinned
  upstream driver schema accepts endpoints only. Native routes that explicitly
  support waypoint paths remain independent.

## Consequences

- Tool identity can no longer collide with or leak into driver arguments.
- Window and desktop routes share click, drag, scroll, text, key, and shortcut
  translation semantics.
- Unsupported drag fidelity fails before dispatch instead of appearing to
  succeed with a different path.

## Alternatives considered

- Retaining two translators with parity tests was rejected because it would
  test duplication rather than remove it.
- Adding an undocumented `path` field to driver JSON was rejected because the
  pinned upstream schema would not consume it.
- Continuing to use `_tool` as a temporary field was rejected because dispatch
  metadata is not part of the driver argument contract.
