# ADR 0013: Test production safety lifecycles directly

## Status

Accepted

## Context

Windows test builds replaced the production interactive-desktop probe with a
portable success report. Observation gate combinators and live-observation
preflight helpers were tested in isolation while production repeated their
steps manually. The browser extension also kept pairing persistence and tab
invalidation inside its background entry point, leaving those lifecycle rules
without executable behavioral tests.

These seams allowed tests to describe the intended safety contract without
proving that production was wired to it.

## Decision

Windows test and production builds use the same native desktop probe wiring.
Deterministic tests continue to exercise pure diagnostic builders with injected
probe results, and a Windows regression test rejects the portable success path.

Observation capture, finalization, and live-observation startup use the same
gate combinators exercised by unit tests. The combinators remain responsible
for readiness checks around awaited work; session code remains responsible for
target revalidation and observation invalidation.

Browser pairing persistence is owned by a storage-injected `PairingLifecycle`.
The background entry point supplies browser session storage, while behavioral
tests supply an in-memory store and exercise restart restoration, malformed
state, exact request matching, navigation, removal, and explicit unpairing.

## Consequences

- test-only configuration can no longer silently bypass the Windows desktop
  readiness wiring;
- production observation paths and their gate-order tests share one
  implementation;
- extension worker restarts and tab lifecycle transitions are reproducible in
  fast tests; and
- capture exclusion documentation states the backend-dependent contract
  instead of promising exclusion for the desktop `BitBlt` fallback.

## Alternatives considered

- Keeping source-string assertions for background control flow was rejected
  because refactors can preserve strings without preserving behavior.
- Mocking `diagnostic()` itself was rejected because it would retain the
  production/test wiring split.
- Requiring every capture backend to hide safety indicators was rejected
  because the verified-visible desktop fallback intentionally captures the
  composed desktop and cannot prove that exclusion contract.
