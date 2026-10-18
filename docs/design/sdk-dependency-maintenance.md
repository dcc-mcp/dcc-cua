# CUA SDK dependency maintenance

The root workspace owns every direct CUA dependency. Core, Host tests, native
adapters and GUI fixtures inherit those declarations. `Cargo.lock` must resolve
all upstream CUA crates to the same immutable Git source. The policy CI checks
this contract with `scripts/test_sdk_dependency_contract.py`.

## Current source

- Previous SDK: Driver 0.28.2 at
  `loonghao/cua@3d2bcf50089ea18aa5bedab328b1afb3f710ea00`.
- Updated SDK: Driver 0.36.0 plus the required DCC compatibility patch series.
- Official release: [cua-driver-rs-v0.36.0](https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.36.0),
  commit `trycua/cua@6d6c734c127025fb39ad5d585a10382808b39dc1` (2026-10-16).
- Published compatibility source: `loonghao/cua@bf6d7d56bdecc3005c5de083c894356dd363e469`
  ([compatibility PR #7](https://github.com/loonghao/cua/pull/7)). All six root
  declarations and ten resolved SDK packages use this immutable Git source.
  Its tree matches the tested local migration candidate, with original fork
  histories retained as merge parents.

This is the Rust **Driver SDK** under `libs/cua-driver/rust`, including
`cua-driver-sdk` and `cua-driver-contract`. It is a separate component from
`libs/cua` and the independently released `cua-sdk-v0.4.4`.

The component-tagged release has native assets for Windows, Linux and macOS.
Both current official installer entry points resolve Driver 0.36.0. GitHub's
`prerelease` label keeps the monorepo-wide Latest pointer separate; the Driver
release notes identify plain SemVer Driver releases as its stable channel.
The integration pins source, not the repository-wide Latest release.

## Why the compatibility fork remains

Official Driver 0.36.0 still does not contain `scope_ancestor_role`,
`scope_anchor` or `browser_scope_unavailable`. Replacing the source URL with
upstream would silently remove a documented DCC capability. Retain only the
necessary compatibility behavior on the official release base:

- Ancestor-scoped snapshots with fail-closed scope evidence. Generic direct
  scopes and outlines retain same-process iframe descendants via renderer
  identity, while ancestor proofs keep exact frame/document boundaries.
  Advertised schemas remain Bedrock/Vertex-compatible; runtime ancestor and
  semantic-continuation checks remain enforced.
- Hidden file-input associations with unique explicit labels. Preserve valid
  associations within shadow roots and same-process iframes while refusing
  mixed labelable controls; an outer host cannot acquire an inner input.
- Explicit foreground navigation, frame/loader-bound completion receipts,
  live-target revalidation and partial/unknown completion reporting. DOM
  readback is fenced on both sides by document identity. Lost CDP replies
  invalidate refs and return an explicit unknown dispatch state; DCC preserves
  that redacted receipt instead of claiming the navigation never happened.
- Responsive existing-profile socket proof. Driver 0.36 now shares consented
  sockets: a failed probe releases only its current grant with
  `SharedSocketRelease::Close`; it must not remove another session's hold.
- Exact Windows native consent-window ownership/topmost disambiguation.
- Console-free construction of existing bounded Windows helpers.

Upstream already contains the old fresh-socket consent retry and stronger
repeated-prompt title/geometry deduplication. Preserve its newer cancellation
settlement and `ConsentNodes` COM cleanup. Additional native-window trees use
that same RAII cleanup. Drop the obsolete netstat subprocess hunk because
upstream uses the native TCP listener API, and do not restore deleted website
documentation paths. The current Driver skill retains the compatibility API
reference. The compatibility branch records the per-patch migration decisions
in `libs/cua-driver/docs/dcc-compatibility-0360.md`.

The migration also compares the unmerged fork candidates, not only the
installed pin. Preserve PR #5's duplicate-ID file-input refusal and borrowed
scroll COM ownership fix, and PR #4's exact launch-family resolution while
retaining its public discovery API. Their existing branches are not rewritten.
Windows console-visibility tests stay opt-in and explicitly selected by the
canonical native runner; this is not evidence that they ran locally.

The dependency graph adds `cua-cursor-motion` at the same immutable SDK source.
The only newly resolved registry packages are the new upstream scripting
engine and its `relative-path` dependency; existing registry versions remain
unchanged. Upstream inventory growth does not grant a new DCC action route:
`run_script` is rejected by both unscoped native entry points.

## Verification and acceptance

Keep source check, executed tests, native acceptance, publication and installed
client switching separate. Linux integration must construct the real embedded
SDK and exercise its live inventory and exact-window capture, not only mock
CDP responses. Test socket liveness both with one holder and with a peer holder.
Preserve ancestor-scope, hidden-upload, navigation and target-ownership tests.

A missing local Unix socket/AT-SPI bus is an environment limitation, not a
passing desktop test. Windows Edge setup/cleanup and macOS input require their
own exact-candidate native acceptance. Do not close those issues based on Linux
compilation or pure matcher tests. The upgrade does not install a new binary.

### Current candidate evidence (2026-10-18)

With the published `bf6d7d56` Git source and no path overrides, Rust 1.95
passed 1,507 downstream library tests (three explicit native opt-ins ignored),
64 CLI/diagnostic/MCP/process tests, and strict workspace/all-target Clippy.
The dependency, formatting, layout and generated Hakari gates also passed.

Real Linux SDK acceptance discovered two owned X11 windows, captured the exact
80-by-60 target changing red to blue, and refused a destroyed target with
`missing_window` without writing an image. AT-SPI was unavailable because the
cloud sandbox denies its Unix bus; the observation reports that degradation.
The source follow-up passed 246 browser tests (one existing opt-in ignored)
and the actual source-built Driver CLI direct-MCP protocol schema test.

This is not a claim of whole-SDK CI success: the fork's attribution baseline
gate remains tracked separately, and an additional standalone SDK-core strict
Clippy check reports existing source warnings. Live browser, native Windows,
native macOS and installed-client acceptance remain separate gates.

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
