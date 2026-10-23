---
name: cua-cli
description: >-
  Operate the standalone dcc-cua CLI for exact-window discovery, scoped
  snapshots, semantic or visual actions, post-action verification, and bounded
  long-running UI tasks. Use this project skill instead of generic Computer Use
  or a DCC-specific MCP sidecar.
license: MIT-0
metadata:
  dcc-mcp:
    dcc: computer-use
    layer: infrastructure
    compatibility: dcc-cua 0.2+ on Windows, macOS, or Linux.
    version: "0.4.2"
    search-hint: "dcc-cua CLI exact window multilingual profile localized aliases snapshot act verify UIA visual control banner long task recovery"
    tags: "computer-use, ui-control, infrastructure, read-only"
---

# Standalone dcc-cua CLI

This project is an independent CUA control plane. It does not depend on Maya
MCP, a DCC adapter, `dcc-mcp-cli`, or generic Computer Use. Use the released
`dcc-cua` binary for UI work instead of generic Computer Use or the in-app
Browser skill. Keep authoritative scene, asset, or engine operations on an
available typed DCC-MCP route; use dcc-cua for the exact-window UI path that the
typed route cannot cover.

For resident-Host commands (`ping`, `host-call`, `host-batch`), a
`host_not_running` error means the selected local endpoint was missing or
refused the connection. Run `dcc-cua host-ensure` before retrying; preserve the
same `--endpoint PATH` if one was supplied. A successful `--spawn` probe checks
its temporary stdio Host, not the resident endpoint. `call --tool NAME` is the
direct driver surface; use `list` or `host-call --method list_windows --json
'{}'` for window inventory. Do not treat unrelated permission, handshake, or
`invalid_action` failures as an absent Host.

## Connection lifetime

Keep one `mcp-server` stdio connection open from `start_task` through all
`dcc_cua_task_call` observations/actions to `stop_task`. Task IDs belong to
that MCP connection; opaque observation/element/browser/window-state refs
belong to their owning session and must still be current. No cross-connection
resume is supported. After disconnect, expiry or stop, start a new authorized
task and take a fresh observation. Inspect `manifest.connection_lifetime`.

Each one-shot CLI invocation creates a new session, even with the same
`--session` label. Do not copy opaque element tokens from `snapshot` to a later
`act`. One-shot `act` already captures fresh evidence internally; element
indexes are re-resolved against that new tree and can drift. For exact token
continuity use the persistent MCP task loop. On `stale_observation`, reobserve
and reassess in the same live task rather than blindly replaying input.

`host-call` also creates a new Host connection each time. A resident Host or
`host-ensure` does not preserve the old connection's sessions or issue task
grants. `dcc-cua tools` lists driver tools, not Host IPC methods: `start_session`
is not a Host IPC method. Host clients use `open_session` with a `TaskGrant`
and keep the returned capability on that connection. Agent clients should use
`start_task` then `dcc_cua_task_call`, which injects the private session binding.
For a visual-only task, select `observation_mode: "pixels_only"` at startup
to avoid unnecessary accessibility traversal. Keep capture diagnostics off
except while measuring; their hashes and histograms add work.

If `get_window_state` reports `frame_recovery`, follow its `next_call` on the
same task, then obtain fresh native state before resizing or observing. A
maximized or minimized window cannot mint a frame token. If recovery requires
a new task, request its indicated method within the authorized scope; do not
ask the user to restore the window until the exact native recovery route has
failed. Never borrow tokens or infer permission from the recovery hint.

`host-jsonl`/`host-batch` preserve their own connection, not authorization by
themselves. Treat `invalid_request` as a schema/validation error and
`session_not_found` as absence on the current connection; neither is fixed by
starting another Host or borrowing a different connection's capability.

## Control loop

1. Discover the real target before acting:

   ```powershell
   dcc-cua doctor
   dcc-cua list --on-screen
   dcc-cua list --app maya.exe --on-screen
   dcc-cua profiles
   ```

2. Bind one exact process/window. Prefer `--pid` plus `--window-id`; use a
   process or title selector only for discovery and then rebind to the returned
   identity. Never widen an exact-window session to the whole desktop because a
   child surface lacks semantic nodes.

3. Use the latest observation as the only action coordinate/token source:

   ```powershell
   dcc-cua snapshot --pid $pid --window-id $hwnd --activate --output before.png
   dcc-cua act --pid $pid --window-id $hwnd --action-json '{"action":"click","element_index":12,"delivery_mode":"foreground"}'
   dcc-cua act --pid $pid --window-id $hwnd --action-json '{"action":"click","x":100,"y":100,"delivery_mode":"foreground"}' --observation-width $imageWidth --observation-height $imageHeight
   dcc-cua snapshot --pid $pid --window-id $hwnd --output after.png
   dcc-cua verify --pid $pid --window-id $hwnd --expect-json '[{"window":{"exists":true}}]'
   dcc-cua clipboard-write --pid $pid --window-id $hwnd --text "bounded text"
   dcc-cua clipboard-read --pid $pid --window-id $hwnd --include-text
   ```

   Prefer an accessibility element token within the same persistent task. A
   one-shot element index is resolved again against its new snapshot. Use coordinates only for a
   custom-drawn surface after a fresh pixel snapshot. A semantic action gets a
   fresh accessibility observation; do not reuse stale element indexes or mix
   tokens from pixel snapshots, other backends, windows, or sessions.

   For coordinate actions, read `$imageWidth` and `$imageHeight` from the
   producing snapshot's `coordinate_space`, whose dimensions come from the
   encoded PNG IHDR. Both `--observation-width` and `--observation-height` are
   required. With device-pixel `window-state` bounds, the corresponding desktop
   point is `screen_x = bounds.x + x * bounds.width / observation_width` and
   `screen_y = bounds.y + y * bounds.height / observation_height`. Do not apply
   `screen-size.structuredContent.scale_factor`; the bounds are already device
   pixels.

   `snapshot`, `act`, `verify`, `clipboard-read`, and `clipboard-write` share
   `--app`, `--pid`, `--window-id`, and `--title`. Treat multiple selectors as
   one conjunctive identity. Any conflict, missing or ambiguous match, or live
   PID/HWND/title drift must stop the command; never fall back to app-only scope.

   `snapshot --activate` falls back to capture on the same exact PID/HWND-bound
   session only when the typed activation error explicitly reports
   `background_delivery_viable: true`. Detect this safe degradation through
   `activation.status == "refused_fallback_background"`. Do not retry or widen
   the target for any other activation error.

   On Windows, if the exact live window has no responsive accessibility provider, use the
   explicit provider-free route:

   ```powershell
   dcc-cua snapshot --pid $pid --window-id $hwnd --pixels-only --output frame.png
   ```

   Add `--activate` when explicitly authorized to bring this exact target to
   the foreground. Activation and capture use the same PID/HWND session and
   skip UIA; capture still requires fresh native identity, geometry, overlay
   hide/acknowledgement, and occlusion proof. Activation does not waive capture
   or input checks. Do not combine
   `--pixels-only` with `--escalate`. A refused native capture reports typed,
   content-free `error.details.capture` metadata; raw backend errors and
   foreign window titles are never included in this diagnostic.

   This route requires both selectors and never publishes a whole-desktop
   screenshot. When native window-content capture cannot prove one exact HWND,
   Windows may use the `VisibleDesktopCrop` fallback: it reads only the target's
   physical rectangle from the desktop DC, and only after complete z-order,
   visibility, PID/HWND, native instance, bounds, and DPI evidence is proven
   before and after capture. Verified native window-content capture may remain
   available behind an occluder. A partially occluded desktop crop masks proven
   foreign rectangles with opaque black; only its visible target pixels are
   published. Ambiguity or drift discards the pixels. Pointer input still needs
   a reachable, unredacted point and keyboard input requires full visibility.
   Treat this visible desktop-rectangle provenance separately from
   native window-content pixels, `pixels_only`,
   `accessibility_unavailable_degraded`, and
   `accessibility_timeout_degraded`.
   Any identity, bounds, DPI, generation, visibility, or occlusion change
   invalidates the frame; rediscover and take a fresh observation.
   If a returned frame contains foreign UI despite a successful proof, stop
   input and retain the image and receipt. A success envelope does not override
   contradictory pixels. For explicit Windows metadata diagnostics, run
   `dcc-cua capture-proof --pid $pid --window-id $hwnd`: it starts neither UIA
   nor a Host, reads no pixels, and never activates or moves a window. Its
   bounded content-free trace records the production root traversal, native
   geometry and DPI/DC state before and after a compositor flush. The result
   always has `authorizes_capture_or_input=false`; it cannot certify pixel
   ownership or replace a fresh accepted observation. This command accepts
   only the exact numeric PID/HWND selectors.
   On macOS and Linux the manifest omits this Windows-only capability and
   `--pixels-only` returns `BackendUnavailable`; do not advertise or invoke it.
   If standalone `accessibility` returns `no_accessibility_provider`, treat the
   result as non-retryable for that window class and use `snapshot
   --pixels-only` plus OCR or another perception layer. A
   `backend_unavailable` error is a provider/backend failure, not evidence that
   the window class permanently lacks a provider.

4. Verify state after every mutation. A successful input call proves that input
   was delivered, not that the application reached the requested state. Use a
   post-snapshot, changed title/tree/value, or an independent application state
   check as the acceptance oracle. `window.exists=true` proves liveness only.
   A successful `clipboard-write` also needs an exact-bound value readback for
   non-sensitive test data or an application-specific pasted-value check; never
   expose private clipboard content as evidence. If a Windows background UIA result reports
   `action_executed: true` with `foreground_restore.success: false`, do not
   retry the input; only the independent foreground restoration failed.

5. Use `desktop-snapshot`/`desktop-act` only for a deliberately desktop-scoped
   target that cannot be represented by an exact window. Keep the scope and
   coordinate source explicit.

For MCP pixel tasks, use `interaction_recovery` in verified native window-state
or activation-refusal responses to separate observation from input. An authorized
snapshot does not require foreground activation; fresh capture proof still
applies. `foreground=true` does not certify input readiness. Native
`background_delivery_viable` hints do not authorize background `execute_action`
in the closed bridge. If activation is refused, retain diagnostics and stop the
input chain; continue authorized passive observations if useful. Do not blindly
retry activation or repeatedly ask the user to switch windows. Repair the native
or Agent Host handoff before another input attempt.

Windows pixels-only dragging requires an explicit raw-input `drag` action scope.
Use two screenshot-local `path` endpoints, optional `button` (left by default),
`duration_ms` from 1 to 10000 (default 500), and `steps` from 1 to 200 (default 20).
Do not mix coordinates, modifiers, text, selectors or backend overrides into the
drag. Take a fresh snapshot first: the whole line must be unredacted and each
sampled point natively reachable on the unchanged foreground target. A failure
releases the operation's held button and consumes the observation; never blindly
resume or retry. Verify the actual node position or other intended result after
delivery. Older runtimes whose closed action list omits `drag` cannot use it.

For a Windows standard native popup menu (`#32768`) without accessible menu
items, the public MCP task route can cancel that exact active popup without
keyboard input or activation. Start an exact `window` / `pixels_only` task with
`allowed_methods=["get_window_state","change_window_state"]` and the independent
action scope
`{"action":"dismiss_menu","input_kind":"window_state","secret_input":false,"authorization_category":"window_state"}`.
This scope permits only `dismiss_menu` in `change_window_state`; it does not
grant activation, restoration, application close, raw input, or frame movement.

Call `get_window_state` with empty parameters, then copy the returned
`state.menu_dismiss_window_state_id` into
`change_window_state` parameters
`{"operation":"dismiss_menu","window_state_id":"<menu token>"}`. The token
is session-bound, purpose-bound, one-use, and expires after five seconds.
Ordinary `window_state_id` frame tokens and snapshot observation IDs cannot
authorize this operation. Reading an ordinary or inactive window cannot mint a
menu token and sends no cancellation. Never guess another owner or target.

The native proof requires one unique active standard popup and its exact
same-process, same-thread menu owner from native GUI thread metadata. The
operation sends at most one bounded standard `WM_CANCELMODE` to that owner,
without selecting a menu item. A zero native message result can be normal;
delivery alone is not confirmation. Confirmed success requires all three
native readbacks: `popup_hidden_or_gone=true`, `menu_inactive=true`, and
`owner_alive=true`. This never claims that the application closed.

A pre-dispatch refusal reports `action_attempted=false` and known completion.
Any failure after dispatch reports `action_attempted=true`,
`completion=unknown`, and `effect_unknown=true`; do not blindly retry or call
that success. Both cases report no keyboard/mouse input and consume prior
metadata and action observations. Reobserve and explicitly rebind when the
popup is gone; the route never follows a replacement popup or owner.

For fast visual loops on Windows, keep one `host-jsonl` connection open, grant
`allow_live_observation: true` in `open_session`, then send
`live_observation_start` with a `params.request` payload of
`{"fps":10,"max_dimension":1568}`. Subsequent `snapshot` requests wait
for and return only a newer exact-window WGC frame without UIA work. Query
`live_observation_state` for capture/replacement counts and always send
`live_observation_stop` before `stop_session`. Use a separate
`accessibility_snapshot` only when semantic controls are required.
For custom-rendered real-time games, start live observation before the first
movement action even when the initial snapshot reports a few UIA nodes. Those
nodes may be only a title bar; held movement must remain on the exact-window
WGC/scoped-input route rather than falling back to a generic key tap.
For a custom-rendered transition, set `capture_after: true` and a bounded
`post_snapshot_delay_ms` (up to 5000) so one request returns the settled frame;
do not sleep and issue a redundant standalone snapshot. When `capture_after`
is false, treat `observation_required: true` as a mandatory snapshot fence.

For a task that lasts longer than one turn, use one persistent `host-jsonl`
connection and keep its `open_session` active. Set one bounded
`idle_timeout_ms` for the logical task (15 minutes by default); every authorized
session request renews it. If it expires, open a fresh session and take a fresh
observation—never replay the previous action. Embedders should prefer
`dcc_cua_client::LogicalTaskSession`, which owns one negotiated connection and
injects the exact session credentials into every request. The Host-owned ControlBanner and
target frame are the user-visible control lease; keep them visible for the whole
task and close the session only at a checkpoint or terminal result. Do not
replace a long task with repeated one-shot CLI processes, because that drops the
same-session banner and loses observation/session continuity. Prefer the
`snapshot → execute_action → snapshot` loop on that one connection. If the
Host reports a suspended input state, take a fresh snapshot and inspect the
structured reason; never blind-retry an input whose dispatch status is unknown.

For real-time games and other held-key controls, use a bounded held keypress:

```json
{"action":"keypress","keys":["D"],"duration_ms":1000,"delivery_mode":"foreground"}
```

`duration_ms` is a bounded key-down/key-up interval (currently at most 10
seconds). It accepts one or two unique WASD/arrow keys with no modifiers, so a
game that supports diagonal movement may use `{"keys":["W","D"]}`. A plain
`keypress` is a tap and is not a substitute for a held WASD movement input.
The Host watches the shared Escape interrupt while the key is held, releases
every key it pressed, and returns `user_interrupted`; this makes the visible
ControlBanner stop control effective even during a long interval. After every
held interval, take a fresh exact-window snapshot and verify the game state;
prefer short intervals and stop or release input at a checkpoint rather than
leaving a key logically held.

## Existing-profile browser setup

An existing-profile `browser_prepare` refusal with reported setup-page effects
may include `structuredContent.recovery.kind: "existing_profile_setup"` for
the last validated native PID/window ID. Follow its manual setup guidance;
recognized Edge/Chrome applications include the internal setup URL and
`Allow remote debugging for this browser instance` checkbox label. Unknown
applications receive no guessed URL. Do not retry preparation automatically.
If `cleanup.status` is `manual_cleanup_required`, have the user identify and
close any leftover setup tab; the SDK has not supplied reusable tab-cleanup
authority. `closed_by_driver` and `not_opened` are reported prior effects, not
fresh tab or setting evidence. A reported `setup_tab_creation_unproven: true`
requires manual review of that exact window even if `opened_setup_page` is false;
recovery reports `manual_cleanup_required` for this uncertain outcome.
Keep the original refusal, revalidate the exact
target and obtain fresh authorized observations before continuing. Recovery
guidance never grants input, navigation, tab closure, or extra methods.

## Profile-guided routing

A semantic profile is not an automation script. It describes how an agent
recognizes an application, names a task area and target, and chooses the owner
of execution:

1. Inspect the profile before acting:

   ```powershell
   dcc-cua profiles
   dcc-cua profile --id fab
   ```

   The default `profiles` result contains only usable entries. If an installed
   package is missing or malformed, inspect it explicitly with `dcc-cua
   profiles --state invalid`; this diagnostic view reports its path, validation
   reason, and remediation hint. Use `--state all` only for a combined audit.

2. Match a selector against the real native window or URL, then choose one
   `surface` and stable target ID. `supported_locales` describes available
   aliases; every locale remains active, so do not guess the UI language or
   translate the target ID. Treat `supported_actions` as an allow-list.
3. Dispatch by `surface.route`:
   - `accessibility`: the profile CLI may inspect or execute against exactly one
     fresh match;
   - `unreal_typed_api`: use the owning Unreal adapter/Skill;
   - `browser_dom`: use dcc-cua's exact-bound browser route;
   - `os_native_dialog`: bind the exact OS dialog and use native control;
   - `visual_fallback`: take a fresh exact-window snapshot and use scoped CUA.
4. Treat `fallback` as a reference, not a jump. Load its `profile_id` and
   `surface_id`, discover and bind the new exact window, and take a new
   observation. Never carry an element token, index, or coordinate across the
   transition.

The optional `dcc-cua-browser-extension` component is developed in this
repository, built for Chrome, Edge, and Firefox, and released independently
from the native binary. Route browser work as follows:

1. Keep CDP as the default. Run `dcc-cua browser-extension plan --browser
   chrome|edge|firefox --extension-id PUBLISHED_ID --cdp-state
   available|unavailable` when provider choice is unclear.
2. If CDP is unavailable and the native host is not registered, ordinary users
   must first install the signed store extension. With the exact published ID,
   run `dcc-cua browser-extension install-native-host --browser BROWSER
   --extension-id PUBLISHED_ID`. This registers the bridge only; it never
   silently sideloads an unpacked extension.
3. Ask the user to click the extension action in the exact tab. On the existing
   logical-task Host session, call `browser_extension_status`; select only a
   provider whose paired `origin` matches the task.
4. Call `browser_extension_call` with that `provider_id`, `expected_origin`,
   method, params, and the exact task session credentials. Reuse the same Host
   connection/session until completion or idle expiry.

The extension-owned `browser_dom` route requires that explicit pairing and a
successful versioned Native Messaging handshake. Never infer installation from
the native dcc-cua version, and never fall back to CDP after an extension
permission, origin, pairing, identity, or protocol failure. The Host manifest
must advertise `browser_provider:extension.v1`; otherwise treat the extension
as unavailable rather than partially active.

## Machine-readable command failures

Parse stdout for both success and failure from one-shot CLI commands. A normal
command failure returns a non-zero exit code and exactly one JSON envelope such
as `{"success":false,"error":{"code":"command_failed","message":"dcc-cua could not complete the command"}}` on
stdout. Do not merge stderr into stdout: stderr is reserved for fixed safe
process diagnostics and never carries a duplicate envelope. Long-lived
`host-jsonl`, Native Messaging, and MCP routes retain their own framed stdout
protocols. Treat the one-shot error identity as deliberately lossy: codes are
bounded local categories and messages are fixed, so raw command/option text,
error strings, paths, arguments, tokens, and remote payloads are never public.

For an accessibility surface, use the identity returned by discovery:

```powershell
dcc-cua list --app maya.exe --on-screen
dcc-cua profile --id maya --pid $pid --window-id $hwnd --surface home --query new_scene
dcc-cua profile --id maya --pid $pid --window-id $hwnd --surface home --query new_scene --action click --activate
```

If `ue/fab/download` is unavailable, its declared fallback is
`fab/launcher_download`. Rebind Epic Games Launcher and continue through its
`visual_fallback` surface; the profile does not launch the application or click
for the agent. Destructive downloads and trusted confirmations still require
their explicit task grants.

After `browser_download` completes, consume its verified `file_path` or
`artifact_ref` directly when present; completion alone does not prove a location.
`stored_filename` identifies the actual artifact;
`suggested_filename` is separate display metadata and may be `null` with an
`unavailable` or `invalid` status. Never derive a destination path or rename a
file from that suggestion. The reported MIME type comes from the stored bytes.

## Long tasks and safety boundaries

Represent long work as checkpoints such as `discover → bind → act → verify →
next checkpoint`, with a deadline for each stage. On `desktop_unavailable`, a
disconnected session, policy/authorization failure, or `user_interrupted`, stop
the current stage and recover the environment before retrying; do not switch to
another input technology.

The ControlBanner is visible on the physical desktop and its structured status
(`visible`, `healthy`, `target_frame_visible`, and `interrupted`) is returned by
the Host so users and agents can verify that control is active. Exact-window
WGC frames intentionally contain only the target window; the external banner
is not expected to appear in those PNG pixels. The banner remains owned by the
Host and must not be painted into game content or synthesized by the client.
