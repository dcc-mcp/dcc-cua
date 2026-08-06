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
    compatibility: dcc-cua 0.1+ on Windows, macOS, or Linux.
    version: "0.2.0"
    search-hint: "dcc-cua CLI exact window snapshot act verify UIA visual control banner long task recovery"
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
   fresh accessibility observation; do not reuse stale element indexes.

4. Verify state after every mutation. A successful input call proves that input
   was delivered, not that the application reached the requested state. Use a
   post-snapshot, changed title/tree/value, or an independent application state
   check as the acceptance oracle.

5. Use `desktop-snapshot`/`desktop-act` only for a deliberately desktop-scoped
   target that cannot be represented by an exact window. Keep the scope and
   coordinate source explicit.

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
   `surface` and `target`. Treat `supported_actions` as an allow-list.
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

The ControlBanner and target frame are visible on the physical desktop so users
know control is active, while the indicator windows are excluded from CUA
observations. That is intentional and must not be “fixed” by painting the
banner into screenshots.
