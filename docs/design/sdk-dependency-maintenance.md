# CUA SDK dependency maintenance

The root workspace owns every direct CUA dependency. Core, Host tests, native
adapters and GUI fixtures inherit those declarations. `Cargo.lock` must resolve
all upstream CUA crates to the same immutable Git source. The policy CI checks
this contract with `scripts/test_sdk_dependency_contract.py`.

## Current source candidate

- Official Driver release: [cua-driver-rs-v0.38.0](https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.38.0),
  immutable commit `trycua/cua@85e2b1318bd77ccf3cfa88ccdf875a4bb7eaed19`.
- Compatibility source: `loonghao/cua@05f43bda479d01aee512a0c092fb064ce1a330bd`,
  integrating that release with the prior DCC source
  `be4bbd3eac57796804f43b291a2012dcd2d96896`. Both contributor histories remain
  ancestors. All six workspace declarations must resolve the same ten SDK
  packages to this one immutable revision.
- The compatibility diff against the official release spans 54 files. It
  retains ancestor scopes, hidden file-input association, navigation readback,
  bounded download metadata and owned Windows browser setup/cleanup contracts.
  Upstream's process-family selection replaces the old name-prefix widening;
  pre-launch PID baseline pruning remains intact.
- Source licensing remains MIT in the official root `LICENSE.md` and Driver
  package metadata. No dependency or runtime is installed by changing a pin.

The merge candidate passed 141 selected mock, launch-selection, synthetic UIA and semantic tests.
Its first complete 43-file review identified three issues, fixed in the pinned
revision: single fully-proven consent selection, rollback on a pre-existing
setup page without claiming tab ownership, and the suggested-filename opt-in
API description. The affected 73 selected tests pass on the consent/rollback fixes. A later
44-file review identified COM cleanup disposal order; the final source keeps
owned-tab proof through rollback and disposes its handle before the outer COM
scope ends. Its directly extracted production helper passes success, error and
unwind ordering cases. The suggested relaxation of foreground visibility was
not adopted: hidden documents cannot prove visible foreground readback.
The subsequent URL comparison finding is fixed with parsed URL equality for
the request/commit comparison and raw fallback on parse failure, while the DOM,
frame, loader and visibility fences stay exact. Its directly extracted pure
regression covers seven canonical and non-equivalent URL cases. Exact final review,
downstream compilation, dependency/Hakari gates and platform CI are separate
candidate requirements; prior revisions' green checks do not qualify this pin.
The prior candidate exposed six SDK core Clippy diagnostics under Rust 1.95.
The current candidate resolves those without lint exemptions; strict SDK core
`--lib` Clippy and 17 targeted regression cases pass. A full SDK workspace
Clippy attempt is still blocked offline by an uncached `webpki-roots v1.0.10`;
the core-only result does not establish workspace-wide strict lint success.
A strict Windows platform attempt also stops in the unchanged cursor-overlay
crate on two inherited lints before reaching the platform crate.

Pending Windows setup now retains a process-wide MTA usage token for agile UIA
identities while keeping thread-bound COM scopes local. Pure tests verify that
identities drop before the token on a different thread and on cleanup-scope
refusal; actual COM and browser behavior remain untested locally. A successful
new-tab invocation is recorded as an attempted action until a unique owned-tab
identity is captured; incomplete ownership reports creation as unproven. The prior
Windows reference drift was corrected using the exact-head CI-generated files;
the related Python formatting failure was fixed only in its one reported file.
The previous SDK revision `4791303` completed a 49-file review with zero
findings; the returned file set matches its complete compatibility diff. The
current revision synchronizes the checked-in installer version with the verified
published 0.38.0 installer artifacts, fixes the compatibility fork's missing
image-doctor ledger source, fences URL fragments during foreground readback,
moves native consent inspection off the async executor, and limits DOM upload
supplementation to explicit chooser elements. The prior SDK head `4fae640` passed first-run and image-doctor CI. The current
`05f43bda` revision refreshes Linux/macOS generated download descriptions from
digest-verified exact-head CI artifacts. Its full 54-file detailed review passed
with zero findings; its fresh CI remains a separate qualification gate.
The trusted-base contributor attribution check still needs a repository-aware
solution for imported upstream source-PR references; it cannot be fixed by
executing a policy change from this pull request's head.

Git-source manifest discovery also requires the SDK to explicitly bind its
workspace root. Without that binding, the offline consumer skipped the SDK
manifest while scanning standalone compatibility fixtures. The same consumer
resolves it with the binding. All direct and generated Hakari SDK references
must agree before compilation qualifies the candidate.

The current SDK source `05f43bda` passed downstream `cargo clippy --workspace
--all-targets --locked --offline -- -D warnings`, formatting, dependency/layout
tests and both Hakari drift checks. The lock audit retains all 614 registry package
identities and checksums; only the ten SDK packages move to Driver 0.38.0 at
one source revision. These local checks do not replace the exact downstream
commit's full review and platform CI, or establish native acceptance.

The separately reported console flash and Max capture refusal remain unproven
runtime issues. See [process roles and console launch limits](windows-console-launch-assessment.md)
and [the Max coordinate audit](max-capture-coordinate-audit.md). No provider,
Host, browser, application window, permission or desktop binding was changed.

## Previous source and validation history

Driver 0.36.0 compatibility source `be4bbd3eac57796804f43b291a2012dcd2d96896`
followed [owned Windows setup cleanup #9](https://github.com/loonghao/cua/pull/9),
[download metadata #8](https://github.com/loonghao/cua/pull/8) and
[compatibility #7](https://github.com/loonghao/cua/pull/7). The following results
refer to those historical candidates, not the 0.38.0 pin.

The follow-up preserves the default SDK download privacy contract. Only a
trusted, already-approved host request can opt into a bounded suggested filename
from the first matching frame/GUID download event. Completion still proves the
stored artifact before returning the name; missing/invalid names remain explicit
and do not rename or change the destination. Public callers cannot forge this
metadata request. DCC uses the existing download authorization boundary to opt in.
The download-metadata parent at `6219e5e4` passed 1,007 SDK-core library tests;
two existing explicit native opt-ins remained ignored. That result and the
earlier 55ab validation below are historical evidence for those revisions.

The Windows cleanup follow-up retains the original native window, baseline
tabs, uniquely created tab and Close control through agile COM references.
Full process birth, native identity and complete readback fence cleanup and
checkbox rollback. Missing or reused identity refuses mutation, and a failed
read cannot establish closure. Uncertain native new-tab dispatch is reported as
`setup_tab_creation_unproven`; DCC preserves that receipt and requests manual
review even when `opened_setup_page` is false.

The final cleanup source passes 29 Windows setup tests and three strict native
enumeration tests with independent exact-source review. Its source PR records
the inherited strict-Clippy debt and validation boundaries; the change adds no
lint exemptions. Cancellation before delivery of the blocking setup handle,
reusable cleanup authorization and native Edge acceptance remain separate gates.

This is the Rust **Driver SDK** under `libs/cua-driver/rust`, including
`cua-driver-sdk` and `cua-driver-contract`. It is a separate component from
`libs/cua` and the independently released `cua-sdk-v0.4.4`.

The component-tagged release has native assets for Windows, Linux and macOS.
The selected release is Driver 0.38.0; source adoption does not run its installers. GitHub's
`prerelease` label keeps the monorepo-wide Latest pointer separate; the Driver
release notes identify plain SemVer Driver releases as its stable channel.
The integration pins source, not the repository-wide Latest release.

## Why the compatibility fork remains

The selected official Driver 0.38.0 does not contain `scope_ancestor_role`,
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
- Responsive existing-profile socket proof. Driver 0.38 shares consented
  sockets: a failed probe releases only its current grant with
  `SharedSocketRelease::Close`; it must not remove another session's hold.
- Exact Windows native consent-window ownership/topmost disambiguation.
- Console-free construction of existing bounded Windows helpers.
- Preserve an intentionally empty host registry. The upstream perception fallback
  must not reintroduce a tool after the Linux pixels-only hook clears it; policy-
  filtered visibility is not evidence that no definitions are registered.

Upstream already contains the old fresh-socket consent retry and stronger
repeated-prompt title/geometry deduplication. Preserve its newer cancellation
settlement and `ConsentNodes` COM cleanup. Additional native-window trees use
that same RAII cleanup. Drop the obsolete netstat subprocess hunk because
upstream uses the native TCP listener API, and do not restore deleted website
documentation paths. The current Driver skill retains the compatibility API
reference. The compatibility branch records the per-patch migration decisions
in `libs/cua-driver/docs/dcc-compatibility-0380.md`.

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
`run_script` is rejected by both unscoped native entry points. The real Linux
pixels-only SDK inventory also rejects `parse_visual_regions`.

Driver 0.38 defaults native `get_window_state` trees to Markdown. DCC explicitly
requests `tree_format: "both"` for semantic snapshots and screenshot-plus-
semantics, preserving read-only status/dialog/menu text alongside actionable
elements and their SDK tokens; exact PID/window/session and bounds remain unchanged. The Windows
overlay adapter supplies `target: None` for its existing point-only movement
request because DCC does not have element bounds to pass upstream.

Native input mappings also follow the 0.38 tool inventory. macOS window cursor
movement uses its typed window target and screenshot coordinates; Linux and
Windows overlay movement converts those coordinates once to native window units
and adds the revalidated window origin. A macOS live/resized observation already
converted to native window units uses that same overlay-only route, avoiding a
second SDK screenshot-scale conversion. The overlay retains window scope.
macOS semantic `set_value` omits the unsupported delivery-mode field, and Linux
explicitly refuses `type_chars` delays that its SDK cannot represent.

DCC preserves `element_index` by retaining the actual SDK token returned in the
same observation, bound to observation/session/PID/window identity. Missing or
ambiguous rows fail closed, and action invalidation discards the mapping. Never
invent a bare row token: two sessions can publish snapshots of the same window.
Move and drag require coordinates rather than unsupported semantic selectors.

On Windows, the CLI dispatches the official SDK's public standard-user shell
launch helper before ordinary startup. This lets an elevated Host launch the
fixture under the SDK's verified standard-user token; it does not remove the
SDK's elevation refusal, token checks or helper timeout. Process tests cover
malformed helper invocations without launching applications, and the native
lifecycle test reads the launched token's effective access-control facts:
Medium-or-lower integrity, an absent or deny-only Administrators SID, and only
standard-user privileges. SAFER tokens inherit `TokenElevation` from their
source; the official SDK explicitly does not use that flag to verify a derived
token. No security settings or SDK token-generation logic are changed.

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

### Candidate evidence (2026-10-18)

With the published `55ab9c62` Git source and no path overrides, Rust 1.95
passed 1,508 downstream library tests (three explicit native opt-ins ignored),
64 CLI/diagnostic/MCP/process tests, the real Linux pixels-only SDK registry
integration test, and strict workspace/all-target Clippy.
The dependency, formatting, layout and generated Hakari gates also passed.

Real Linux SDK acceptance discovered two owned X11 windows, captured the exact
80-by-60 target changing red to blue, and refused a destroyed target with
`missing_window` without writing an image. AT-SPI was unavailable because the
cloud sandbox denies its Unix bus; the observation reports that degradation.
The source follow-up passed 246 browser tests (one existing opt-in ignored)
and the actual source-built Driver CLI direct-MCP protocol schema test.

The `55ab9c62` source follow-up passes all nine real SDK registry/runtime tests,
including a policy-file subprocess regression, and 16 documentation-generator
checks against the source-built CLI. Native CI snapshots supply the generated
Linux/macOS/Windows references. The downstream results and exact-window
acceptance above were rerun against the published pin. The native fixture
returns a degraded root-window node, not proof of AT-SPI child-control access.

The native-action follow-up passed 1,528 library tests (three explicit native
opt-ins ignored), 64 CLI/process tests, the actual empty-registry SDK integration,
strict workspace/all-target Clippy, and dependency/format/layout/Hakari gates.
It adds a contract matrix against the real embedded
SDK inventory, screenshot-scaling and observation-token isolation tests. A real
Linux CLI overlay move placed the agent cursor at the target's window-relative
position while reporting the user pointer unchanged. Native CI must qualify the
final downstream candidate on Windows and both macOS architectures. Local Unix
socket-dependent CLI tests remain blocked by the cloud sandbox's `EPERM`; they
are not counted as passing local tests.

The observation follow-up restores the official `both` projection. Format-sensitive
red/green fixtures cover read-only status, menu and dialog text beside the actual
qualified action token, including refusal to turn text-only row numbers into
tokens. The reviewed local library suite passes 1,537 tests; seven additional pure
Windows privilege-policy cases cover safe and unsafe access facts. A real SDK launch-schema matrix covers selectors, URL/argument forwarding and
session ownership; unsupported false launch defaults are omitted, while explicit
unsupported opt-ins fail before dispatch. A real Linux CLI launch starts the
owned, self-closing X11 fixture and returns its PID with both exact windows;
both unsupported true flags are refused. Windows new-instance remains an
accepted upstream no-op, not an isolation guarantee. The GUI
runner keeps failing on any error but runs all cases with `--no-fail-fast`.
The `dc63d4a` candidate passed all ten CI jobs, including native GUI, release IPC
and immutable artifact verification on Linux, Windows and both macOS architectures.
Its GUI suites passed 17 cases per Linux/macOS target and 52 Windows cases
(including seven pure privilege-fact cases, not 52 distinct native actions).

The review follow-up preserves terminal `InvalidAction` for indexed actions on
pure visual observations, while stale observation IDs and tokenless semantic
snapshots still fail with `StaleObservation`. Three red/green cases verify error
precedence and no dispatch. Schema-only checks continue to inspect the real SDK:
Windows uses its metadata-only inventory without interactive-desktop admission
or cursor startup; Linux/macOS retain the existing runtime to avoid changing
process-global cursor configuration during tests. These follow-up changes require
a fresh exact-head native matrix; the prior candidate's results are not a substitute.

This is not a claim of whole-SDK CI success: the fork's attribution baseline
gate remains tracked separately. The inherited docs container-image gate also
lacks doctor-ledger entries in the fork; image references and baselines are
unchanged. An additional standalone SDK-core strict
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
