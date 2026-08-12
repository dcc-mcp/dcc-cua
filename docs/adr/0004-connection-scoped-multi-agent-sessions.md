# ADR 0004: Connection-scoped multi-agent sessions

## Status

Accepted

## Context

Agent supervisors need to run independent workers against different DCC
windows without sharing stale observations, cancellation state, or authority.
The Host already accepts multiple client connections and each connection can
own multiple window, desktop, and launch sessions. That behavior was not
explicit enough in the machine manifest, and an unbounded number of sessions
on one connection could leak native runtime resources.

Sharing mutable sessions across connections was considered. It would require a
second attach token, cross-client cancellation rules, ownership transfer, and
recovery semantics. More importantly, it would allow one worker connection to
affect another worker's observations and lifecycle. That conflicts with the
existing TaskGrant and random window-capability boundary.

## Decision

- One agent thread owns one persistent Host connection.
- Sessions, capabilities, observations, waits, recordings, and cancellation
  remain private to that connection.
- Different connections may reuse a public session identifier because the
  connection and random capability token form the actual authorization scope.
- Each connection may own at most 16 active window, desktop, and launch
  sessions in total.
- Background-safe actions may progress on independent connections.
- Physical desktop input remains serialized by the host-global FIFO input
  arbiter.
- Disconnect cleanup stops only sessions owned by the disconnected client.
- The machine manifest advertises this model and its limits so supervisors can
  schedule workers without hard-coded assumptions.

## Consequences

Agent supervisors can fan work out across independent connections without a
new cross-client session-sharing protocol. A worker must reopen a session and
take a fresh observation after its connection is lost; requests are never
replayed automatically. Work that needs the physical foreground may queue or
fail closed when the exact target cannot become foreground. The fixed session
limit bounds leaked native runtime state while leaving enough capacity for a
worker to coordinate several related DCC windows.

## Rejected alternatives

### Host-wide mutable session registry

This would make reconnect and handoff easier, but it expands the authorization
surface and makes disconnect, interrupt, recording, and stale-observation
ownership ambiguous.

### One Host process per agent

This isolates workers but duplicates native runtime resources, defeats the
single endpoint contract, and cannot safely coordinate process-global input.

### Concurrent physical input

Desktop keyboard and pointer state are process-global on current platforms.
Interleaving mutations would make target verification nondeterministic, so the
Host retains one FIFO arbiter.
