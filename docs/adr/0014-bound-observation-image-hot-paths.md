# ADR 0014: Bound observation image hot paths

## Status

Accepted

## Context

Observation publication converted each full BGRA frame to a second full RGBA
buffer before PNG encoding. Portable capture decoded a driver PNG and later
re-encoded the same pixels even when no resize was requested. Host response
paths independently assembled image descriptors and attachments, and enforced
the wire limit per image instead of on the combined binary frame. Successful
Windows readiness gates also allocated a public JSON diagnostic on every
check.

These costs sit on frame-rate or per-action paths. They also made the wire
bound weaker than the frame protocol that ultimately carries the bytes.

## Decision

- Convert BGRA to RGBA one scanline at a time while streaming PNG output.
- Preserve a validated portable source PNG beside its decoded shared BGRA
  frame, reuse it for unscaled snapshots, and borrow BGRA when output
  dimensions are unchanged.
- Centralize host image transport preparation. Check the checked-sum of all
  binary attachments against the single-frame limit before allocation, move a
  lone image buffer directly, and allocate exactly once when concatenation is
  required.
- Represent a Windows desktop readiness probe as typed state. Successful gates
  evaluate booleans directly; JSON diagnostics are constructed only for the
  public diagnostic endpoint or a rejected gate.
- Retain safety revalidation across awaited work, while removing only the
  consecutive duplicate target enumeration that had no intervening await.

## Consequences

- PNG encoding uses one row-sized conversion buffer instead of a second
  full-frame buffer.
- Portable unscaled snapshots avoid decode-resize-encode round trips while
  recorders can still consume the shared decoded frame.
- A multi-image result cannot exceed the host's binary wire frame limit, and
  native, browser, and verification images share one descriptor contract.
- Common successful readiness checks avoid diagnostic JSON allocation without
  weakening desktop, input-surface, foreground, or exact-target gates.
- Multi-image binary transport still concatenates once because the current
  wire protocol carries a single attachment frame.

## Alternatives considered

- Keeping only per-image bounds was rejected because multiple individually
  valid images can overflow one binary wire frame.
- Storing only the portable PNG was rejected because live recording and scaled
  snapshots require decoded pixels.
- Caching readiness across actions was rejected because desktop and foreground
  state can change between checks.
- Removing target checks around awaited work was rejected because those checks
  enforce the exact-window safety contract rather than redundant computation.
