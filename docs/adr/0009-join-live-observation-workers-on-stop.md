# ADR 0009: Join live-observation workers on explicit stop

## Status

Accepted

## Context

Windows live observation runs persistent WGC capture on a blocking worker.
The public `stop()` path aborted only the async wrapper, which detached the
blocking worker and reported the stream inactive before capture had actually
stopped. The same wrapper handle was also used as the activity signal, so its
state could disagree with the worker that owned the capture resources.

## Decision

Give every live-observation instance a shared, idempotent shutdown signal.
Explicit `stop()` requests shutdown and awaits the wrapper, which in turn
joins the blocking WGC worker. The Windows worker checks the signal before and
after capture and uses an interruptible wait between attempts. Portable
capture races both in-flight capture and retry waits against the same signal.

Keep the wrapper alive until its worker exits so `is_active()` reflects the
worker lifecycle. `Drop` cannot await, but it requests cooperative shutdown
before aborting the wrapper as a best-effort fallback.

## Consequences

- a successful explicit stop acknowledges that the capture worker released
  its resources;
- the activity signal no longer becomes false merely because its wrapper was
  aborted; and
- Windows stop latency can include completion of the bounded in-flight WGC
  frame wait, but never reports completion while that worker remains active.

## Alternatives considered

- Aborting the wrapper and relying on the frame receiver to close was rejected
  because a detached blocking task still owns its sender.
- Spawning a second cleanup task from `stop()` was rejected because it would
  preserve the disagreement between reported and actual lifecycle state.
