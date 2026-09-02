---
name: steam-chromium-profile
description: Use the declarative Steam embedded-Chromium profile for bounded install-flow testing.
---

Use only project-owned DCC-CUA browser DOM or accessibility routes. Before any
observation attest `provider=dcc-cua`, the runtime version, exact PID, and exact
HWND. Rebind and snapshot again whenever any identity changes.

The install flow is snapshot → unique semantic `install_button` → click →
post-snapshot verification of `installed_state`. If the bridge is unavailable,
the target is duplicated/disabled, or the post-state is not observed, fail
closed. Never use coordinates, guessed labels, keyboard shortcuts, credentials,
or a bypass of Steam's own security confirmation.
