# ADR 0030: Partial Occlusion and Background UIA Delivery

## Status

Accepted

## Context

Custom-rendered DCC windows can remain observable while another window covers
part of the target. Desktop pixels under the covering window cannot be treated
as target pixels. Semantic UI Automation can also operate without taking the
user's foreground focus when the target provider supports background delivery.

## Decision

Windows exact-window observation keeps three explicit routes.

- Exact WGC content capture for an HWND-bound frame.
- A verified visible desktop crop when complete z-order proof is available.
- A redacted desktop crop when partial overlap is known. Proven occluder
  rectangles are filled opaque black and published in provenance.

The redacted route remains observation-only. Coordinate input requires a fresh
unobscured point proof. Physical pointer and keyboard input use the existing
foreground and input-desktop gates.

Windows UIA actions default to background delivery. Their receipts include
`delivery_mode`, `focus_mutation_attempted`, `foreground_changed`, and
`cursor_moved`. Foreground delivery remains explicit.

## Consequences

- A smaller window can cover a DCC window while the agent continues reading
  target content that remains proven.
- When the exact WGC route is available, partial overlap may still publish
  HWND-bound content without redacted rectangles; otherwise the desktop crop
  is redacted and publishes the proven occluder rectangles.
- The same-executable WGC exception is limited to a session created from the
  requested HWND and revalidated with exact native instance, geometry, DPI,
  frame-size, and capture-generation fences; unbound WGC callers remain
  fail-closed.
- Occluded pixels cannot be mistaken for DCC pixels.
- Semantic actions can preserve the user's active application when provider
  behavior supports background execution.
- Every mutation still requires the existing exact target, authorization,
  freshness, and post-state verification contracts.

## Validation

- Windows fixture tests cover partial redaction, occluder geometry, and exact
  publication fences.
- PC8 ZBrush 2023.2.2 validation covers an overlapping window, exact WGC
  content capture, `occlusion.state=partial`, and no foreground activation.
- Background UIA receipts remain covered by platform unit tests.

