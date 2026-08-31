---
name: unity-runtime-cua
description: >-
  Observe and control a packaged Windows Unity player through exact-window
  dcc-cua, optionally using an application-owned read-only UI state companion.
license: MIT-0
metadata:
  dcc-mcp:
    dcc: computer-use
    layer: example
    compatibility: dcc-cua 1.7.1+
    version: "1.0.0"
    tags: "computer-use, unity, game-testing, read-only-state"
---

# Packaged Unity runtime CUA

Treat the profile as a template. Before installing it, replace the sample
executable and title selectors with values observed from the exact build. A
Unity player executable is product-named, so there is no safe universal
`*.exe` selector.

## Perception order

1. Discover and bind the exact PID/HWND with `dcc-cua list --app APP.exe
   --on-screen`. Do not use desktop scope.
2. If the build deliberately includes the read-only companion, run `dcc-cua
   profile-state --id unity-runtime --source unity-ui`. Accept its state only
   when `application.processId` and `application.windowId` equal the bound
   target and the coordinate-space dimensions agree with the current window.
3. If the source is absent or degraded, use `snapshot --pid PID --window-id
   HWND --pixels-only`, `zoom`, and a persistent `live_observation` stream.
   OCR may supplement readable text but is not a semantic authority for
   art-driven widgets.
4. Send every input through the exact-window DCC-CUA Host with a fresh visual
   observation. The companion is read-only and cannot authorize or execute an
   action.

Stop if the process/window identity drifts, the companion reports another
PID/HWND, its semantic tick regresses, its render size cannot be mapped to the
fresh snapshot, or the requested widget is not present. Never inject the
companion into a third-party build or bypass anti-cheat, login, consent, or
other security controls.
