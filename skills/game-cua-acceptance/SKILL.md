---
name: game-cua-acceptance
description: >-
  Test a packaged Windows game through the standalone dcc-cua exact-window
  Host, including persistent ControlBanner sessions, held WASD movement,
  checkpoint verification, upgrade selection, failure recovery, and final
  gameplay evidence. Use for black-box game acceptance; use a typed engine
  adapter when an authoritative engine API exists.
license: MIT-0
metadata:
  dcc-mcp:
    dcc: computer-use
    layer: example
    compatibility: dcc-cua 0.4+
    version: "0.1.0"
    search-hint: "packaged Windows game CUA acceptance WASD held keypress persistent host-jsonl ControlBanner snapshot upgrade survive victory evidence"
    tags: "computer-use, game-testing, acceptance, destructive"
---

# Packaged game acceptance with dcc-cua

Use this skill for a real packaged game that exposes only a native window and
keyboard/mouse controls. It is a thin workflow on top of `cua-cli`; it does not
invent a game-specific driver or bypass the exact-window Host boundary.

## Required setup

1. Start the named game visibly. On Windows, confirm the exact process and HWND
   with `dcc-cua list --app <game.exe> --on-screen`.
2. Run `dcc-cua doctor` and keep the reported interactive desktop/input state.
   A snapshot may prove observation while input remains unavailable; do not
   treat that as permission to act.
3. For a task longer than one turn, start one persistent `host-jsonl` session
   and call `open_session` with the exact PID/HWND and a task grant. Confirm:
   `banner.visible=true`, `banner.target_frame_visible=true`, and
   `input_state.status=ready`. Keep this connection and banner alive until the
   terminal result.
4. For a custom-rendered real-time game, start `live_observation` immediately
   after `open_session`, even if the initial accessibility snapshot contains a
   title bar. UIA title-bar nodes are not gameplay evidence and can otherwise
   select the generic tap route; held movement requires a newer WGC frame and
   the exact-window scoped input route.

## Game control loop

Use this checkpoint sequence:

`discover → exact snapshot → start/restart → held movement → snapshot →
select upgrade → snapshot → repeat → terminal proof`

- Bind every request to the exact process ID and window handle. Never widen to
  desktop scope because the game is custom-rendered and has no useful UIA tree.
- Treat a plain keypress as a tap. For real-time movement use one bounded
  foreground held keypress, for example:

  ```json
  {"action":"keypress","keys":["W","D"],"duration_ms":1000,"delivery_mode":"foreground"}
  ```

  Use short single- or diagonal-direction intervals and fresh snapshots
  between them. Choose each direction deterministically from the latest frame:
  estimate the player's center, identify the nearest enemy or collision
  footprint, and steer along the largest-clearance direction. Prefer a
  diagonal escape when two axes are blocked, then replan after the next frame.
  Do not use random movement or a fixed square patrol, and do not issue a long
  unverified stream of input.
- After each action, verify an independent visual state change: timer/health,
  player/enemy position, level-up overlay, pause state, defeat, or victory.
  “Input sent” alone is not game success.
- For real-time combat, treat avoidance as a closed-loop controller rather than
  a macro: `snapshot → detect player/enemy/obstacles → choose clearance vector
  → held WASD/diagonal input (≤2 s) → snapshot`. If the frame is degraded or
  the target is not visible, stop movement and recover observation first.
- Keep the live observation active for the whole combat loop. A snapshot backed
  only by UIA metadata is suitable for semantic menu actions, not for movement
  or enemy avoidance.
- When a level-up/choice overlay appears, select only from the visible choices
  (for keyboard games, usually `1`, `2`, or `3`), then snapshot again. Do not
  reuse the previous observation ID.
- At checkpoints, pause with the game’s documented pause key if possible. If
  the Host reports `user_interrupted`, security, authorization, or unavailable
  desktop/input state, stop and recover; never blind-retry or switch to generic
  Computer Use.

## Acceptance evidence

Save exact-window PNGs and structured responses for:

- cold start/title and first gameplay frame;
- at least one held-WASD interval with post-action verification;
- upgrade/choice handling;
- meaningful progress checkpoints;
- the final victory/survival screen, score, timer, or explicit success state.

Record the capture provenance, PID, HWND, observation ID, and banner state with
the evidence. A local build or a successful input response is not final proof.
If CUA lacks a required capability, first preserve the reproduction and
structured error, then make the smallest regression-safe fix in this project,
validate it, submit the PR, and resume the same acceptance workflow.
