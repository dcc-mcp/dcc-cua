# ADR 0011: Fail closed on client request cancellation

## Status

Accepted

## Context

The Host client previously allowed an async request future to be dropped while
reading or writing a length-prefixed frame. Tokio does not guarantee that
`read_exact` and `write_all` preserve a resumable frame after cancellation.
Reusing that connection could therefore interpret leftover bytes as a new
frame. Responses with unknown correlation identifiers were also buffered
without a bound, and generated requests had no first-class timeout API.

## Decision

Treat one request or read-only batch as an explicit connection operation. A
fully decoded success or typed remote error returns the connection to ready.
Timeout, task cancellation, I/O failure, malformed framing, or correlation
failure makes the connection unusable; callers must reconnect instead of
reusing uncertain stream state.

Track only request identifiers registered by the current operation, cap them
at the protocol batch limit, and reject unknown or duplicate responses. Expose
bounded request APIs that return a typed timeout error and apply the same
fail-closed transition.

## Consequences

- a cancelled mid-frame request cannot silently corrupt a later request;
- correlation storage is bounded by the negotiated operation shape;
- ordinary requests keep their compatible unbounded-wait API; and
- callers choosing a deadline must reconnect after it expires.

## Alternatives considered

- Resuming partially transferred frames was rejected because response parsing
  also spans the JSON envelope and optional binary frame, creating a larger
  persistent decoder state with no benefit to mutation safety.
- Silently discarding unknown response identifiers was rejected because it
  hides Host/client protocol divergence and can misassociate caller-owned ids.
