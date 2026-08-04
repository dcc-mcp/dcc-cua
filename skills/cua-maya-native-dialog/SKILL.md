---
name: cua-maya-native-dialog
description: >-
  Use the dcc-mcp-cua Maya profile to operate Maya's home screen and OS-native
  file dialogs through scoped UI actions. Use for Maya dialog and startup-flow
  validation; do not edit Maya preference files or use Maya MCP as a substitute.
license: MIT-0
metadata:
  dcc-mcp:
    dcc: computer-use
    layer: domain
    compatibility: dcc-mcp-cua 0.1+ on Windows, macOS, or Linux with Maya.
    version: "0.1.0"
    search-hint: "Maya OS native dialog home new open file dialog dcc-mcp-cua profile UIA"
    tags: "computer-use, maya, ui-control, read-only"
---

# Maya OS-native dialog workflow

Use the repository's `dcc-mcp-cua` binary and the built-in `maya` profile. The
profile's `dialog_style` is `os_native`; the setting is a routing contract, not
permission to patch Maya files.

## Workflow

1. Inspect the official profile and bind an exact Maya window:

   ```powershell
   dcc-mcp-cua profile --id maya
   dcc-mcp-cua list --app maya.exe --on-screen
   dcc-mcp-cua profile --id maya --app maya.exe --surface home --query new_scene --activate
   ```

2. Start at Maya Home when validating startup behavior. Use a fresh snapshot,
   click `New`/`新建` or `Open`/`打开`, then snapshot again.

3. For file dialogs, prefer the current UIA element index/token from the latest
   native-dialog snapshot. Use visual coordinates only when the native dialog
   has no usable semantic child nodes. Never reuse coordinates across dialogs or
   DPI/session changes.

4. Verify the resulting dialog title/window identity and final Maya state. A
   changed preference file is not acceptance evidence; the visible UI transition
   is.

5. Stop immediately on a user interruption, policy/authentication boundary, or
 unavailable desktop. Do not retry through another input path.
