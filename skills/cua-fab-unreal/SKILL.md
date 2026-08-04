---
name: cua-fab-unreal
description: >-
  Use the dcc-mcp-cua release CLI to acquire an explicitly approved Fab asset
  and import it into Unreal through visible, scoped UI actions. Use for repeatable
  Fab-to-Unreal validation; do not use for generic desktop automation or direct
  Unreal project-file edits.
license: MIT-0
metadata:
  dcc-mcp:
    dcc: computer-use
    layer: domain
    compatibility: dcc-mcp-cua 0.1+ on Windows; requires an installed Fab-capable browser or Epic Games Launcher and Unreal.
    version: "0.1.0"
    search-hint: "Fab Unreal asset download import CUA CLI marketplace cache FBX UE5 UE4 visible UI workflow"
    tags: "computer-use, unreal, fab, pipeline, destructive"
---

# Fab → Unreal with dcc-mcp-cua

Use the repository's `dcc-mcp-cua` binary for every observation and action. Do
not substitute a generic Computer Use client, Maya MCP, shell file edits, or
direct Unreal project-file changes.

## Workflow

1. Preflight the installed profiles and the real application windows:

   ```powershell
   dcc-mcp-cua profiles
   dcc-mcp-cua profile --id fab
   dcc-mcp-cua profile --id ue
   dcc-mcp-cua list --app EpicGamesLauncher.exe --on-screen
   dcc-mcp-cua list --app UE5Editor.exe --on-screen
   ```

   If only UE4 is installed, report that version boundary instead of claiming
   UE5 coverage.

2. Use the latest scoped snapshot before each action. Prefer semantic/UIA
   targets; use one bounded visual action only when the embedded Fab or Unreal
   surface exposes no child accessibility nodes. Never reuse coordinates from an
   older snapshot.

3. Treat account, purchase, download, export, and import actions as explicit
   mutations. Stop for a user confirmation at the destructive-confirmation
   boundary. Record the asset name, engine target, download completion state,
   and cache location in the task result.

4. Import through Unreal's visible Content Browser and native file dialog. Verify
   both the FBX import result and the post-import Content Browser asset; a cache
   file alone is not proof that Unreal loaded the asset.

5. Run the task as a bounded long operation with checkpoints:
   `profile/preflight → Fab ready → download complete → Unreal ready → imported`.
   On `desktop_unavailable`, a disconnected user session, policy denial, or user
   interruption, stop and recover the environment before retrying the same stage.

## Acceptance evidence

- a completed Fab/download state;
- the actual cache artifact or native file-dialog path;
- Unreal's import completion state;
- a final scoped screenshot or accessibility result showing the imported asset.
