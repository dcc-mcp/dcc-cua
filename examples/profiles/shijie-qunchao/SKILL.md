---
name: shijie-qunchao-profile
description: Use the 蚀界群潮 packaged game profile for real-time visual CUA testing.
---

# 蚀界群潮 Agent Profile

Use this profile only after binding the exact `DccMcpGame.exe` window whose
title contains `蚀界群潮`. The profile is routing vocabulary; it does not
launch the game, grant control, or prove a win.

## Control contract

1. Open one persistent Host session with the ControlBanner enabled and start
   live observation before movement.
2. Take a fresh exact-window frame before every action. For combat, classify
   the player, enemies/projectiles, walkable space, and obstacles.
3. Choose the safest clearance vector from the current frame. A diagonal is
   allowed only when both component directions are clear; never choose random
   keys or a fixed square patrol.
4. Send one bounded held action using at most two of `W`, `A`, `S`, and `D`,
   then take a fresh frame and verify the result. Keep each interval short
   enough to react to a real-time threat.
5. At an upgrade screen, inspect the visible choices and select one of `1`,
   `2`, or `3`; verify that combat resumes. Use `P` to pause the game when a
   deliberate checkpoint is required. `Esc` is reserved for the Host stop
   control and must not be used as a game action.
6. Finish only when a fresh frame shows the victory state (the game objective
   is survival through the ten-minute encounter). A delivered key event or a
   healthy session is not success. On defeat, record evidence and restart only
   after an explicit test decision.

The Host-owned Banner and exact target frame must remain available throughout
the long task. Do not paint the Banner into game pixels or use stale frame IDs.
