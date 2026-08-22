# ADR 0010: Separate indicator rendering failures from safety failures

## Status

Accepted

## Context

The Windows control banner used one fatal error channel for target identity,
Escape-hook liveness, presenter startup, and later cosmetic rendering calls.
A transient property, opacity, shape, or positioning failure therefore stopped
the presenter and made the Core session fail even though the exact target and
operator stop boundary were still valid.

## Decision

Add a typed `rendering` failure kind. Initial presentation must still succeed
before a session starts, but rendering failures after readiness are recorded
as degraded diagnostics while the presenter loop and Escape boundary continue
running. Failed dynamic state is not committed, so later loops can retry it.

Target loss, Escape-hook loss, presenter startup failure, and unexpected thread
termination remain session-fatal. Core ignores only the explicitly non-fatal
rendering kind when checking an active banner.

## Consequences

- transient visual-system failures no longer end an otherwise safe session;
- status remains auditable through the typed degraded rendering failure; and
- a session is never admitted without a successfully created and positioned
  initial presenter.

## Alternatives considered

- Treating every Win32 presenter error as fatal was rejected because cosmetic
  calls do not invalidate target identity or the stop boundary.
- Ignoring rendering errors entirely was rejected because embedders need
  durable diagnostics when the visible presenter is degraded.
