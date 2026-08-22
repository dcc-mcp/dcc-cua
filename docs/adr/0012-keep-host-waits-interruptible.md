# ADR 0012: Keep Host waits interruptible

## Status

Accepted

## Context

The Host connection loop gave dedicated cancellation treatment to semantic and
window waits, but session-event long polls ran on the ordinary serial path.
While a poll or post-action capture delay was awaiting a deadline, the loop did
not observe the process-wide Escape generation. A same-connection Host stop
also could not be handled until the request completed.

## Decision

Route session-event polls through the existing interruptible connection lane.
That lane reads cancellation or Host-stop frames while the operation runs and
checks the shared Escape generation every 50 milliseconds. A Host stop ends
the in-flight wait with a typed `user_interrupted` response.

Post-action capture delays use the same 50-millisecond interrupt cadence. If
interrupted after input completed, the response remains `action_completed`,
records that the action was executed, marks the post-snapshot failure, and
requires a fresh observation. The Host never turns a completed mutation into a
blindly retryable generic failure.

## Consequences

- long polls no longer block physical or same-connection Host interrupts;
- delayed captures stop promptly without hiding the completed action;
- session cleanup runs before interrupted wait responses are published; and
- ordinary mutation futures are not cancelled at an unknown completion point.

## Alternatives considered

- Running every session request in an independent task was rejected because
  mutable observation and capability state must remain serialized.
- Aborting arbitrary mutation futures on Escape was rejected because driver
  completion could be unknown and a retry could duplicate input.
