# CUA SDK dependency maintenance

The root workspace owns every direct CUA dependency. Core, Host tests, native
adapters and GUI fixtures inherit those declarations. `Cargo.lock` must resolve
all upstream CUA crates to the same immutable Git source. The policy CI checks
this contract with `scripts/test_sdk_dependency_contract.py`.

## Current source

- Previous SDK: 0.23.2 at `loonghao/cua@50f5d303e3b81c031ef67f1dfb7127e8aaabfff7`.
- Updated SDK: 0.28.2 at `loonghao/cua@3d2bcf50089ea18aa5bedab328b1afb3f710ea00`.
- Upstream base: official release tag `cua-driver-rs-v0.28.2`, commit
  `trycua/cua@fc188250b4ca8549b8e61f937fdb1fb560770e86` (2026-09-15).
- Upstream `main` at evaluation time: `trycua/cua@83f142c4290a0f7d9ed545ae8532858c6e4f8145`
  (2026-09-18), 15 commits ahead of the release tag.

The updated revision replays the previous fork-only delta verbatim on top of
the official upstream release tag, instead of merging upstream into the old
compatibility branch. The delta is unchanged: 28 files, 4212 insertions and
259 deletions. Because the base is an upstream release tag rather than a
merge of two histories, the package version now identifies the tested SDK
directly and the fork delta stays reviewable as a rebaseable patch series.

Upstream commits between the previous base and this release bring the driver
to 0.28.2; the replayed fork delta carries no upstream content of its own, so
no upstream change is dropped or duplicated.

## Why the compatibility fork remains

Twelve commits (thirteen on the old branch, minus one upstream-sync merge) are
absent from the upstream history. They implement existing-profile consent and
socket-liveness proofs, ancestor-scoped semantic snapshots, hidden file input
associations, foreground tab activation and navigation completion receipts,
along with related tests and subprocess handling. Do not replace the fork with
upstream solely because upstream's package version is newer.

The decisive check is the browser contract surface that dcc-cua depends on:
`scope_ancestor_role` (a `get_browser_state` parameter), `scope_anchor`
(snapshot result evidence) and the `browser_scope_unavailable` refusal code.
Neither `cua-driver-rs-v0.28.2` nor upstream `main` mentions any of them, so
moving the pin to upstream would silently remove a documented dcc-cua
capability. Re-run that check before proposing a return to upstream.

The replay hit two conflicts. One was textual, in the driver core module list.
The other was semantic, in Windows native Chromium consent: upstream had
refactored the structural action collector, so the fork's target-owned window
proof was re-applied on top of upstream's shape. The result keeps
target-owned duplicate-prompt selection for allow actions and upstream's
structural cancel action for cleanup. Both use the same language-independent
topology proof. Distinct cancel candidates fail closed. The result tests
opaque Unicode, mirrored RTL, missing and contradictory focus, renderer
lookalikes and repeated prompts.

Two fork commits that only added and then reverted a cua-repo CI diagnostic
step were dropped; they cancel out and have no effect on the published crates.

## Updating again

1. Fetch upstream and record the immutable candidate SHA. Compare both sides
   of the fork divergence, including behavioral changes already squashed
   upstream under different hashes.
2. Integrate upstream in an isolated compatibility branch and retain necessary
   patches. Run native adapter tests for merge conflicts and browser contract
   tests for retained patches before publishing its immutable revision.
3. Change the six root dependency declarations together. Run
   `cargo update -p cua-driver-sdk` (plus the other five pins), then
   `cargo hakari generate` and review the lockfile. Avoid unrelated registry
   upgrades.
4. Run the dependency contract, formatting and Hakari checks; compile and test
   the entire downstream workspace with Rust 1.95.0. Verify GUI test compilation
   and the repository's exact-head native CI matrix before delivery claims.
   Note that `cargo hakari verify` currently reports pre-existing failures that
   are unrelated to the pin; capture its output before and after so a bump is
   not blamed for them.
5. Keep local tests, native GUI acceptance, release packaging and installed
   runtime verification as separate evidence. A dependency bump does not update
   an already installed executable.

Return to the official Git source once every required patch is present upstream
and the same downstream acceptance gates pass. Preserve the third-party notice
and remove obsolete compatibility comments at that point.
