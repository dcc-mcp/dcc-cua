# CUA SDK dependency maintenance

The root workspace owns every direct CUA dependency. Core, Host tests, native
adapters and GUI fixtures inherit those declarations. `Cargo.lock` must resolve
all nine CUA packages to the same immutable Git source. The policy CI checks
this contract with `scripts/test_sdk_dependency_contract.py`.

## Current source

- Previous SDK: 0.28.2 at `loonghao/cua@3d2bcf50089ea18aa5bedab328b1afb3f710ea00`.
- Candidate SDK: 0.33.2 at
  `loonghao/cua@7ce18efd2489c29244235481fb547e013f0f4c14`.
- Official upstream base:
  `trycua/cua@c82d32e3e1adbc6578a148962002ddf6e3e8a15a`.
- Compatibility source review: [loonghao/cua#5](https://github.com/loonghao/cua/pull/5),
  a draft pull request.

The compatibility series is replayed over the newer official upstream base. The
pinned candidate includes the consent-matcher correction and source formatting
and lint corrections for that tree. Record the actual current diff when reviewing
a future replay; file and commit counts from the previous 0.28.2 replay do not
describe this candidate.

The selected 0.33.2 release has separately recorded channel facts:

- Release API: `prerelease=true`.
- Release body: describes 0.33.2 as stable.

The selected version is frozen for this source evaluation.

The downstream package version remains 1.9.3. A source pin, a portable candidate
bundle identifier and an installed executable are distinct identities. A source
update does not replace an installed executable or an already running Host.

## Why the compatibility fork remains

DCC-CUA relies on the ancestor-scoped browser contract: `scope_ancestor_role`
(a `get_browser_state` parameter), `scope_anchor` (snapshot result evidence) and
the `browser_scope_unavailable` refusal code. The compatibility series also
retains existing-profile consent and socket-liveness proofs, hidden file input
associations, foreground tab activation and navigation completion receipts,
with related tests and subprocess handling.

The Windows consent change must preserve target-owned prompt selection and
language-independent topology evidence. Distinct cancel candidates fail closed.
Review matcher fixtures together with the native adapter; pure fixtures do not
prove native window, UI Automation, desktop access or existing-profile consent.

Do not switch Git sources solely because upstream's version is newer. Check the
current upstream contract and retained behavior directly before proposing that
switch. The previous evaluation's claims about upstream `main` or an older tag
are not evidence about the current upstream base.

## Acceptance gates

The immutable six root pins and nine lock sources must agree. Validate both that
source contract and the resolved package versions. Review normal Cargo resolver
changes and genuine Hakari output without relaxing the existing Hakari settings
or dependency policy.

Source compilation and pure contract tests provide bounded evidence. The draft
source workflow keeps its strict compilation checks and compiles the test
inventory; completing those checks does not establish executed native behavior.
The exact-head native CI matrix, Windows consent and desktop behavior, GUI
acceptance, SDK public Rust API compatibility, private-worker protocol
compatibility and each platform's FFI/native behavior remain pending until their
actual results are recorded. Rust source compilation does not prove a stable
cross-version binary ABI. The inherited attribution metadata gate also remains
pending; changing the SDK pin does not resolve that separate provenance check.

Keep source results, executed tests, native acceptance, release packaging and
installed runtime verification as separate evidence. Preserve third-party
notices and the complete previous bundle for any later supported rollback.

## Updating again

1. Fetch upstream and record the immutable candidate SHA. Compare both sides of
   the fork divergence, including behavior already squashed upstream under
   different hashes.
2. Integrate upstream in an isolated compatibility branch and retain necessary
   patches. Review the browser contract, native conflicts and existing-profile
   consent before publishing the immutable source revision.
3. Change the six root declarations together. Run normal `cargo update` for all
   six packages, then `cargo hakari generate` and review the lockfile. Avoid
   unrelated registry upgrades; never substitute a local path pin or alter
   Cargo's Git cache to make a source check pass.
4. Run the dependency contract, formatting and current-head Hakari checks;
   compile the workspace with Rust 1.95.0. Verify GUI test compilation and
   record the exact-head native and platform results before broader acceptance
   claims. Report actual Hakari results rather than assuming an older failure
   still applies.
5. Coordinate later installed-runtime acceptance in the ordinary user context
   using the supported Host lifecycle and source/file/running-Host identities.
   A matching version string or pipe `pong` alone does not identify this SDK
   candidate. Do not introduce an alternate endpoint to bypass that check.

Return to the official Git source once every required patch is present upstream
and the same downstream acceptance gates pass. Preserve the third-party notice
and remove obsolete compatibility comments at that point.
