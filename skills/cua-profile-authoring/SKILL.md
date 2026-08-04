---
name: cua-profile-authoring
description: >-
  Create, inspect, and safely extend dcc-mcp-cua semantic profiles for official
  application defaults or user-authored JSON. Use for profile schema, selectors,
  semantic surfaces, route ownership, dialog policy, and profile validation.
license: MIT-0
metadata:
  dcc-mcp:
    dcc: computer-use
    layer: infrastructure
    compatibility: dcc-mcp-cua 0.1+
    version: "0.1.0"
    search-hint: "dcc-mcp-cua semantic profile JSON official custom selectors surfaces targets routes dialog policy"
    tags: "computer-use, infrastructure, read-only"
---

# dcc-mcp-cua semantic profiles

Profiles are application vocabulary and route hints. They do not replace the
CUA Host's exact-window identity, fresh-observation fence, authorization, input
owner, or independent application state authority.

## Official and user profiles

- Official defaults live in the Rust crate under `profiles/` and are covered by
  Rust tests. Add one only when the vocabulary is stable and reusable across
  tasks.
- Users and studios can supply a compatible JSON file with `--profile-file`
  without changing the binary or editing application preferences.
- Keep host-specific execution in the owning adapter/route. The current CLI
  directly executes semantic actions only for `accessibility` surfaces; routes
  such as `unreal_typed_api`, `browser_dom`, `os_native_dialog`, and
  `visual_fallback` are declarations for the owning control path.

## Contract

Each profile declares:

- `schema_version: 1`, a stable `id`, and `display_name`;
- selectors containing real process names, localized title fragments, or URL
  hosts;
- named surfaces with a route and targets;
- target aliases (`names`/`automation_ids`) and an explicit action allow-list;
- `dialog_style` and `destructive_confirmation_required`.

Do not put credentials, private secrets, direct input injection, arbitrary shell
commands, or application API code in profile JSON. Keep destructive actions
confirmation-aware.

## Inspect and validate

```powershell
dcc-mcp-cua profiles
dcc-mcp-cua profile --id maya
dcc-mcp-cua profile --profile-file C:\profiles\studio.json
dcc-mcp-cua profile --profile-file C:\profiles\studio.json --app maya.exe --surface home --query new_scene
```

The first two commands inspect official profiles; the third validates and prints
a user profile without starting an application. The live form binds the profile
to one target, takes a fresh semantic observation, and reports current matches.
Execute an action only when the profile route supports it and exactly one live
element has a current locator.
