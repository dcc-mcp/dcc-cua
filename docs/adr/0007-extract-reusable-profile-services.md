# ADR 0007: Extract reusable profile services from the CLI

## Status

Accepted

## Context

The profile package store, store-aware inheritance, deterministic matching, and
identity-fenced context selection were implemented inside the CLI binary. That
made the documented embedding path unable to reproduce CLI behavior without
spawning the executable and parsing JSON output. It also left package safety
invariants coupled to argument parsing and made read-only lookups repeatedly
walk package trees.

These invariants form one domain boundary: a consumer must see a coherent set
of profiles whose package contents, platform declarations, inheritance edges,
and cross-profile fallback edges have all been validated together.

## Decision

Add the public `dcc-cua-profiles` library crate. It owns:

- strict schema-2 package parsing, bounded file inspection, and normalized path
  validation;
- staged install and replace with backup rollback, plus dependency-aware staged
  uninstall;
- store-aware inheritance resolution and cycle detection;
- referential integrity for cross-profile fallback edges;
- deterministic user-over-builtin matching and ambiguity reporting; and
- identity- and selector-fenced context document selection.

`ProfileStore` is a validated in-memory snapshot. Read-only `profile`, catalog,
matching, and context operations reuse that snapshot and never re-walk the
filesystem. External filesystem changes become visible only after an explicit
`refresh`. Refresh resolves readiness to a fixed point: a package is not ready
when its platform is unsupported or any inheritance or fallback dependency is
not ready.

The CLI remains an adapter that parses flags, calls the library, and serializes
results. The semantic profile crate continues to own the declarative data model
and built-in profiles; it does not take filesystem or package-install concerns.

## Consequences

- Embedded applications and future GUI or Host adapters can use the same typed
  contracts as the CLI.
- Package mutations fail closed and preserve the prior valid snapshot when a
  dependency would break.
- Read-only profile operations are independent of package-tree size after store
  construction or refresh.
- Callers that modify the store outside this API must explicitly refresh and
  handle invalid entries; silent live filesystem mutation is not supported.

## Alternatives considered

- Expanding the semantic profile crate was rejected because filesystem package
  management and context I/O are separate responsibilities from the data model.
- Exposing only Host RPC methods was rejected because it would still deny
  in-process embedders a reusable domain API and would duplicate validation at
  another boundary.
