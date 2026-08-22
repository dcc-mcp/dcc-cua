# ADR 0019: Type tool result status

## Status

Accepted

## Context

`ComputerUseToolResult` carried success or rejection only inside its open-ended
JSON payload. Core and Host code wrote a `success` field and later parsed that
field back to decide the public result channel. A missing field defaulted to
success, while a contradictory field could disagree with the Rust `Result` and
`degraded` channels.

Open-ended extension tool content must remain representable, but lifecycle
meaning must not depend on inspecting that content.

## Decision

- Add the closed `ComputerUseToolStatus` enum with `succeeded` and `rejected`
  variants to every `ComputerUseToolResult`.
- Set status at the typed boundary that knows whether the operation succeeded,
  including partial or rejected Windows input delivery.
- Derive CLI and Host `success` fields from the enum and overwrite any
  conflicting payload field at the public boundary.
- Keep the raw JSON value only as extension content; Core and Host control flow
  must not recover status from it.

## Consequences

- Embedders receive one explicit machine-readable status on every tool result.
- Missing or contradictory JSON cannot silently turn a rejection into success.
- Open-ended tool payloads remain forward compatible without weakening the
  typed result channel.
- Existing producers must choose a status when constructing a result.

## Alternatives considered

- Treating every `Ok` as success was rejected because several bounded input
  paths truthfully return partial or rejected delivery evidence.
- Inferring status from `degraded` was rejected because degraded success is a
  valid outcome.
- Removing open-ended JSON entirely was rejected because extension tools do not
  share one closed payload schema.
