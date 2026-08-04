# ADR 0001: Keep deep DCC semantics in independent profiles

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

Create `dcc-mcp-cua-semantic-profiles` as a separate workspace crate. Profiles
are validated JSON data with selectors, semantic surfaces, target aliases,
preferred routes, and dialog policy. Adapters choose a profile and then use the
existing Host contracts for exact scope, fresh observations, and actions.

The Maya profile sets `dialog_style` to `os_native`; the Maya dialog surface
therefore uses the operating system native-dialog route.

## Consequences

- Core and Host remain application-neutral and retain one safety boundary.
- Profile data can evolve independently and can be extended without changing
  the wire protocol.
- A profile is a routing and vocabulary contract, not proof of authoritative
  scene state; Unreal/Fab adapters still need their native APIs and explicit
  confirmation for destructive account or download actions.
