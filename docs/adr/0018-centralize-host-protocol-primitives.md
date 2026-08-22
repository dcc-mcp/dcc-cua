# ADR 0018: Centralize Host protocol primitives

## Status

Accepted

## Context

The CLI, client, and Host independently parsed request envelopes, classified
methods, and implemented the same length-prefixed frame codec. The copies used
the same wire format but could drift in request identity validation, payload
limits, error handling, or flush behavior.

These are protocol concerns shared by every transport participant. Request
dispatch, domain authorization, and logical-task orchestration remain concerns
of their existing higher-level crates.

## Decision

- Make `dcc-cua-protocol` the canonical owner of request-envelope parsing,
  request ID validation, and closed Host method traits.
- Make the protocol crate own the asynchronous big-endian length-prefixed frame
  reader and writer, including zero-length and configured-limit validation.
- Keep transport and protocol errors typed in the protocol crate. Client and
  Host boundaries map those errors into their existing public domain errors.
- Keep request-specific schema validation in the Host and logical-task/session
  behavior in the client; neither belongs in the wire contract.

## Consequences

- CLI, client, and Host accept and reject the same envelope and frame shapes.
- Frame limits remain supplied by each channel, while their enforcement is
  identical and allocation happens only after validation.
- Higher-level public error contracts do not expose transport implementation
  details.
- Protocol changes have one implementation and one focused test surface.

## Alternatives considered

- Keeping copies with parity tests was rejected because tests would preserve
  three sources of truth.
- Moving Host dispatch and client task orchestration into the protocol crate was
  rejected because it would couple wire representation to domain behavior.
- Introducing a generic serialization framework was rejected because the
  existing four-byte frame is small, explicit, and already part of the public
  contract.
