# CUA SDK dependency maintenance

The root workspace owns every direct CUA dependency. Core, Host tests, native
adapters and GUI fixtures inherit those declarations. `Cargo.lock` must resolve
all upstream CUA crates to the same immutable Git source. The policy CI checks
this contract with `scripts/test_sdk_dependency_contract.py`.

## Current source

- Previous SDK: 0.22.0 at `loonghao/cua@39f8a1a976f9e87d8d28affb74a1c87368893ca5`.
- Updated SDK: 0.23.2 at `loonghao/cua@50f5d303e3b81c031ef67f1dfb7127e8aaabfff7`.
- Upstream base: `trycua/cua@aabb2082c170289256f0c8d9db4cce094c778578`.

The updated revision merges upstream into the previous compatibility branch;
it retains both parent histories and contributor attribution. It includes
upstream's embedded shutdown drain, browser debugging cleanup, capture-only
window observations, macOS backing-scale fixes and Linux Wayland fixes.
This source revision includes changes after the 0.23.2 release tag; the Git
revision, rather than the package version alone, identifies the tested SDK.

## Why the compatibility fork remains

Thirteen commits on the previous branch are absent from the upstream history.
They implement existing-profile consent and socket-liveness proofs, scoped
semantic snapshots, hidden file input associations, foreground tab activation
and navigation completion receipts, along with related tests and subprocess
handling. Do not replace the fork with upstream solely because upstream's
package version is newer.

The refresh had one textual conflict, in Windows native Chromium consent.
The merged implementation keeps target-owned duplicate-prompt selection for
allow actions and adds upstream's structural cancel action for cleanup. Both
actions use the same language-independent topology proof. Distinct cancel
candidates fail closed. The merged source tests opaque Unicode, mirrored RTL,
missing and contradictory focus, renderer lookalikes and repeated prompts.

## Updating again

1. Fetch upstream and record the immutable candidate SHA. Compare both sides
   of the fork divergence, including behavioral changes already squashed
   upstream under different hashes.
2. Integrate upstream in an isolated compatibility branch and retain necessary
   patches. Run native adapter tests for merge conflicts and browser contract
   tests for retained patches before publishing its immutable revision.
3. Change the six root dependency declarations together. Run
   `cargo update -p cua-driver-sdk`, then `cargo hakari generate` and review the
   lockfile. Avoid unrelated registry upgrades.
4. Run the dependency contract, formatting and Hakari checks; compile and test
   the entire downstream workspace with Rust 1.95.0. Verify GUI test compilation
   and the repository's exact-head native CI matrix before delivery claims.
5. Keep local tests, native GUI acceptance, release packaging and installed
   runtime verification as separate evidence. A dependency bump does not update
   an already installed executable.

Return to the official Git source once every required patch is present upstream
and the same downstream acceptance gates pass. Preserve the third-party notice
and remove obsolete compatibility comments at that point.
