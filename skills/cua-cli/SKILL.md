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
    version: "0.4.0"
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
   dcc-cua snapshot --pid $pid --window-id $hwnd --activate --output after.png
   ```

   Prefer an accessibility element index/token. Use coordinates only for a
   custom-drawn surface after a fresh pixel snapshot. A semantic action gets a
   fresh accessibility observation; do not reuse stale element indexes or mix
   tokens from pixel snapshots, other backends, windows, or sessions.

4. Verify state after every mutation. A successful input call proves that input
   was delivered, not that the application reached the requested state. Use a
   post-snapshot, changed title/tree/value, or an independent application state
   check as the acceptance oracle. If a Windows background UIA result reports
   `action_executed: true` with `foreground_restore.success: false`, do not
   retry the input; only the independent foreground restoration failed.

5. Use `desktop-snapshot`/`desktop-act` only for a deliberately desktop-scoped
   target that cannot be represented by an exact window. Keep the scope and
   coordinate source explicit.

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
connection and keep its `open_session` active. The Host-owned ControlBanner and
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
After every held interval, take a fresh exact-window snapshot and verify the
game state; stop or release input at a checkpoint rather than leaving a key
logically held.

## Profile-guided routing

A semantic profile is not an automation script. It describes how an agent
recognizes an application, names a task area and target, and chooses the owner
of execution:

1. Inspect the profile before acting:

   ```powershell
   dcc-cua profiles
   dcc-cua profile --id fab
   ```

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

## Long tasks and safety boundaries

Represent long work as checkpoints such as `discover → bind → act → verify →
next checkpoint`, with a deadline for each stage. On `desktop_unavailable`, a
disconnected session, policy/authorization failure, or `user_interrupted`, stop
the current stage and recover the environment before retrying; do not switch to
another input technology.

The ControlBanner and target frame are visible on the physical desktop and in
the exact-window observation so users and agents can verify that control is
active. The banner remains owned by the Host and must not be painted into game
content or synthesized by the client.
