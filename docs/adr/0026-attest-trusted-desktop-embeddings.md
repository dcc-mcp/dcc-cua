# ADR 0026: Attest trusted desktop embeddings

## Status

Accepted

## Context

The inline task card can mint a bounded no-popup authorization only from an
authenticated embedding surface. Process names, command-line arguments,
environment variables, stdin, and install directories are client-controlled
claims and cannot establish that surface. Windows desktop hosts also use two
different distribution models: packaged applications expose an OS package
identity, while signed Electron applications may not.

## Decision

- Keep the immediate MCP parent as the trust boundary. Do not walk through a
  shell, generic runtime, or arbitrary process chain to find an allowed name.
- Attest packaged Codex and Claude parents by exact executable name and exact
  package family returned for the live process.
- Attest unpackaged CodeBuddy CN and WorkBuddy parents only after silent
  `WinVerifyTrust` validation of the executable, then match the verified signer
  publisher, signed `ProductName`, and executable name against a closed registry.
- If a process has a package identity, validate that identity and never
  downgrade a package mismatch to Authenticode.
- Return only a stable host label to the task broker. Do not expose certificate
  details, paths, command lines, or reusable authorization capabilities.

## Consequences

- The four desktop embeddings can render the same one-time authorization card
  and execute the exact approved task without per-action native popups.
- A copied or renamed binary from the same publisher is rejected when its signed
  product identity does not match.
- Certificate and package publisher changes require a reviewed registry update;
  missing metadata, offline trust failures, and unsupported launch chains fail
  closed.

## Alternatives considered

**Allow executable names or install paths**

Rejected because both are writable claims and signed binaries can be renamed.

**Trust any binary from an approved publisher**

Rejected because one publisher can sign many unrelated products.

**Let the model select an embedding label**

Rejected because labels are attestation output, not client input.

## References

- [ADR 0024: Bridge trusted user input to task authorization](0024-bridge-trusted-user-input-to-task-authorization.md)
- [Issue #184](https://github.com/dcc-mcp/dcc-cua/issues/184)
