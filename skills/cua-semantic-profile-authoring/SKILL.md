---
name: cua-semantic-profile-authoring
description: >-
  Author and validate user or studio JSON semantic profiles for dcc-mcp-cua.
  Use when adding application vocabulary, selectors, surfaces, routes, or target
  aliases; use the official built-ins for normal runtime work.
license: MIT-0
metadata:
  dcc-mcp:
    dcc: computer-use
    layer: infrastructure
    compatibility: dcc-mcp-cua 0.1+
    version: "0.1.0"
    search-hint: "custom semantic profile JSON selectors surfaces targets route dialog style dcc-mcp-cua"
    tags: "computer-use, infrastructure, read-only"
---

# dcc-mcp-cua semantic profiles

Built-in `ue`, `maya`, and `fab` profiles are the official defaults. A user or
studio may provide a compatible JSON file through `--profile-file`; custom
profiles extend vocabulary and routing without bypassing the CUA Host's exact
window scope, fresh-observation fence, permissions, or input owner.

## Authoring contract

- Keep `schema_version` at `1` until the project publishes a migration.
- Give the profile, every surface, and every target stable IDs.
- Use selectors for real process names, localized title fragments, or URL hosts.
- Declare the surface's authoritative route (`unreal_typed_api`, `browser_dom`,
  `accessibility`, `os_native_dialog`, or `visual_fallback`). A route is a hint
  for the owning adapter; it is not proof of scene or account state.
- List only actions the target actually supports. Keep destructive confirmation
  enabled for downloads, exports, purchases, and other irreversible operations.
- Do not put credentials, private paths, direct SendInput logic, or host API code
  in a profile.

## Validate and inspect

```powershell
dcc-mcp-cua profile --profile-file C:\profiles\studio-maya.json
dcc-mcp-cua profile --profile-file C:\profiles\studio-maya.json --app maya.exe --surface home --query new_scene
```

The first command validates and prints the normalized profile without starting
an application. The second command binds the profile to a live target and
returns the current semantic match set. Only execute an action after confirming
the target has exactly one current element and the profile allows that action.
