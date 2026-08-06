---
name: cua-profile-authoring
description: >-
  Create, inspect, and safely extend dcc-cua semantic profiles for official
  application defaults or user-authored JSON. Use for profile schema, selectors,
  semantic surfaces, route ownership, dialog policy, and profile validation.
license: MIT-0
metadata:
  dcc-mcp:
    dcc: computer-use
    layer: infrastructure
    compatibility: dcc-cua 0.1+
    version: "0.2.0"
    search-hint: "dcc-cua semantic profile JSON official custom selectors surfaces targets routes dialog policy"
    tags: "computer-use, infrastructure, read-only"
---

# dcc-cua semantic profiles

Profiles are application vocabulary and route hints. They do not replace the
CUA Host's exact-window identity, fresh-observation fence, authorization, input
owner, or independent application state authority.

Treat a profile as a small routing graph:

`selector -> surface -> target -> route`, with an optional
`target -> fallback profile/surface` edge. None of these declarations launches,
clicks, changes scope, or proves success by itself.

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

## Route ownership

| Route | Agent action |
| --- | --- |
| `accessibility` | Bind one exact window, take a fresh semantic observation, require exactly one match, then the profile CLI may execute an allowed action. |
| `unreal_typed_api` | Call the owning Unreal adapter/Skill and verify authoritative engine state. |
| `browser_dom` | Use the exact-bound dcc-cua browser route; do not switch to the in-app Browser skill. |
| `os_native_dialog` | Rebind to the exact platform dialog and use its native control path. |
| `visual_fallback` | Take a fresh exact-window visual snapshot and act only inside that scope. Use desktop scope only when explicitly granted. |

## Contract

Each profile declares:

- `schema_version: 1`, a stable `id`, and `display_name`;
- selectors containing real process names, localized title fragments, or URL
  hosts. Selector objects are OR alternatives. Populated application and title
  fields inside one selector are AND constraints. A URL-only selector does not
  match native windows;
- named surfaces with a route and targets;
- target aliases (`names`/`automation_ids`) and an explicit action allow-list;
- an optional target `fallback` containing only a non-empty `profile_id` and
  `surface_id`. It is a route edge, not automatic execution;
- `dialog_style` and `destructive_confirmation_required`.

Do not put credentials, private secrets, direct input injection, arbitrary shell
commands, or application API code in profile JSON. Keep destructive actions
confirmation-aware.

Target resolution prefers the stable target `id`. Labels are substring aliases;
`names` and `automation_ids` are exact, case-insensitive aliases. A live element
must always match the target `role`; when aliases exist, it must also match one
of them. A target without aliases is role-only and may produce multiple matches,
so the CLI refuses to act unless exactly one fresh element remains.

Minimal valid profile:

```json
{
  "schema_version": 1,
  "id": "studio-tool",
  "display_name": "Studio Tool",
  "selectors": [
    {
      "application_names": ["studio.exe"],
      "window_title_contains": ["Studio Tool"]
    }
  ],
  "surfaces": [
    {
      "id": "home",
      "label": "Home",
      "role": "panel",
      "route": "accessibility",
      "targets": [
        {
          "id": "run",
          "label": "Run task",
          "role": "button",
          "names": ["Run", "运行"],
          "automation_ids": ["RunButton"],
          "supported_actions": ["click"]
        }
      ]
    }
  ],
  "settings": {
    "dialog_style": "host_owned",
    "preferred_route": "accessibility",
    "destructive_confirmation_required": false
  }
}
```

## Agent decision loop

1. Inspect `dcc-cua profiles`, then print the chosen profile before opening a
   session.
2. Discover the real PID/window and bind its exact identity. Confirm its native
   selector or URL selector matches; never choose a profile from the task name
   alone.
3. Select one surface for the current task and resolve a target by stable ID.
   Treat labels and localized names as discovery aliases.
4. Reject actions not present in `supported_actions`. Dispatch through the
   surface's owning route; do not force `profile --action` onto a non-accessibility
   surface.
5. When the target declares `fallback`, load the referenced profile/surface,
   re-discover the new application/window, and take a new observation. Never
   carry PID, window ID, element index, token, or coordinates across that edge.
6. Verify application state after every mutation. A successful input result is
   only delivery evidence.

## Inspect and validate

```powershell
dcc-cua profiles
dcc-cua profile --id maya
dcc-cua profile --profile-file C:\profiles\studio.json
dcc-cua list --app maya.exe --on-screen
dcc-cua profile --profile-file C:\profiles\studio.json --pid $pid --window-id $hwnd --surface home --query new_scene
```

The first two commands inspect official profiles; the third validates and prints
a user profile without starting an application. The live form binds the profile
to one target, takes a fresh semantic observation, and reports current matches.
Execute an action only when the profile route supports it and exactly one live
element has a current locator.
