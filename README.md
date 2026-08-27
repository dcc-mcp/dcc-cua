<p align="center">
  <img src="assets/brand/dcc-cua-logo.png" alt="CUA logo" width="900">
</p>

<p align="center">
  English | <a href="README.zh-CN.md">简体中文</a>
</p>

# dcc-cua

Cross-platform Computer Use Automation runtime and CLI, backed by the
open-source [CUA SDK](https://github.com/trycua/cua).

The project began inside `dcc-mcp-core`, where it reproduced the Computer Use
workflow used by Codex. CUA SDK's cross-platform support made that boundary
useful beyond one host, so it was extracted and extended as `dcc-cua` for any
agent or application to embed.

`dcc-mcp-core` remains one consumer, not a runtime dependency. The Host protocol
is product-neutral; `application_label` is only a bounded human-readable
safety-banner label. Its public protocol is owned by this repository and
provides:

- exact PID/window/title scope; agent requests cannot widen the target;
- a fresh observation ID is required for every mutation;
- bounded text, key, drag, and coordinate input;
- fail-closed rejection when target application identity is missing or unreadable,
  plus a shared defense-in-depth policy for known terminal, command-interpreter,
  authentication, password, and security application identities;
- explicit stop/resume lifecycle and structured errors;
- a visible, vector-rendered CUA mouse-pointer cursor on Windows, Linux, and
  the packaged macOS Host,
  plus a Host-owned, localized `<agent> is controlling <app>` safety banner
  with a session-specific hue and softly breathing target-window frame on
  Windows. Windows and Linux reuse CUA's native per-session cursor and badge;
  macOS re-enters the same `dcc-cua` executable
  as a private SDK worker so AppKit remains on that worker's main thread.
  Headless macOS sessions still return CUA's structured readiness refusal. All
  supported indicators are click-through. Capture exclusion is backend-specific:
  exact-window capture backends exclude their indicator surfaces, while the
  Windows verified-visible desktop `BitBlt` fallback can include the safety
  banner or frame. Indicators are safety UI, not target content, and must not
  be used as action evidence.
  Windows physical Escape and the cross-platform `interrupt_all` Host request
  advance the same Host-process stop generation for every active connection.
  The Windows Escape boundary uses a non-exclusive low-level hook only while a
  control banner is active, so another application's global hotkey cannot
  silently disable the stop control; the key is consumed instead of also being
  delivered to the controlled application. A blocking message pump refreshes
  the hook every second and fails active banners closed if refresh stops.
  Agent-injected Escape is not an
  operator stop signal: it passes through to the granted target window and
  does not advance the shared stop generation.

Windows mixed-DPI acceptance can be collected without activating or inputting
into the target with `python -B scripts/indicator-acceptance-probe.py
--target-pid <pid> --target-hwnd <hwnd>`. The probe emits JSONL and fails closed
unless the exact 45-DIP frame, 20-band fade, motion policy, monitor migration,
window styles, and target-scoped z-order contract are all observed.

The repository is a Cargo workspace with thirteen product responsibilities plus
the Hakari dependency-unification support crate:

- `dcc-cua-core`: scoped Computer Use domain, safety policy, and
  CUA execution boundary;
- `dcc-cua-e2e`: opt-in controlled GUI tests against the upstream CUA
  Electron fixture and the public Host IPC contract;
- `dcc-cua-browser`: exact-window browser binding, tab snapshots, typed
  browser actions, and bounded file transfer;
- `dcc-cua-client`: reusable Core-side Host IPC client with request
  correlation and binary image attachments;
- `dcc-cua-host`: long-lived versioned IPC and request
  routing;
- `dcc-cua-indicator`: Host-process stop generation plus the Windows control
  banner/frame and physical Escape boundary; cursor/badge rendering stays in
  the CUA SDK, while the packaged macOS Host uses its self-hosted private-worker overlay;
- `dcc-cua-platform-windows`: the single Windows execution adapter for exact
  PID/HWND UI Automation fallback, input injection, foreground fencing, and
  upstream Windows platform integration;
- `dcc-cua-protocol`: shared Host wire limits and per-user local endpoint
  identity used by both the Host and reusable Client;
- `dcc-cua-shm`: cross-platform shared-memory image handoff;
- `dcc-cua-semantic-profiles`: validated application selectors, surfaces,
  targets, route hints, and fallback edges;
- `dcc-cua-profiles`: reusable package-store, inheritance, matching, and
  identity-fenced context services for CLI and embedded consumers;
- `dcc-cua-showcase`: bounded frame-channel recording, OpenH264 encoding,
  segmented MP4 publication, and terminal recording evidence;
- `dcc-cua-cli`: the thin CLI process that composes the workspace crates.

Inside `dcc-cua-core`, source files follow domain responsibility rather
than numeric partitions: `contracts.rs` owns public requests/results and shared
limits, `runtime.rs` owns driver/session orchestration, `window_target.rs` owns
exact native-window identity, `observation.rs` owns observation construction,
and `policy.rs` owns trust-boundary validation, action translation, and SDK
error/result normalization.

Application-specific adapters stay above this workspace. Browser control uses
CUA's typed CDP routes first, then a browser semantic/DOM adapter, exact-window
OS accessibility, and finally explicitly approved exact-window pixels. It does
not enter UIA before proving an available CDP route. Unreal/Fab flows belong in
the Unreal or browser adapter and should combine typed Unreal APIs with scoped
CUA. If the in-editor Fab download route is unavailable, the UE profile points
to the Fab profile's Epic Games Launcher fallback. Fab account and purchase
boundaries remain explicit; security and human-verification clicks additionally
require a trusted-confirmation task grant.

### Independent semantic profiles

The `dcc-cua-semantic-profiles` crate owns the declarative profile model and
built-in profiles. The `dcc-cua-profiles` crate owns the reusable package store,
store-aware inheritance, deterministic matching, and identity-fenced context
selection. It exposes the same typed services used by the CLI, so an embedded
application does not need to spawn a process or parse CLI output. The built-ins
are the official defaults; installed user or studio packages can override them
through an explicitly refreshed `ProfileStore` snapshot.

Profiles are declarative routing and vocabulary contracts. They do not launch
an application, switch control routes, perform a fallback, or prove application
state. Core and Host still own exact-window scope, fresh observations, grants,
actions, and post-action verification.

Portable profile packages use `profile-package.json` schema 2. The manifest
declares typed `artifacts` rather than an untyped file list, requires exactly
one `semantic_profile`, and declares the minimum compatible CLI through
`requires.dcc_cua`. Skills, documentation, fixtures, context documents, and
companion source are optional artifact types. A package is installed only after
every declared artifact, version requirement, identity, path, and size limit is
validated. Install, replace, and dependency-aware uninstall are staged and
rolled back if the resulting store is not valid. Cross-profile fallback and
inheritance edges must resolve entirely within the ready snapshot.

Application startup knowledge uses generic identity-fenced documents, so the
same contract can represent an Excel workbook/template, PowerPoint deck/theme,
DCC plugin ABI, game data snapshot, or policy revision:

```powershell
dcc-cua profile context --id excel `
  --identity document=sha256:... --identity template=quarterly-v3 `
  --selector workflow=financial-review
```

Identity and selector values are exact and case-sensitive. Every matching
document is returned; duplicate document IDs or an index/document fence
disagreement fail closed. The JSON schemas live under `docs/schemas/`.

| Field | Agent meaning |
| --- | --- |
| `profile_version` | SemVer of the Profile data. In a package it must equal `profile-package.json.version`. |
| `application` | Stable host family plus optional exact host-version tokens. `maya-2024` therefore cannot match a Maya 2025 title. |
| `extends` | Optional single parent ID plus SemVer requirement. Runtime resolves and flattens inheritance before the Agent uses the Profile. It never inherits authorization or window scope. |
| `selectors` | Candidate application/window or URL identities. Selector objects are OR alternatives. Inside one object, the application constraint is ANDed with one match from the combined generic/localized title aliases. URL-only selectors never match native windows. |
| `surfaces[]` | A stable task area such as `outliner`, `dialog`, or `launcher_download`. Its `route` selects the owning executor. |
| `targets[]` | Stable intent vocabulary. `names`, BCP-47 keyed `localized_names`, and `automation_ids` narrow live matches; `supported_actions` is an allow-list. Optional `key_bindings` maps a supported semantic action to 1-4 verified keys. |
| `fallback` | A reference to another `profile_id` and `surface_id`. The agent must re-discover, bind, observe, and verify that route; no transition is automatic. |
| `settings` | Profile-wide route preference, optional `default_locale` for untagged aliases, dialog style, and destructive-confirmation policy. Surface routes take precedence for a concrete task. |

Route ownership is explicit:

| Route | Owning path |
| --- | --- |
| `accessibility` | The profile CLI can inspect and act when exactly one fresh live element matches. |
| `unreal_typed_api` | The Unreal adapter/Skill executes the typed operation and returns authoritative state. |
| `browser_dom` | The exact-bound browser adapter executes DOM work. Do not substitute the in-app Browser skill. |
| `os_native_dialog` | The platform-native dialog path binds and controls the exact dialog window. |
| `visual_fallback` | `dcc-cua` uses a fresh exact-window visual observation; desktop scope remains explicit and separate. A declared `key_bindings` action may execute after that same observation fence. |

Multilingual aliases are additive and locale-agnostic at runtime. Every alias is
eligible to match, so an agent does not need to guess the current UI locale
before discovery. Locale tags make coverage inspectable through
`dcc-cua profiles` and maintainable by profile authors. Matching trims and
collapses whitespace and applies Unicode lowercase conversion. Keep profile,
surface, target, role, action, and automation IDs stable; localize only visible
window-title fragments and element names. `default_locale` identifies untagged
aliases when known. Existing profiles that only use `window_title_contains` and
`names` remain valid.

Use this profile-aware agent loop:

1. Run `dcc-cua profiles`, inspect its `supported_locales`, and print the
   candidate with `dcc-cua profile --id ID`. When the application/window is
   already known, use `dcc-cua profile match --app APP --title TITLE`; a unique
   version-specific match wins over the family Profile, while equally specific
   candidates are reported as ambiguous instead of being guessed.
2. Discover the real PID/window with `dcc-cua list --on-screen`, then bind that
   exact identity. Confirm that its application/title or URL matches a selector.
3. Choose the task surface and query the stable target ID. Use localized names
   only to match live UI. Reject actions absent from `supported_actions`.
4. Dispatch through the surface route above. `profile --action` is supported
   only for `accessibility`; other routes are intentionally declarations for
   their owning adapter.
5. If a target has `fallback`, switch to the referenced profile/surface and
   repeat discovery, exact binding, and observation. For example,
   `ue/fab/download` falls back to `fab/launcher_download` when the in-editor
   path is unavailable.
6. Take a fresh observation after every mutation and verify the requested state
   independently. Input delivery alone is not success.

Inspect the extension catalog without starting a DCC process:

```powershell
dcc-cua profiles
dcc-cua profile --id maya
dcc-cua profile --id maya-2024
dcc-cua profile match --app maya.exe --title "Autodesk Maya 2024: scene.ma"
dcc-cua profile --profile-file C:\profiles\maya-studio.json
```

After discovery, inspect or execute one accessibility target using the exact
identity returned by `list`:

```powershell
dcc-cua list --app maya.exe --on-screen
dcc-cua profile --id maya --pid $pid --window-id $hwnd --surface home --query new_scene
dcc-cua profile --id maya --pid $pid --window-id $hwnd --surface home --query new_scene --action click --activate
```

Maya's profile explicitly sets `dialog_style` to `os_native` and routes its
file dialog surface through `os_native_dialog`, so an adapter can request the
operating system's native dialog semantics instead of treating it as an
application-rendered panel.

The repository also ships standalone Agent Skills under `skills/`:

- `cua-cli`: the standalone CLI control loop, verification, safety, and long-task recovery;
- `cua-profile-authoring`: official and user-authored semantic profile guidance.

They use the released `dcc-cua` CLI and are included in release archives, so
another agent can install a single Skill without coupling to the Rust workspace.

Multiple agents may keep independent exact PID/HWND sessions open for different
applications. Semantic, UIA, and browser operations remain parallel; actions
that consume the single OS keyboard/mouse stream use one fair Host-wide FIFO and
release it before post-action capture. Run one Host process per interactive OS
seat so the shared Escape broadcast and raw-input ordering have one owner.
Public `session_id` values are connection-scoped: Host mints a private CUA
runtime identity for every opened window or desktop session and rewrites it at
the IPC boundary. Two agents may therefore both use `session-1` without sharing
cursor, browser refs, recording ownership, capture scope, or cleanup state.

Game automation is supported as an exact-window black-box test surface for
menus, launch flows, HUD interaction, input replay, screenshots, and visual
regression. For authoritative 3D state, performance, physics, or gameplay
assertions, combine CUA input/vision with engine-native telemetry and test APIs
such as Unreal Automation or Gauntlet; protected anti-cheat environments and
exclusive full-screen capture are outside this Host's contract.

This project wraps the CUA SDK for agent-friendly, bounded operations. The
`dcc-cua` CLI owns its Host and update lifecycle and uses the SDK in-process;
users do not install or run a separate `cua-driver` executable or daemon. Use
this CLI/Host for its safety envelope, exact-window capability, fresh
observations, grants, friendly actions, and software-specific adapters.

## Development gates

Every Rust file is limited to 2000 lines. Unit tests live in a sibling
`src/tests.rs` module rather than production files, use `rstest`, and run with
`cargo-nextest`; the same layout gate runs before formatting in CI. A
Hakari-managed `dcc-cua-workspace-hack` unifies dependency features for the
four supported build targets while leaving target and host graphs separate for
the upstream MSVC Spectre dependency. Install both tools once:

```powershell
vx cargo install cargo-nextest cargo-hakari --locked
```

```powershell
vx cargo hakari generate --diff
vx cargo hakari manage-deps --dry-run
pwsh -NoProfile -File scripts/check-rust-layout.ps1
vx cargo fmt --all -- --check
vx cargo nextest run --workspace --all-targets --locked
vx cargo test --workspace --doc --locked
```

Use Cua-Bench only as an external development harness for deterministic tasks
with reset and outcome evaluators. Drive those tasks through the same
`host-jsonl` contract used in production, and report success, action count,
visual/semantic/post-action observation count, image pixels/encoded bytes,
errors, elapsed time, and wire bytes. Keep transport
microbenchmarks in Rust and do not add Cua-Bench, its Python environment, or VM
providers to the shipped runtime. A live game session without a reset/oracle is
showcase evidence, not a repeatable benchmark.

Pass `--metrics-output artifacts/cua-bench/dcc-cua-metrics.json` to the
development `host-jsonl` bridge. The bridge atomically refreshes the report
after every response, so a long-running task can be inspected before EOF.
`run_status` is `running`, `succeeded`, or `failed`; `transport_success` is null
until the bridge finishes. The v3 report separates attempted/succeeded/rejected
actions, visual/semantic/post-action observations, image pixels/encoded bytes,
live-observation start/clean-stop lifecycle, errors, elapsed time, and JSON payload bytes.
`action_kinds` and `error_codes` make repeated pointer motion and platform
refusals visible without reparsing the full JSONL trajectory. Cua-Bench remains
responsible for reset and outcome evaluation.

On Windows, `vx.toml` selects MSVC 14.44 with Spectre-mitigated libraries and
injects its Windows SDK environment into Cargo. Release archives contain one `dcc-cua`
executable plus assets, Skills, and both project and upstream license notices.
Each target also publishes a stable
`dcc-cua-install-manifest-<target>.json` containing the exact HTTPS archive URL
and SHA-256 digest for package managers such as `dcc-mcp-cli`. The built-in
updater requires the exact `<archive>.sha256` sidecar and verifies it before
extracting or replacing the executable.

## CLI

```powershell
cargo run -p dcc-cua-cli -- list --app chrome.exe --title "New Tab" --on-screen
cargo run -p dcc-cua-cli -- wait-window --app UE5Editor.exe --title "PCG Fab" --on-screen
cargo run -p dcc-cua-cli -- apps
cargo run -p dcc-cua-cli -- tools
cargo run -p dcc-cua-cli -- call --tool check_permissions --json '{}'
cargo run -p dcc-cua-cli -- call --tool set_config --json-file payload.json
cargo run -p dcc-cua-cli -- manifest
cargo run -p dcc-cua-cli -- desktop-snapshot --output desktop.png
cargo run -p dcc-cua-cli -- screen-size
cargo run -p dcc-cua-cli -- cursor-position
cargo run -p dcc-cua-cli -- desktop-act --action-json '{"action":"click","x":100,"y":100}'
cargo run -p dcc-cua-cli -- act --pid 4242 --window-id 123456 --action-json '{"action":"click","x":100,"y":100}' --output after.png
cargo run -p dcc-cua-cli -- launch --name Calculator
cargo run -p dcc-cua-cli -- doctor
cargo run -p dcc-cua-cli -- doctor --spawn target/debug/dcc-cua
cargo run -p dcc-cua-cli -- snapshot --app chrome.exe --output screenshot.png
cargo run -p dcc-cua-cli -- accessibility --app chrome.exe
cargo run -p dcc-cua-cli -- window-state --app chrome.exe
cargo run -p dcc-cua-cli -- activate --app chrome.exe
cargo run -p dcc-cua-cli -- set-window-frame --pid 4242 --window-id 123456 --x 80 --y 80 --width 1280 --height 720
cargo run -p dcc-cua-cli -- invoke-menu --app maya.exe --menu File --menu "New Scene"
cargo run -p dcc-cua-cli -- click --app chrome.exe --x 100 --y 100
cargo run -p dcc-cua-cli -- click --app chrome.exe --element-index 12
cargo run -p dcc-cua-cli -- toggle --app chrome.exe --element-index 14
cargo run -p dcc-cua-cli -- set-value --app chrome.exe --element-index 15 --value Published
cargo run -p dcc-cua-cli -- drag --app chrome.exe --from-x 100 --from-y 100 --to-x 300 --to-y 200 --duration-ms 750 --steps 32
cargo run -p dcc-cua-cli -- type --app chrome.exe --text "hello" --focused
cargo run -p dcc-cua-cli -- hotkey --app chrome.exe --key CTRL --key L
cargo run -p dcc-cua-cli -- scroll --app UE5Editor.exe --scroll-x 4 --by page --x 600 --y 900
cargo run -p dcc-cua-cli -- act --pid 4242 --window-id 123456 --action-json '{"action":"click","x":100,"y":100}'
cargo run -p dcc-cua-cli -- snapshot --pid 4242 --window-id 123456 --output after.png
cargo run -p dcc-cua-cli -- verify --pid 4242 --window-id 123456 --expect-json '[{"window":{"exists":true}}]'
cargo run -p dcc-cua-cli -- clipboard-write --pid 4242 --window-id 123456 --text "bounded text"
cargo run -p dcc-cua-cli -- clipboard-read --pid 4242 --window-id 123456 --include-text
cargo run -p dcc-cua-cli -- --version
cargo run -p dcc-cua-cli -- update --check
```

`update` downloads one exact platform archive, validates the replacement
executable, and replaces the CLI. The SDK runtime is statically linked into the
same versioned executable. Release discovery uses the GitHub REST API first and
falls back to the public `releases/latest` redirect when the API is rate limited
(HTTP 403); set `GITHUB_TOKEN` or `GH_TOKEN` to raise the unauthenticated API
quota of 60 requests per hour per IP address.

In an interactive terminal, successful commands also perform a best-effort
update check in parallel. Only releases with both the exact platform archive
and its SHA-256 sidecar qualify for a reminder. The result is cached for 24
hours under `~/.dcc-cua/cache`, failures are silent and retried after one hour,
and protocol/CI commands are never interrupted. Set
`DCC_CUA_NO_UPDATE_CHECK=1` to disable these reminders.

`manifest` is the stable machine-readable discovery entry for Core and other
independent callers. It reports the current platform, Host protocol, frame
limits, snapshot transports, capabilities, endpoint, recommended stdio/JSONL
launch arguments, and that no separate driver executable is required.

One-shot CLI commands write their normal structured success or error envelope
to stdout. A command error preserves a non-zero exit code and emits exactly one
JSON envelope, so stdout pipelines, redirects, and command substitution can
parse failures without merging stderr. Stderr is reserved for fixed, safe
process diagnostics such as an internal panic or an unavailable stdout pipe;
it never duplicates the command envelope. Long-lived `host-jsonl`, native
messaging, and MCP commands keep their protocol-specific stdout framing.

`list`, `apps`, `tools`, `desktop-snapshot`, `screen-size`, `cursor-position`, and
`doctor` are read-only. `snapshot` and `act` require one exact
target; if an application has multiple windows, pass `--pid` and
`--window-id` instead of relying on an app name.

`doctor` runs CUA's `check_permissions` and `health_report` concurrently with
driver/window discovery. It reports the upstream structured checks for native
accessibility, screen capture, platform support, and input readiness, and exits
non-zero when the authoritative health status is not `ok`. Its `ready` field
also becomes false when the current Windows session is disconnected, its input
desktop is not the interactive `Default` desktop, or the process cannot read
the current input surface, even if UIA and screen-capture permissions remain
available. A temporarily absent foreground window is reported as diagnostic
state but does not by itself imply that the desktop is locked. Exact-window
capture and showcase recording remain independently observable, while raw
input first requires a successful read-only cursor probe and then activates and
revalidates the granted PID/HWND before injection. The report includes the
native WTS, observation, and input-surface state; raw mouse/keyboard routes fail
before input with `interactive_desktop_unavailable`, while
exact-element Windows UIA operations can continue through their background
semantic path.

Use `doctor --route visual` when the intended contract is exact-window capture
plus bounded coordinate input on a custom-rendered DCC or game surface. The
report keeps the strict aggregate `ready` field and adds independent `routes`
for `full`, `visual`, and `semantic`; a degraded UIA provider therefore remains
visible without incorrectly rejecting a healthy WGC/Win32 visual route. On
Windows, a timed-out global UIA desktop enumeration is reported as a degraded
`exact_window_uia_fallback` semantic route when UIA permission remains present;
the fallback still requires an exact PID/HWND and a fresh fenced observation.
The default `doctor` behavior remains strict and backward compatible.

`list` accepts optional `--app`, `--pid`, `--window-id`, `--title`, and
`--on-screen` filters. `--app` is case-insensitive; `--title` is exact and
bounded, so Core can select a UE/Fab subwindow without receiving the entire
inventory over Host IPC. `wait-window` polls CUA's native window inventory for an app,
process ID, window handle, or exact title. It is capped at 30 seconds and is
intended for launch/switch orchestration before a target-bound action.

`snapshot` returns the screenshot plus a bounded semantic tree (default 512
elements and 16 levels). `accessibility` reads a larger semantic tree without
transferring screenshot pixels when the agent needs to locate a deeply nested
control. Prefer the returned `element_token`/`element_index` over guessed
pixels, then verify the post-action state. UIA element `bounds` are explicitly
tagged as `virtual_desktop`, while window pixel actions use non-negative
coordinates from the latest exact-window screenshot. Desktop actions use signed
virtual-desktop coordinates, including negative X/Y on Windows monitors left of
or above the primary display. `window-state`, `activate`,
`set-window-frame`, and `invoke-menu` operate on the same exact target scope.
Long-lived recording remains on the persistent Host session so its start/stop
lifecycle is not lost when a one-shot CLI process exits.

The friendly selector forms also include toggle, drag, set-text, and set-value.
Click, toggle, scroll, press, hotkey, type, and value actions can target the
latest semantic tree with --element-index or --element-token; coordinate
actions remain available for custom-rendered surfaces. Semantic `type`
uses CUA's canonical `type_text` element path; `type --x X --y Y`
uses CUA's pixel-focus-then-type path for Chromium/Electron and other
custom-rendered inputs; `press` and `hotkey` accept the same focus coordinates.
`press --modifier M` and drag's `--modifier M` map to CUA's native modifier
fields, while click and drag accept `--button left|middle|right`.
Scroll accepts one signed axis per action (`--scroll-x` or `--scroll-y`), an
optional `--by line|page`, and either a semantic element or exact window-local
coordinates. This preserves horizontal timeline/canvas scrolling instead of
silently converting it to a vertical wheel action.

One-shot window and desktop actions always attempt a fresh post-action
snapshot. `--output FILE` writes that post-action image, and the JSON result
includes `post_snapshot`. If capture fails after input was already delivered,
the command reports `action_was_executed: true` instead of returning a generic
mutation failure that could cause an unsafe retry.
Their banner and frame exist only for that command's session lifetime. Core and
other multi-step agents should keep one Host IPC session open so the same exact
PID/HWND retains its visible banner, breathing frame, cursor, and observation
fence across actions. The Windows UIA worker starts through a separate readiness
handshake. A due upstream-session refresh never crosses an action request: Core
refreshes before the next observation and requires a new observation fence
before input. A refresh or activation timeout leaves the local long-running
session alive and returns a typed no-blind-retry result. Only a timeout after an
action was actually dispatched is completion-unknown and invalidates that exact
local window session.

The shared Core window session owns this ControlBanner, so direct `dcc-cua`
window actions and Host IPC sessions use the same visible banner and target
frame. The Host's Hello `client_name` is shown as the agent name; the text
follows the operating-system language and the color is stable for that agent's
session while avoiding the default banner hue. Desktop-scope sessions have no
single PID/HWND to frame and therefore retain the localized CUA desktop marker
instead.

For common controls, `click`, `double-click`, `right-click`, `move`, `scroll`,
`press`, `hotkey`, and `type` build the same fenced CUA actions without manual
JSON. `type` requires `--focused` or an explicit `--element-index`/
`--element-token`; `hotkey` accepts repeated `--key` values.

`apps` uses CUA's cross-platform application inventory. `launch` accepts one
explicit selector (`--name`, `--bundle-id`, `--aumid`, `--path`, or
`--launch-path`) or at least one bounded `http`, `https`, or Epic Launcher URL,
supports repeated `--url`/`--arg` values, and applies the same
sensitive-application deny policy as the host.

`snapshot`, `act`, `verify`, `clipboard-read`, and `clipboard-write` accept the
same `--app`, `--pid`, `--window-id`, and `--title` window selectors. Multiple
selectors are conjunctive. Conflicting duplicate selectors, missing or zero
native identities, no match, multiple matches, and PID/HWND/title drift fail
closed before a mutation or clipboard readback. Prefer the PID/HWND pair
returned by `list` and take a fresh snapshot before an action.

An `act` success receipt proves bounded delivery, not application effect. Require
its successful `post_snapshot` and verify a changed tree/value, changed pixels,
or an application-specific state before accepting the mutation. A
`window.exists=true` predicate proves liveness only. Likewise,
`clipboard-write` is not its own postcondition: for a non-sensitive bounded
test value, use an exact-bound `clipboard-read --include-text` comparison, or
verify the application-specific pasted/changed value without exposing private
clipboard content.

Clipboard access is session-scoped and grant-gated: `clipboard_read` does not
return text unless the caller asks for it, and `clipboard_write` accepts exactly
one bounded text, image path, or regular-file path. Recording is also
grant-gated through `recording_start`, `recording_stop`, and `recording_state`.
`live_observation_start` prefetches the exact window's latest frame with a
requested ceiling of 1..30 FPS. Windows owns one persistent D3D11
device, capture session, frame pool, and reusable staging texture for the feed;
if that backend cannot initialize, the state identifies the one-shot WGC
fallback instead of silently claiming the fast path. macOS and Linux retain the
same latest-frame contract through bounded driver PNG capture/decode. Decision snapshots preserve
the full-window coordinate mapping while bounding their longest edge with
`max_dimension` (default 1568, accepted range 256..4096); showcase recording
keeps its independent video sizing. State reports the lifetime `effective_fps`,
EWMA `recent_effective_fps`, `last_capture_duration_ms`,
`max_capture_duration_ms`, and `capture_mode`. While active, `snapshot` consumes
only a newer frame, encodes that selected frame once, and skips UIA; use
`accessibility_snapshot` separately when semantic state is needed. Stop it with
`live_observation_stop`.

Showcase video rolls into independently decodable five-minute MP4 segments so
an interrupted long run cannot invalidate footage that was already finalized.
The first segment retains the compatible `showcase.mp4` path; later segments
use ordered names such as `showcase-0002.mp4`. Only the active tail is written
as `*.partial.mp4` and exposed as `current_partial`. Each completed segment
starts with an IDR access unit and carries its own SPS/PPS track configuration.
`recording_stop` finalizes the tail, clears `current_partial`, and atomically
publishes `showcase.manifest.json` with the ordered, non-overlapping segment
timeline. If the interactive desktop is temporarily unavailable, the live
producer and recording owner remain active but report `paused` and
`video_paused`; cached frames are not encoded again. Capture resumes in the same
stream only after a strictly newer frame, while a missing window or changed
PID/HWND owner remains terminal. A crash can therefore strand only the
explicitly partial tail, not the earlier readable segments.

## Host IPC

Start one persistent host process instead of spawning a process per action:

```powershell
cargo run -p dcc-cua-cli -- host --stdio
cargo run -p dcc-cua-cli -- host
cargo run -p dcc-cua-cli -- host-ensure
cargo run -p dcc-cua-cli -- ping --spawn target/debug/dcc-cua
cargo run -p dcc-cua-cli -- interrupt-all
cargo run -p dcc-cua-cli -- host-call --method list_apps --json '{}'
cargo run -p dcc-cua-cli -- host-batch --json '[{"method":"list_apps","params":{}},{"method":"screen_size","params":{}}]'
cargo run -p dcc-cua-cli -- host-call --spawn target/debug/dcc-cua --method list_apps --json '{}'

# Keep one Host connection open and process one JSON request per input line.
cargo run -p dcc-cua-cli -- host-jsonl --spawn target/debug/dcc-cua --output-dir artifacts
# Batch only stateless discovery lines within a short window.
cargo run -p dcc-cua-cli -- host-jsonl --parallel-discovery --spawn target/debug/dcc-cua
```

`dcc-cua-client` is the direct embedding path for dcc-mcp-core. It opens
the per-session endpoint, performs `hello`, sends JSON requests, and returns
the following binary image frame without base64 decoding in the control path:

```rust,no_run
let mut host = dcc_cua_client::HostClient::connect_default("dcc-mcp-core").await?;
let response = host.request("list_windows", serde_json::json!({})).await?;
let stopped = host.interrupt_all().await?;
```

For one logical task, consume the negotiated client into
`LogicalTaskSession`. It opens exactly one window session, injects its private
grant/capability into every scoped request, and returns the still-negotiated
connection on `close`:

```rust,no_run
let host = dcc_cua_client::HostClient::connect_default("dcc-mcp-core").await?;
let mut task = host.open_logical_task_session(
    "task-42",
    serde_json::json!({
        "task_grant_id": "grant-42",
        "application_label": "Chrome",
        "process_id": 1200,
        "window_handle": 2400,
        "allow_browser_input": true
    }),
    15 * 60 * 1000,
).await?;
let snapshot = task.request("snapshot", serde_json::json!({})).await?;
let host = task.close().await?;
```

`HostClient::interrupt_all` and `dcc-cua interrupt-all [--endpoint PATH]`
broadcast a cooperative safety stop to every connection in the selected Host
process. The calling connection is cleaned up before acknowledgement. Other
connections poll the shared stop generation while otherwise idle, then
proactively stop live observation, finalize recording, invalidate frames, and
retain interrupted window/desktop tombstones; those sessions return
`user_interrupted` on subsequent use. An already-running native SDK call remains
bounded by the Host action timeout because CUA exposes no portable preemption
primitive.

`dcc-cua host-ensure [--endpoint PATH]` is the idempotent supervisor entry
for Core and adapters. It first probes the local endpoint, starts this same
version of `dcc-cua host` only when absent, and waits for a negotiated ping.
An endpoint served by a different CLI version fails closed with restart
guidance instead of being reported as ready.
Windows also holds one named singleton per endpoint because named pipes alone
permit multiple server instances; Unix keeps the existing socket bind as its
singleton boundary. Independent Core bridges can therefore share one Host per
interactive OS session while retaining connection-scoped agent sessions.

When Core owns the Host lifecycle, use `HostProcess::spawn` to start the CLI
with `host --stdio`, reuse the same negotiated `HostClient`, and call
`shutdown` when the task ends. This keeps process supervision out of Core's
request code while preserving the same protocol as endpoint connections.

```rust,no_run
let mut host = dcc_cua_client::HostProcess::spawn(
    "dcc-cua",
    "dcc-mcp-core",
    dcc_cua_client::SnapshotTransport::SharedMemory,
).await?;
let response = host.client_mut().request("list_apps", serde_json::json!({})).await?;
let _status = host.shutdown().await?;
```

Supervisors can poll `host.is_running()` and call `host.restart(...)` after a
process exit. Restart is explicit and never replays requests; Core must reopen
sessions and obtain a fresh observation before sending another action.
`HostClient::ping` and `dcc-cua ping` provide a small protocol-level
liveness check without querying the native CUA backend or transferring its
tool inventory.
`HostClient::doctor` and `dcc-cua doctor --endpoint/--spawn` probe the
selected CUA runtime owned by that Host process. The structured report keeps
transport liveness separate from driver, window inventory, permission, and
native health readiness.

Long waits can use `HostClient::request_with_cancel`; it sends `cancel` on the
same connection and consumes both the cancellation acknowledgement and the
wait terminal response.

Read-only discovery and observation calls can use `HostClient::request_batch`
to write several requests before one flush; responses are returned in request
order. Core callers that need task/turn tracing can use
`HostClient::request_batch_with_ids`, which preserves caller-owned IDs through
the same pipelined write. The Host dispatches stateless discovery calls
(`ping`, `list_apps`, `list_tools`, `list_windows`, `screen_size`, and
`cursor_position`) concurrently; exact-window and desktop session state stays
serialized. The client matches responses by `request_id`, so completion order
does not change the caller's result order. Mutating methods stay on `request`
so side effects remain explicit. The handshake advertises this as
`pipelined_read_requests` and `parallel_discovery_requests`.

The `host-batch` CLI accepts `{request_id?, method, params}` objects and uses
one persistent Host connection; supplied IDs are echoed, while omitted IDs
receive a deterministic `host-batch-N` ID. If a response contains image
pixels, pass `--output-dir DIR`; metadata-only batches can omit it. `host-call` and
`host-batch` accept `--spawn BINARY` for a one-shot stdio-managed Host when a
supervisor does not already own an endpoint.

`host-jsonl` is the streaming CLI bridge for dcc-mcp-core and scripts. It keeps
one negotiated Host session open, reads one `{request_id?, method, params}`
object per line from stdin, and writes one response object per line to stdout.
When present, `request_id` is preserved end to end for Core task/turn tracing.
Binary image attachments are written to `--output-dir`; `shared_memory` keeps
image pixels out of the Host control pipe. The machine manifest publishes the
transport-neutral `host-jsonl` entry point so Core can prefer shared memory and
fall back to binary attachments when its native shared-memory reader is absent.

Consumers that need a standard MCP tool-result envelope can opt in with
`--response-format mcp`. Each JSONL response then contains `content`,
`structuredContent`, and `isError`; window, desktop, post-action, browser, and
native-tool image attachments are promoted to native MCP `image` content. The
default `host` response format remains unchanged. This flag projects Host
responses only—it does not turn the JSONL transport into an MCP JSON-RPC
server.

```bash
dcc-cua host-jsonl --response-format mcp --snapshot-transport shared_memory
```

`--parallel-discovery` batches contiguous `ping`, `list_apps`, `list_tools`,
`list_windows`, `screen_size`, and `cursor_position` requests for up to 5 ms or
32 lines, preserves input order and `request_id`, and leaves stateful,
visual, browser, and mutating requests serialized.
The Host enforces the same per-connection ceiling with backpressure and reaps
completed discovery tasks while the connection stays open, so a long-running
Core bridge cannot retain one task allocation per request.
The Rust Client rejects larger single batches before writing to the transport;
`host-jsonl --parallel-discovery` automatically splits a longer stream into
bounded batches on the same connection.
The endpoint admits at most 32 simultaneous client connections and applies
transport backpressure before creating another connection task. The manifest
publishes both connection and per-connection discovery limits for supervisors.
Each logical agent task should own one persistent Host connection and one
TaskGrant-bound window session on that connection. `open_session` defaults to a
15-minute idle timeout, accepts a bounded `idle_timeout_ms` from 1 second to 24
hours, and renews the lease after every authorized session request. Expiry
stops the session, invalidates its observations, and requires a fresh session;
it does not silently replay the previous request. A connection may own up to
16 window, desktop, and launch sessions in total. Different connections may use
the same public `session_id`; their random window capabilities and observation
state remain private to the owning connection. Background-safe actions can make
progress independently, while physical desktop input is arbitrated by one
host-global FIFO so two agents cannot interleave keys or pointer mutations.
Disconnecting one agent stops only the sessions owned by that connection.
Supervisors can discover this contract through
`host.session_concurrency` and the `multi_agent_sessions` capability in
`dcc-cua manifest`.
Clients have 10 seconds from connection acceptance to complete `hello`.
Negotiated Host connections remain long-lived, while their logical-task window
sessions use the bounded idle lease above.
EOF, transport failures, and malformed frames all abort outstanding discovery
work and stop every private window, desktop, and launch session on that connection.
`--metrics-output FILE` atomically checkpoints development metrics after every
response and finalizes them at EOF or failure; it does not alter response JSONL
or the production Host protocol.

## Browser provider routing

CDP remains the default provider for browser sessions that dcc-cua can bind and
control directly. Use the optional extension only when CDP is unavailable or a
user-owned signed-in tab must remain under the browser's extension permission
boundary. The machine-readable planner makes that decision explicit:

```powershell
dcc-cua browser-extension plan --browser chrome --extension-id PUBLISHED_ID --cdp-state available
dcc-cua browser-extension plan --browser chrome --extension-id PUBLISHED_ID --cdp-state unavailable
dcc-cua browser-extension install-native-host --browser chrome --extension-id PUBLISHED_ID
```

`install-native-host` registers the current checksum-verified `dcc-cua` binary as
`com.dcc_mcp.dcc_cua` for the exact published extension identity. It does not
silently sideload an extension. Ordinary users install the signed store package
with browser/user authorization, then click the extension action in the exact
tab once. That click starts the Native Messaging bridge, which registers the
paired provider with the persistent Host.

On the existing logical-task session, call `browser_extension_status` with its
exact session credentials. If a provider is ready, call
`browser_extension_call` with the selected `provider_id`, exact expected
origin, method (`snapshot`, `click`, `type`, or `unpair`), and method-specific
params. Snapshot refs remain extension-owned and actions are fenced by the
latest extension snapshot. A permission, origin, identity, pairing, or protocol
failure is terminal for that route; do not silently fall back to CDP after such
a failure.

```text
{"request_id":"core-task-42","method":"list_apps","params":{}}
{"request_id":"core-task-43","method":"list_windows","params":{"on_screen_only":true}}
```

For large or frequent images, use `connect_default_with_transport(...,
SnapshotTransport::SharedMemory)` and open the returned descriptor with
`dcc_cua_shm::SharedImageReader`; window/desktop snapshots, verification
screenshots, and browser responses containing one image keep pixels out of the
control pipe. Published regions remain owned independently of observation-cache
invalidation until their response handoff is replaced or the session ends.
Shared-memory client batches reject more than one image publisher for the same
window or desktop session, preventing an unread descriptor from being replaced.
Native extension results and browser responses containing multiple images use one
bounded binary attachment frame with offset descriptors.

On Windows, the default endpoint is the per-session named pipe
`\\.\pipe\dcc-cua-v1-session-<WindowsSessionId>` with a protected DACL
that grants access only to LocalSystem and the current Windows logon SID. The
Host reserves the pipe name with a first-instance-only create and fails closed
if it was pre-created; the Rust client also verifies the connected server
process belongs to the current Windows user before sending `hello`. On Unix, the default
endpoint is `$XDG_RUNTIME_DIR/dcc-cua-v1.sock` when that directory is
owned by the current user with mode `0700`; otherwise it falls back to
`$TMPDIR/dcc-cua-<uid>/dcc-cua-v1.sock`. The Host creates a missing
endpoint parent with mode `0700` and refuses relative paths or parents
that are not current-user `0700` directories. The protocol uses an unsigned
big-endian `u32` length followed by one UTF-8 JSON request or response. A
client selects `snapshot_transport` as `binary_frame` (the default) or
`shared_memory` in `hello`; binary snapshots are followed by one additional
length-prefixed PNG frame, avoiding base64 pixel transfer and keeping the JSON
frame under 4 MiB. Requests may include a top-level `request_id` (1–128
characters); the host echoes it on the JSON response, including errors. The
handshake advertises the exact capabilities of this build.
`dcc_cua_client::HostClient::capabilities()` and
`supports_capability(...)` expose that negotiated list without requiring Core
to parse Host handshake JSON; callers can choose shared memory, batching, or
optional routes from the actual Host instead of assuming them.

Native tool results expose every returned image in `attachments`. The single
binary frame following the JSON response is the concatenation of those images;
each descriptor gives its `offset`, `length`, and `mime_type`.

The supported request surface is `hello`, `ping`, `list_apps`, `list_tools`, `list_windows`, `wait_for_window`, `launch_app`, `open_session`,
`get_window_state`, `change_window_state` (`activate`, `restore_activate`, or `close`), `set_window_frame`, `invoke_menu`, `snapshot`,
`accessibility_snapshot`, `verify_state`, `call_tool`, `call_global_tool`, `get_session_state`, `cursor_tool`, `escalate_session`, `find`, `wait_for`, `browser_snapshot`,
`browser_prepare`, `browser_navigate`, `browser_click`, `browser_type`, `browser_pointer`,
`browser_set_input_files`, `browser_download`, `browser_dialog`,
`clipboard_read`, `clipboard_write`, `recording_start`, `recording_stop`,
`recording_state`, `live_observation_start`, `live_observation_state`,
`live_observation_stop`,
`desktop_snapshot`, `screen_size`, `cursor_position`, `open_desktop_session`,
`desktop_session_snapshot`, `execute_desktop_action`, `stop_desktop_session`,
`zoom`,
`get_input_state`, `poll_session_events`, `execute_action`, `resume_session`, `terminate_app`, and
`stop_session`; `cancel` is available while `wait_for` is active and
`cancel_window_wait` while `wait_for_window` is active on the same connection.

`get_input_state` and `poll_session_events` expose one bounded, per-session
event stream with orthogonal `current_state` (interactive input) and
`current_target_state` (exact-window availability). Always continue polling
from the response-level `latest_sequence`; component `sequence` fields identify
only that component's last transition. The stream deduplicates unchanged
statuses without advancing the cursor while refreshing their sampled metadata,
reports overflow with `resync_required`, and emits
`target_minimized`/`target_restored` as well as input suspend/resume events.
`foreground` is sampled target metadata, not a separate transition event.
A minimized target pauses every action with `automatic_input=false`. For a
grant originally bound to an exact PID/HWND, an agent may explicitly request
`change_window_state` with `operation: "restore_activate"`; this restores only
that HWND, validates ownership and final foreground state, and still requires a
fresh observation. It never runs automatically or blind-retries. Interactive
desktop readiness is checked again immediately before each Windows mutation,
and all action/UIA/browser/shared observation caches are invalidated even when
the restore attempt only partially succeeds. Live observation, showcase, and
recording ownership remain intact.
`execute_action` accepts `capture_after: true` plus optional
`post_snapshot_delay_ms` (0..5000), `post_snapshot_max_nodes`, and
`post_snapshot_max_depth`. The Host then performs
the mutation and captures the next exact-window observation in one serialized
request, returns its screenshot and semantic tree as `post_snapshot`, and keeps
that observation current for the next action. The handshake advertises this as
`action_post_snapshot` and `action_post_snapshot_delay`. Use the delay for
custom-rendered applications that need a bounded settle period instead of
issuing a second snapshot. If only the post-action capture fails, the response
still reports the completed mutation and sets `observation_required: true`.
An action without `capture_after` also sets `observation_required: true`.
`execute_desktop_action` accepts the same `capture_after: true` flag and returns
the next full-display image and desktop state in `post_snapshot`; it has no
window accessibility-tree bounds.
Semantic actions use CUA `element_index` values from the latest
accessibility snapshot, and `set_text`/`set_value`/`set_checked` use CUA's
native semantic value path. Coordinate actions remain available for
custom-drawn surfaces. For applications that miss fast keystrokes, use
`action: "type_chars"` with `delay_ms` (0..200); it requires an
`element_index`/`element_token`, or the explicit `type_chars_only: true` when
the target field is already focused. This maps to CUA's cross-platform
`type_text` input path with character pacing and does not accept screen
coordinates.
`list_windows` supports optional `app`, `pid`, `window_id`, `window_title`, and
`on_screen_only` filters;
these are applied by the native backend before the response crosses IPC.
`open_session` grants may bind an exact `window_title` when Core does not yet
have the PID/HWND; the Host still requires the title to resolve to exactly one
native window before starting the CUA session.
`open_session.params.activate_before` performs only exact PID/HWND activation;
it does not restore a minimized target. The Host revalidates the same identity
at the Windows mutation boundary and validates the final foreground state. The
option defaults to `false`; title-only and process-only bootstrap activation
fail closed, and no general native-tool permission is granted. To recover a
minimized exact target, open without bootstrap activation and issue the explicit
grant-scoped `restore_activate` operation before taking a fresh observation.
If it becomes minimized after the session opens, use the typed target event
contract above. `activate` retains its existing semantics; only the explicit
`restore_activate` operation performs an exact restore before activation.
`close` posts a polite `WM_CLOSE` only to an exact Windows PID/HWND target and
requires `allow_trusted_confirmation: true`. It never terminates the process
and reports failure unless the HWND actually disappears.
`wait_for_window` accepts `query` with `app`, `process_id`, `window_handle`,
`window_title`, and `on_screen_only`; its `timeout_ms` is capped at 30 seconds.
When a request ID is supplied, cancel it on the same connection with
`cancel_window_wait` and `{"wait_id":"<request_id>"}`. The Rust Client's
`wait_for_window_with_cancel` helper sends that route automatically.
`find` filters the current accessibility tree by text, role, or element index
and returns a fresh `accessibility_state_id`. `wait_for` is bounded to 30 seconds and supports `element_present`,
`text_contains`, `text_equals`, and `value_equals`. `launch_app` requires a non-empty `session_id`, `task_grant_id`, `application_label`, and
`allow_app_launch: true`; `terminate_app` requires the separate
`allow_app_terminate: true` grant and force-closes only the exact session target;
neither permission inherits from an open DCC window
session. Clipboard operations require `allow_clipboard_read` or
`allow_clipboard_write`; recording operations require `allow_recording: true`;
live observation requires `allow_live_observation: true`.
Grant IDs are capped at 128 characters and application labels at 80; both reject
control characters and surrounding whitespace. The manifest exposes these
limits under `host.grant_limits` for non-DCC callers.
When CUA returns a newly launched PID, the Host keeps its private runtime
session alive and `open_session` with the same public `session_id`, task grant,
and application label promotes that ownership into the exact window session. This lets
CUA standard mode prove that `terminate_app` targets a process created by that
runtime; a different PID or grant is rejected. Keep these requests on one
persistent Host connection because launch ownership is connection-scoped.
Browser mutations additionally require `allow_browser_input: true`.
`browser_prepare` is destructive and separately requires
`allow_browser_prepare: true`; it never changes a personal browser profile
implicitly and forwards CUA's explicit setup refusal/approval contract.
For an isolated browser, mint the one-use approval through an operator-owned
upstream CUA installation, then pass its token to `browser_prepare`; DCC CUA
never mints a browser approval on behalf of an agent. Existing-profile attachment requires
a trusted Core authorization host created through
`ComputerUseDriver::create_with_authorization_host`; the default runtime and
Host process keep refusing it.
`browser_snapshot` first binds the exact native window, then snapshots a
specific CUA tab; `browser_click`, `browser_type`, and `browser_pointer` require
the latest browser `snapshot_id`, exact binding, and an explicit input route.
Hosts advertising `nearest_ancestor_role_v1` accept `scope_ancestor_role` only
for a `semantic_v2` tab snapshot with an exact `target_id`, `tab_id`,
`scope_ref`, and `query`. CUA resolves the nearest same-frame ancestor with the
requested accessibility role and refuses missing, ambiguous, stale, or
unproven ancestry. A successful first page reports
`snapshot.scope: "ancestor_subtree"` plus `scope_anchor` evidence containing
the requested ref, normalized role, accessible name, frame kind, and positive
ancestor distance. Scoped continuation tokens are connection-scoped and
single-use; each continued page reports `snapshot.scope: "continuation"` and
must preserve that exact anchor. A new observation invalidates prior refs and
continuations.
`browser_navigate` defaults to `delivery_mode: "background"`, preserving the
selected tab. Callers that require a visible switch must explicitly request
`delivery_mode: "foreground"`; success then requires exact target/tab identity,
activation, and settled live `current_url`, `title`, `heading`, `ready_state`, and `visibility_state`
readback bound to the committed frame and loader (including redirects). Any
stale document, identity, or visibility drift fails closed. A post-dispatch
failure remains a structured receipt with `dispatched`, activation/readback
state, target/tab identity, and a stable error code, so callers can distinguish
no delivery from a navigation that changed the page but failed verification. `browser_navigate`,
`browser_set_input_files`, and `browser_download` invalidate the tab snapshot.
Upload uses `allow_browser_input`; download is a separate
destructive grant (`allow_browser_download`) and CUA's host approval evidence.
`browser_dialog` only resolves page-owned JavaScript dialogs and requires the
exact current `dialog_id` for accept/dismiss.
Attaching to a logged-in Chromium profile keeps CUA's R2 gate: launch the Host
with `dcc-cua host --grant existing-profile` and also set the session's
`allow_browser_prepare` grant. Without both approvals, attachment is refused.
`allow_trusted_confirmation: true` on an exact window or desktop task grant
only permits the Host to ask for confirmation; it never authorizes an action
by itself. Embeddings that support confirmation must start the library Host
with `dcc_cua_host::run_with_confirmation_host` and a constructor-owned
`TrustedActionConfirmationHost`. The callback is not reachable through Host
IPC. On Windows, the packaged CLI Host installs a native user prompt at this
constructor boundary by default. Each request binds the session, task grant,
exact capability, PID/HWND when window-scoped, current
observation/accessibility state, intent, and complete action into a SHA-256
digest. The prompt serializes concurrent requests, identifies the exact target
and action type, defaults to denial, and never echoes action text or secrets.
The Host accepts only an inline decision echoing that exact digest, so a
decision cannot be replayed after the evidence, target, or action changes.
Missing, failed, or mismatched callbacks remain `approval_required`; explicit
denial or cancellation has its own typed error.

Exact-window foreground raw input declared as `navigate` or `ordinary_edit`
does not open an action-time prompt for bounded pointer actions or the closed
safe-key envelope. That keyboard envelope permits one unmodified alphanumeric
or reviewed navigation key; a held keypress permits only one or two unique
WASD/arrow keys for 1–10,000 ms. Modified shortcuts, function keys, Delete,
Backspace, multi-key taps, and unknown or malformed key tokens require
action-time confirmation regardless of the caller-provided intent. Keyboard
aliases, key lists, and modifier lists are classified before `input_kind`, so
they cannot borrow a task-granted semantic element policy. A backend selector
or any other non-keyboard routing field also excludes the closed safe-key
envelope. These actions still require `allow_raw_input: true`, the exact
session/task/window capabilities, and the latest Host observation. Text and
secret-value injection, sensitive intents, semantic controls classified above the task-grant tier,
window or process scope changes, and non-foreground raw input keep their
existing confirmation or refusal behavior.

For long-running automation, an embedding may instead collect explicit user
input before the task and install a constructor-owned
`TrustedTaskAuthorizationHost` through `HostSecurityServices`. The task grant
references only an authorization ID; Host IPC cannot mint or widen it. At
session open, the trusted host returns a short-lived in-memory lease
bound to the task grant, application label, PID/HWND, action kind, input kind,
risk category, secret/non-secret mode, and browser origin where applicable.
Host revalidates expiry and revocation before every otherwise-confirmed action.
An active exact-target lease runs without modal prompts. Once a session references a
task authorization, target changes, origin changes, category changes, expiry,
revocation, or validation failure return a typed `task_authorization_*` refusal
and never fall back to a popup. An explicit task-start denial returns
`task_authorization_denied`. Existing sessions without a task authorization
retain per-action confirmation. The packaged CLI does not trust flags or
redirected stdin as user presence, so it keeps per-action confirmation until a
trusted non-modal input broker is installed by its embedding.

Embeddings can construct that broker with
`dcc_cua_host::trusted_task_authorization_broker`. It returns two separate
capabilities: a move-only `TrustedTaskAuthorizationIssuer` retained by the
authenticated user-input surface, and a `TrustedTaskAuthorizationHost` trait
object installed in `HostSecurityServices`. After one explicit user input, the
issuer registers a task grant, application label, capability, either an exact
PID/HWND or the closed owned-browser spec, action/risk scopes, browser origins,
and expiry. It returns only a random
authorization ID for the task grant. Registrations are single-use for session
open, bounded to 24 hours, revalidated before every action, and revocable. The
issuer is not serializable or available through CLI arguments, environment
variables, stdin, or Host IPC, so those routes cannot mint or widen a task
authorization.

Browser tasks may instead register the closed owned-browser target
`browser=chromium, profile=isolated_new`. After the same single user input,
DCC-CUA starts an isolated profile in one upstream session and derives its PID,
native HWND, and exact CDP binding. Clients cannot provide or replace those
identities, an executable, a profile path, or a CDP endpoint. The authorization
card starts the session and reports `provider`, runtime version, PID, and HWND
before the first observation or input. Authorized HTTP(S) origins are copied
into the Host lease and checked for navigation and browser mutations. Repeating
`browser_prepare` is denied, and hidden upload controls use only
`browser_set_input_files`, never a native file chooser. Existing-browser
attachment still requires a separately authorized exact target.

Secret-bearing input uses an opaque `secret_handle` instead of putting the
secret in Host IPC. The packaged Host resolves that handle from the current
platform keyring only after the exact action confirmation succeeds. `text` and
`secret_handle` are mutually exclusive for `execute_action`,
`execute_desktop_action`, and `browser_type`; a missing constructor-owned vault,
unknown handle, stale observation, or denied confirmation fails closed. The
confirmation digest binds the handle, target, evidence, and action but never the
resolved value. Short-lived resolved buffers are redacted from `Debug` and
zeroized where the downstream action contract permits.

For credentials generated by a web portal, `clipboard_capture_secret` requires
the same exact session/capability, the latest window observation,
`allow_clipboard_read`, `allow_clipboard_write`, and
`allow_trusted_confirmation`. After the user confirms, Host reads only the
structured privacy-sensitive clipboard text, stores it under the requested
handle in the platform keyring, clears the clipboard, and returns only the
handle plus whether clearing succeeded. It never returns the captured value.
The extension semantic snapshot also never uses a form control's current value
as its accessible-name fallback, so passwords, API keys, and authentication
codes cannot leak through unlabeled controls.

Password, credential, authentication-code, security-setting, privacy-setting,
purchase, payment, publishing, and account controls inside an otherwise
eligible application are classified as `action_confirmation`: they can proceed
only after the exact task grant and an explicit user decision. This does not
make task grants blanket approval. Terminal/run-dialog controls, unverifiable
targets, protected operating-system authentication/password-manager surfaces,
scope escape, safety bypass, and automated human-verification circumvention
remain hard-denied. The task-grant gate defaults to false and does not follow
from raw-input, browser-profile, or session-escalation access.

On Windows, non-pixel semantic access reuses one exact PID/HWND UIA worker per
session; CUA remains the cross-platform, browser, and visual backend. An
`accessibility_snapshot` creates a fresh non-pixel observation and returns
`dcc-wuia:` tokens; click, toggle, set-text, and set-value can consume those
tokens without a screenshot, raw-input grant, or explicit window activation.
The target application may still choose to activate itself when it opens a
visible menu or dialog. The UIA worker script is loaded over its private stdin
pipe instead of being written to a same-user-writable temporary file or using
`ExecutionPolicy Bypass`. Readiness, requests, and responses carry a checked
protocol version, and the PowerShell policy-tier, sensitive-target, and stale
fence decisions have fixture-driven behavioral tests.
If the upstream Windows session start itself times out, Host reports
`upstream_session.state: "visual_only"` and never re-enters that unresponsive
driver session. Exact local WGC/UIA observation remains available only after
the existing explicit escalation grant; semantic or upstream evidence stays
unavailable until a new session is opened.

For native windows whose UIA/AX provider is unavailable (for example a
game-engine editor or custom-rendered DCC surface), `snapshot` first attempts
the semantic window capture. After an explicit `escalate_session` approval,
the same exact-window session first uses CUA's native platform capture for that
validated HWND. Only when exact-window capture is unavailable may it crop a CUA
desktop frame, which requires the target to be foreground so another window
cannot be mistaken for it. The standalone Windows Host enters Per-Monitor V2
DPI awareness before creating CUA, and resized exact-window frames are mapped
back to native target coordinates. Exact Windows visual
captures also attach the bounded Windows UIA tree when available; otherwise
they are marked `accessibility_available: false`. Coordinate actions remain
bound to their exact PID/HWND.
One-shot CLI `act` and friendly action commands accept the same `--activate`
flag, so their pre-action observation and mutation remain in that foreground
session instead of racing a separate activation process.
`desktop_snapshot` is a full-display visual discovery surface; it does not
widen an existing window session or grant desktop-wide mutation. For an
explicit desktop input scope, use `open_desktop_session`, then take a fresh
`desktop_session_snapshot` and call `execute_desktop_action`; raw input grant
and the exact desktop capability are required.
Action responses preserve the CUA SDK's structured result, text, degraded
status, and image attachments. Host transports image bytes as the negotiated
binary attachment, so Core does not need to decode base64 on its control path.
While `wait_for` is running, the same connection accepts `cancel` with the
exact session grant and window capability; the host returns both a cancellation
acknowledgement and the wait's cancelled terminal response. Other requests stay
ordered and are rejected until that wait completes.
On Windows, `activate` uses the input-gated exact PID/HWND foreground primitive;
other platforms retain CUA's scoped `bring_to_front` operation. It never
restores a minimized target, and the returned state is independently
revalidated. An activation timeout is `completion_unknown`, requires a fresh
observation, and never authorizes a blind retry or tears down live observation,
showcase, or recording ownership.
`set_window_frame` uses an exact PID/HWND `SetWindowPos` path on Windows and
CUA's native cross-platform mutation elsewhere, followed by independent
geometry readback. Moving or resizing a window invalidates native and browser
observations, so callers must snapshot again before the next action.
`invoke_menu` forwards CUA's live native menu-path resolver, never guesses
pixels, and fails closed for missing or ambiguous levels. A successful native
delivery can remain `unverifiable`; take a fresh accessibility snapshot and
verify the application state before the next mutation. Host sessions require
the explicit `allow_menu_invoke: true` task grant because native menu commands
can be destructive; the one-shot CLI command itself is the explicit request.

`zoom` is a typed, read-only crop of the latest window `snapshot`. It requires
the matching `observation_id`, keeps the exact PID/HWND binding, limits the
requested width to 500 pixels, and returns the JPEG through the negotiated
binary or shared-memory image transport. This is useful for dense DCC/UE/Fab
panels where the full observation is resized.

Window actions accept CUA's `element_token` as an alternative to
`element_index`; when both are supplied the token wins. They also accept
`delivery_mode: "background" | "foreground"`. The default is `background`,
which preserves the user's foreground and lets CUA select its accessibility or
synthetic-event route. Use `foreground` only after CUA reports that the
background route is unavailable.

Element indexes and tokens belong to the exact accessibility snapshot that
created them. They are not interchangeable with indexes or tokens from a pixel
snapshot, another backend, another window, or an earlier persistent session.
The friendly semantic CLI commands take a fresh accessibility snapshot and
rebind an index to its current token before delivery.

On Windows, a background UIA action can succeed even when Windows refuses to
restore the previous foreground window (or that window disappears during the
action). In that case the action result remains successful with
`action_executed: true` and reports the independent failure under
`foreground_restore.success: false`. Do not retry the input; take a fresh
observation and verify application state.

`get_session_state` reads CUA's live capture policy. `escalate_session` grants
pixel fallback only inside the existing exact-window scope and requires the
separate `allow_session_escalation: true` grant plus one of CUA's bounded
escalation reasons; it does not widen the session to desktop control.
The stable reasons are `ax_tree_pixel_mismatch`, `background_delivery_failed`,
`foreground_ineffective`, `no_window_target`, `uia_timeout`, and `other`.
Use `--escalation-detail` for the separate bounded audit note; `manifest`
publishes the same enum and meanings for machine discovery.
`cursor_tool` exposes `move_cursor`, `set_agent_cursor_enabled`,
`set_agent_cursor_motion`, `set_agent_cursor_theme`, and
`get_agent_cursor_state`; `move_cursor` is forced to `scope: "window"`, and the
session id is always injected by Host, so the mouse-shaped marker cannot be
redirected to another session or move the real system pointer.
CUA owns the cursor state, scoped motion, and native cursor/badge renderer on
Windows and Linux. `dcc-cua` embeds and installs its purple 12-state CUA v2
theme as `com.dcc-mcp.cursor` in the standard CUA theme store. The packaged
macOS Host selects the same theme through its private worker. Application
identity remains the real executable icon in the dynamic control banner.

The safety banner remains a native overlay rather than a WebView. Static SVG
art can be added as a cached theme layer later, while the app name and stop
state remain runtime text; the current Win32 backend draws the small capsule
directly and does not rerender an SVG every frame.

Extension tools from the live CUA inventory are available through `call_tool`
only after `open_session` grants `allow_native_tool: true`. The host injects
the exact session PID/HWND/session values from the live SDK schema and rejects
reserved arguments. CUA's canonical action-result vocabulary is always kept on
the dedicated routes, including platform-specific pointer actions. Click,
keyboard, browser, clipboard, recording, and
session-lifecycle tools stay on their dedicated grant-gated routes so the
extension surface cannot bypass observation or approval fences. This includes
CUA's legacy `page` compatibility tool; browser work must enter through the
exact-binding `browser_snapshot` route before any browser mutation.

The CLI `call` and `host-call` commands accept either `--json JSON` or
`--json-file PATH`; `host-call` reuses the persistent Host endpoint instead of
creating a new CUA driver for each request. `host-call --output FILE` consumes
both binary-frame images and negotiated shared-memory image descriptors.
`--json-file -` reads UTF-8 JSON from stdin, keeping large payloads off the
process command line. Host clients use `call_global_tool` for the grant-gated
global CUA tools `check_permissions`, `health_report`, `get_accessibility_tree`, `get_config`,
`set_config`, `replay_trajectory`, and `install_ffmpeg`; window tools continue
through `call_tool` with an exact session capability.

Example host requests:

```json
{"method":"list_apps","params":{}}
{"method":"launch_app","params":{"session_id":"session-1","grant":{"task_grant_id":"task-1","application_label":"Unreal Editor","allow_app_launch":true},"launch":{"name":"Calculator"}}}
{"method":"call_tool","params":{"session_id":"session-1","task_grant_id":"task-1","window_capability":"cua-window-...","tool":"debug_window_info","arguments":{}}}
{"method":"zoom","params":{"session_id":"session-1","task_grant_id":"task-1","window_capability":"cua-window-...","request":{"observation_id":"session-1-obs-1","x1":120,"y1":80,"x2":420,"y2":220}}}
{"method":"call_global_tool","params":{"grant":{"task_grant_id":"task-1","application_label":"Desktop","allow_native_tool":true},"tool":"health_report","arguments":{}}}
```

## Build and test

```powershell
vx cargo hakari generate --diff
vx cargo hakari manage-deps --dry-run
vx cargo fmt --all -- --check
vx cargo check --workspace --all-targets --locked
vx cargo nextest run --workspace --all-targets --locked
vx cargo test --workspace --doc --locked
vx cargo nextest run --locked -p dcc-cua-e2e --features gui-e2e --no-run
pwsh -NoProfile -File scripts/run-gui-e2e.ps1 -Binary target/debug/dcc-cua.exe
```

## CI/CD and release

CI checks layout, formatting, workspace tests, the locked release build, and a
real release-binary E2E on Windows, Linux, and macOS. The E2E validates the
machine manifest, platform identity, shared-memory negotiation, a spawned Host
handshake, lightweight ping, pipelined/streaming request correlation, invalid
request recovery (including a UTF-8 BOM on the first JSONL line), and the
CUA application/tool inventories. On macOS the release Host also proves the
bundled private-worker route and its AppKit-owned session cursor. All three
platforms build CUA's official Electron fixture and verify the
real launch -> scoped PNG snapshot -> semantic find -> input -> state oracle ->
exact-window raw-input coordinate click -> independent state oracle ->
exact browser binding -> semantic browser snapshot -> click/type -> independent
state oracle -> cleanup path. Each lane also builds CUA's native WPF, GTK3, or
AppKit fixture and verifies an exact `Window -> Arrange -> Left` menu path by
reading fresh UIA, AT-SPI, or AX application state. Concurrent-session coverage
uses two independent endpoint clients controlling two Electron windows on
Linux/macOS and two non-foreground WPF windows on Windows; it also verifies
same-named public sessions remain runtime-isolated, Host-wide stop propagation,
and serialized concurrent raw input. The same CLI
E2E launches a real endpoint Host and
checks ping plus pipelined application/tool discovery over the platform transport
(Windows named pipe or Unix socket).
An additional lifecycle E2E uses Host IPC itself to launch an isolated native
fixture (Calculator as a fresh instance on macOS), promote the launch into the
same private runtime session, record a real mutation to `action.json`, stop the
recording, terminate only that proven PID, and verify its windows disappear.
The release workflow packages one `dcc-cua` executable, assets, Skills, and
both MIT license notices. A newly created tag, its peeled commit, the checked-out
HEAD, and the GitHub Release target must resolve to the same commit. Each native
target is built once; a single workflow artifact then binds the complete asset
set to one artifact ID and content digest before create-only GitHub Release
attachment. Existing tags, releases, or assets are never rebuild targets.
Every consumer verifies the SHA-256 of the exact raw workflow artifact ZIP
before extracting it; the action-managed download is quarantined and is not the
published source. After create-only upload, the workflow reads the release back
and requires the exact target commit, asset names, sizes, digests, no extras,
and an unchanged native Latest release.

The native release contract contains one archive, SHA-256 sidecar, and install
manifest for each supported Rust target:

| Platform | Release target | GitHub runner |
| --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | `windows-latest` |
| Linux x64 | `x86_64-unknown-linux-gnu` | `ubuntu-latest` |
| macOS Apple silicon | `aarch64-apple-darwin` | `macos-26` |
| macOS Intel | `x86_64-apple-darwin` | `macos-26-intel` |

The upload job fails closed unless all four target triples have matching
archives, checksums, manifests, and aggregate provenance. Native executables are
not currently platform-signed: the published verification contract is SHA-256
checksums and recorded `not_performed` signing facts, not a code-signing claim.
Intel macOS remains a public support target
while the official `macos-26-intel` runner is available. Both macOS release
targets select Xcode 26.6 and fail closed unless the runner architecture and
macOS 26 SDK match; the runner and release contract must change together if
that hosted image or pinned toolchain is retired.

The release gate is intentionally closed while the product is still being
completed. Do not set the repository variable `DCC_CUA_RELEASE_READY=true`
until the full computer-control goal is accepted. The initial manifest starts
at `0.0.0`, so the first release-please release will be `0.1.0`; after that
release-please owns the manifest version and conventional-commit changelog.

The workflow uses the official `cargo-workspace` release-please plugin so all
workspace crates and `Cargo.lock` stay aligned. It does not publish crates;
`publish = false` remains intentional.

The CUA SDK revision is pinned in `Cargo.toml` and `Cargo.lock`. Release
integrity checks do not operate a user's desktop or prove real-user raw-input
behavior. Native desktop permissions, an interactive session, and separate
runtime acceptance are still required for real capture and input on Windows,
macOS, and Linux.
