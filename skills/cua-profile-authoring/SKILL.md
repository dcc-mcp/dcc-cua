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
    compatibility: dcc-cua 0.2+
    version: "0.3.0"
    search-hint: "dcc-cua semantic profile JSON multilingual BCP-47 localized aliases selectors surfaces targets routes dialog policy"
    tags: "computer-use, infrastructure, localization, read-only"
---

# dcc-cua semantic profiles

Profiles are application vocabulary and route hints. They do not replace the
CUA Host's exact-window identity, fresh-observation fence, authorization, input
owner, or independent application state authority.

Treat a profile as a small routing graph:

`application/version -> selector -> surface -> target -> route`, with optional
`child -> parent profile version`,
`target -> fallback profile/surface` and `target/action -> key binding` edges.
None of these declarations launches, changes scope, or proves success by itself.

## Official and user profiles

- Official defaults live in the Rust crate under `profiles/` and are covered by
  Rust tests. Add one only when the vocabulary is stable and reusable across
  tasks.
- Users and studios can supply a compatible JSON file with `--profile-file`
  without changing the binary or editing application preferences.
- Keep host-specific execution in the owning adapter/route. The CLI executes
  live element actions on `accessibility` surfaces and verified `key_bindings`
  after a fresh exact-window visual observation. Other actions remain
  declarations for the owning control path.

## Route ownership

| Route | Agent action |
| --- | --- |
| `accessibility` | Bind one exact window, take a fresh semantic observation, require exactly one match, then the profile CLI may execute an allowed action. |
| `unreal_typed_api` | Call the owning Unreal adapter/Skill and verify authoritative engine state. |
| `browser_dom` | Use the exact-bound dcc-cua browser route; do not switch to the in-app Browser skill. |
| `os_native_dialog` | Rebind to the exact platform dialog and use its native control path. |
| `visual_fallback` | Take a fresh exact-window visual snapshot and act only inside that scope. A declared key binding may execute after this fence. Use desktop scope only when explicitly granted. |

## Contract

Each profile declares:

- `schema_version: 3`, a stable `id`, SemVer `profile_version`, and
  `display_name`;
- `application.family` plus optional exact `application.versions` tokens;
- optional single-parent `extends` with an ID and SemVer requirement. The
  Runtime flattens inheritance before Agent use. Selectors replace inherited
  selectors when present; state sources, surfaces, and targets merge by stable
  ID with child values winning;
- selectors containing real process names, localized title fragments, or URL
  hosts. Selector objects are OR alternatives. Populated application and title
  constraints inside one selector are ANDed, while generic and localized title
  aliases are one combined OR list. A URL-only selector does not match native
  windows;
- named surfaces with a route and targets;
- target aliases (`names`, BCP-47 keyed `localized_names`, and
  `automation_ids`) and an explicit action allow-list;
- optional `key_bindings` from a supported semantic action to 1-4 verified key
  names; these never remove the fresh-observation or exact-window fence;
- an optional target `fallback` containing only a non-empty `profile_id` and
  `surface_id`. It is a route edge, not automatic execution;
- `preferred_route`, `dialog_style`, optional `default_locale`, and
  `destructive_confirmation_required`.

Do not put credentials, private secrets, direct input injection, arbitrary shell
commands, or application API code in profile JSON. Keep destructive actions
confirmation-aware.

## Multilingual aliases

- Keep profile, surface, target, role, action, and automation IDs stable and
  language-neutral. Localize only visible window-title fragments and element
  names.
- Put generic or legacy aliases in `window_title_contains` and `names`. Put
  locale-specific aliases in `localized_window_title_contains` or
  `localized_names`, keyed by a common BCP-47 tag such as `zh-CN`, `zh-TW`,
  `ja-JP`, or `pt-BR`. Set `settings.default_locale` when the untagged aliases
  belong to one known language.
- All aliases are active simultaneously. Locale tags describe coverage; they do
  not gate matching, so an agent can discover a mixed-language UI without first
  knowing its locale.
- Matching trims and collapses whitespace and uses Unicode lowercase conversion.
  Add explicit aliases for punctuation, abbreviations, or wording differences;
  do not rely on machine translation or fuzzy matching.
- Prefer stable automation IDs when the application exposes them. Require real
  UI evidence before promoting studio-specific wording into an official profile.
- `dcc-cua profiles` reports `supported_locales`. A user-authored profile that
  omits localized fields remains valid.

Target resolution prefers the stable target `id`. Labels are substring aliases;
`names`, `localized_names`, and `automation_ids` are exact aliases after text
normalization. A live element must always match the target `role`; when aliases
exist, it must also match one of them. A target without aliases is role-only and
may produce multiple matches, so the CLI refuses to act unless exactly one fresh
element remains.

Minimal valid profile:

```json
{
  "schema_version": 3,
  "id": "studio-tool",
  "profile_version": "1.0.0",
  "application": {"family": "studio-tool", "versions": ["2024"]},
  "display_name": "Studio Tool",
  "selectors": [
    {
      "application_names": ["studio.exe"],
      "window_title_contains": ["Studio Tool"],
      "localized_window_title_contains": {
        "zh-CN": ["工作室工具"],
        "ja-JP": ["スタジオツール"]
      }
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
          "names": ["Run"],
          "localized_names": {
            "zh-CN": ["运行"],
            "ja-JP": ["実行"]
          },
          "automation_ids": ["RunButton"],
          "supported_actions": ["click"]
        }
      ]
    }
  ],
  "settings": {
    "dialog_style": "host_owned",
    "preferred_route": "accessibility",
    "default_locale": "en",
    "destructive_confirmation_required": false
  }
}
```

## Agent decision loop

1. Inspect `dcc-cua profiles` and its `supported_locales`, then print the chosen
   profile before opening a session. If PID/window discovery already supplied
   an application name and title, run `dcc-cua profile match --app APP --title
   TITLE`; accept `selected` only when `ambiguous` is false.
2. Discover the real PID/window and bind its exact identity. Confirm its native
   selector or URL selector matches; never choose a profile from the task name
   alone.
3. Select one surface for the current task and resolve a target by stable ID.
   Treat labels and localized names only as discovery aliases.
4. Reject actions not present in `supported_actions`. Dispatch through the
   surface's owning route. A non-accessibility action requires an explicit
   `key_bindings` entry; never infer one from aliases or labels.
5. When the target declares `fallback`, load the referenced profile/surface,
   re-discover the new application/window, and take a new observation. Never
   carry PID, window ID, element index, token, or coordinates across that edge.
6. Verify application state after every mutation. A successful input result is
   only delivery evidence.

## Inspect and validate

```powershell
dcc-cua profiles
dcc-cua profile --id maya
dcc-cua profile match --app maya.exe --title "Autodesk Maya 2024: scene.ma"
dcc-cua profile --profile-file C:\profiles\studio.json
dcc-cua list --app maya.exe --on-screen
dcc-cua profile --profile-file C:\profiles\studio.json --pid $pid --window-id $hwnd --surface home --query new_scene
```

The first two commands inspect official profiles; the third validates and prints
a user profile without starting an application. The live form binds the profile
to one target, takes a fresh semantic observation, and reports current matches.
Execute an action only when the profile route supports it and exactly one live
element has a current locator, or when the target declares a validated key
binding. Both paths take a fresh exact-window observation first.
