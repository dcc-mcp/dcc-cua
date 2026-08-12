# ADR 0005: Generic profile artifacts and identity-fenced context

## Status

Accepted

## Context

CUA profiles must serve games, DCC applications, office documents, browsers,
and other ecosystems. A game-specific catalog, character, or time-period model
cannot represent a workbook template, presentation theme, plugin ABI, policy
revision, or arbitrary application data identity.

## Decision

Profile package schema 2 replaces untyped contents and capabilities with typed
`artifacts` and a required `requires.dcc_cua` version contract. Exactly one
`semantic_profile` artifact is required. An `agent_skill` is optional.

`context_index` artifacts contain generic documents. Each index entry declares
an ID, relative path, zero or more identities, and zero or more selectors. The
document repeats the entry identities as its `fences`. The CLI accepts repeated
`--identity namespace=value` and `--selector key=value` arguments, compares all
keys and values exactly and case-sensitively, and loads every matching document.
The command fails closed on duplicate document IDs, an ownership mismatch, or
any discrepancy between index identities and document fences.

Mutable documents use the same schema at
`~/.dcc-cua/knowledge/<profile-id>/index.json`. No domain aliases exist in the
generic CLI contract. It performs no network access and grants no input rights.

## Consequences

- One package and context contract supports PowerPoint, Excel, games, DCCs, and
  future application ecosystems.
- Publishers can add optional documentation, fixtures, skills, and companion
  source without turning those files into runtime requirements.
- Consumers receive deterministic context or an explicit refresh requirement;
  conflicting knowledge never wins by implicit precedence.
