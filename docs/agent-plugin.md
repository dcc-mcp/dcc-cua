# dcc-cua agent plugin

## Automation model

The packaged `dcc-cua mcp-server` treats the connected Agent Host as the user
authorization boundary. Codex, DSH, Claude, Cursor, WorkBuddy, CodeBuddy, and
other hosts apply their own sandbox, tool approval, and permission policy before
calling DCC-CUA. The runtime does not repeat that decision with an authorization
card, Windows Hello/PIN, a physical-key sequence, or another prompt.

The MCP surface is intentionally small:

- `start_task` declares one exact PID/HWND or a closed DCC-CUA-owned browser
  launch plus bounded Host methods, final action scopes, browser origins, and
  expiry. It creates the internal lease, opens the session, and returns
  provider, runtime version, PID, and HWND in one call.
- `dcc_cua_task_call` performs one method inside that retained task scope.
- `task_status` reads lifecycle state.
- `stop_task` closes the session and revokes its internal lease.

There are no MCP UI resources or separate prepare, authorize, and start tools.
The internal constructor-owned broker remains process-local so an MCP caller
cannot supply or widen grant IDs, authorization IDs, capabilities, receipts,
targets, methods, action categories, origins, or expiry after `start_task`.
Every call still revalidates the retained scope, target identity, lease expiry,
stop state, fresh-observation fence, and post-action state.

[ADR 0028](adr/0028-delegate-task-authorization-to-agent-hosts.md) records this
boundary. The lower-level signed cross-client receipt API described by
[ADR 0027](adr/0027-cross-client-task-authorization.md) remains available to
custom in-process embeddings, but it is not part of the packaged MCP flow.

## Plugin discovery

This checkout is a standalone Codex-compatible agent plugin. The root manifest
is [.codex-plugin/plugin.json](../.codex-plugin/plugin.json); it exposes the
repository `skills/` directory and local `dcc-cua mcp-server` bridge. The
marketplace package under `plugins/dcc-cua-computer-use` contains only the MCP
manifest and configuration, never Rust sources, build output, or `target/`.

## Installation

Install the matching released `dcc-cua` binary on `PATH`, install or point the
Agent Host at this plugin, then start a new task so the MCP server is
rediscovered:

```powershell
codex plugin marketplace add .
codex plugin add dcc-cua-computer-use@dcc-cua
```

The package contains one executable, `dcc-cua`, for interactive CLI and MCP
commands. The MCP command is `dcc-cua mcp-server`; no wrapper child or separate
background executable is shipped. Protocol, task authorization, EOF cleanup,
diagnostics and exit handling remain in the same shared runtime.

Windows MSVC builds retain the Console PE subsystem and embed
`consoleAllocationPolicy=detached`. Windows 11 24H2 (build 26100) and Windows
Server 2025 or later honor this loader policy before application code starts,
while the Console subsystem retains shell waiting semantics. Older Windows
ignores this policy; those clients must supply `CREATE_NO_WINDOW` when creating
the process. Non-MSVC Windows builds also require that launcher flag. Portable
MCP command/args configuration does not itself set process creation flags.
Support for that flag in every Codex/Agent Host launcher has not been established;
do not advertise universal console-free launch on older systems. See
[Microsoft console policy](https://learn.microsoft.com/en-us/windows/console/console-allocation-policy).

Preserve inherited stdin/stdout/stderr pipes. MCP stdout is JSON-RPC only;
diagnostics remain on stderr. Do not call FreeConsole or attach a console after
startup to conceal a console allocation. A changed plugin entry takes effect
only after normal client rediscovery; it does not close existing connections.

The updater validates and replaces only the canonical `dcc-cua` executable,
retaining its installation lock and recovery bytes on incomplete rollback.
Historical sibling binaries are unowned and are never automatically deleted.
Close affected resident clients before updating on Windows, then reconnect.

The checkout also includes `.claude-plugin/marketplace.json` and a portable
`.mcp.json`. Hosts without Codex plugin support may launch `dcc-cua mcp-server`
directly through their native MCP configuration.

After installation, confirm that `tools/list` exposes exactly `start_task`,
`task_status`, `stop_task`, and `dcc_cua_task_call`. Instrumented builds also
advertise the read-only `dcc-cua://connection/current` resource, which reports
only the caller's current connection. It does not grant task authority or list
other clients. Use `dcc-cua connections` for the local bounded diagnostic
registry; older resident bridges have no records. See
[MCP connection diagnostics](design/mcp-connection-diagnostics.md) for the
schema and attribution limits. A stale authorization tool or card means an
older runtime/plugin is still loaded; reconnect after upgrading.

Before the first observation or input, report the `provider=dcc-cua`, runtime
version, exact PID, and exact HWND returned by `start_task`. Use one persistent
task for the exact target, take fresh observations, verify every mutation, and
call `stop_task` on success, failure, interruption, or abandonment.

For an exact PID/HWND target that is minimized or maximized, declare `change_window_state`
in `start_task.allowed_methods` and call it through the same task:

```json
{
  "task_id": "<task_id returned by start_task>",
  "method": "change_window_state",
  "params": {"operation": "restore_activate"}
}
```

Take a fresh observation after restoring. This operation restores only the
bound window, clears both minimized and maximized state, and verifies its
native instance and foreground state. A minimized window that returns to its
previous maximized state is normalized in a second bounded restore stage.
`get_window_state` reports the native `maximized` fact. Restore a maximized
target before `set_window_frame`, then read fresh native window state; an outer
frame match alone cannot confirm that a maximized client resized. The MCP bridge
returns `frame_recovery` when a pixels-only state read cannot mint a frame token
because the target is minimized or maximized. Its `next_call` keeps the current
task and requests `restore_activate` only when that task already grants
`change_window_state` without a dismiss-menu-only scope. Otherwise it reports
the required method and that a new task is needed; it never restores implicitly.
After a confirmed restore, read fresh window state before moving the frame.
The MCP bridge
also accepts `activate`, which does not restore a minimized window; it rejects
`close` and other window state operations. An unavailable interactive desktop
still blocks activation and input. Repair that environment before retrying.

## Explicit pixels-only observation

`observation_mode` defaults to `semantic`. To observe an exact native window
without starting UIA, explicitly select `pixels_only` on the `window` surface:

```json
{
  "application_label": "Exact native window",
  "target_process_id": 1234,
  "target_window_handle": 5678,
  "surface": "window",
  "observation_mode": "pixels_only",
  "allowed_methods": ["snapshot", "get_window_state", "minimize_window"],
  "allowed_actions": [{
    "action": "minimize_window",
    "input_kind": "window_state",
    "secret_input": false,
    "authorization_category": "window_state"
  }]
}
```

After reporting the returned provider/runtime/PID/HWND, call `snapshot` with
`params: {}`. Exact-window capture proof, geometry, DPI, visibility, occlusion,
and native instance checks still apply. A minimized or unproven capture fails;
window metadata cannot substitute for a screenshot. A successful pixel snapshot
returns a formal `observation_id`, `accessibility_available=false`, and no
`accessibility_state_id` or semantic element authorization.

On Windows, independently verified window-content captures remain readable in
the background, including when another window covers the target. A desktop-crop
fallback may publish a partially visible target: every proven overlapping root
rectangle is filled with opaque black, and provenance reports
`capture_mode: "redacted_desktop_crop"` plus `visibility.redacted_rectangles`
in physical desktop coordinates. Unknown occlusion geometry, changing masks,
and a fully covered desktop crop still refuse publication. These masks exclude
foreign-window pixels; they do not reconstruct the covered target content.

Pixel tasks permit observation, native state reads, the explicit minimize
operation, and these explicitly scoped foreground raw actions: `click`,
`double_click`, `right_click`, `toggle`, `keypress`, `keyboard_shortcut`, `type`,
and `type_chars`. To use them, include `execute_action` in `allowed_methods` and
grant each required action with `input_kind: "raw_input"`, `secret_input: false`,
and `authorization_category: "raw_input"`. The Host derives raw-input permission
from those trusted scopes; granting the method alone cannot enable input.

Each action must carry the latest pixel `observation_id`. Omit
`accessibility_state_id`, semantic element indices/tokens, secret handles, and
`input_backend_id`. Only omitted or `foreground` delivery is accepted. The
captured process/window instance, native bounds, DPI, foreground window, and
occlusion proof are checked again at every physical input boundary. A failed
check refuses dispatch or reports a partial dispatch; it does not activate the
window or fall back to another backend. Any attempted action consumes the old
observation. With `capture_after: true`, the post-action observation uses the
same pixel route and cannot mint semantic element authorization. A failed post
snapshot requires a fresh observation before another action.

Pointer actions additionally require an unredacted point in that observation
and a fresh native hit test on the exact target or its child. Other parts of the
window may be covered. Keyboard and text actions retain the complete visibility
requirement. A readable background capture alone never authorizes foreground
input or click-through to another window.

Semantic selectors, browser/clipboard input, secret input, drag, wheel, move,
and recording are outside this pixel contract. It retains the existing
separately authorized `change_window_state` activate/restore route; pixel startup,
snapshots, and raw input never implicitly activate a window.

To minimize, call `dcc_cua_task_call` with `method: "minimize_window"` and
`params: {"observation_id": "<latest snapshot observation_id>"}`. The retained
task must grant both the method and the exact `window_state/minimize_window`
action scope. Host and Core check the latest observation and the actual captured
native window instance before dispatch. The result reports native minimized
state; an attempted mutation consumes the old observation even when its outcome
is uncertain. Do not retry with that observation ID. `stop_task` releases this
task without closing or terminating the application.

Account verification, CAPTCHA/2FA, agreements, payments, and final irreversible
publication remain separate human boundaries. Removing the duplicated DCC-CUA
authorization card does not automate those external account/security decisions.

## Other agent hosts

Hosts that support `.codex-plugin/plugin.json` can load the checkout directly.
Hosts that only support an `SKILL.md` directory can load the individual
directories under `skills/`; `skills/cua-cli` is the base contract and
`skills/game-cua-acceptance` adds the packaged-game workflow.

The game workflow keeps the Host session alive so the user-facing ControlBanner
remains visible. It uses bounded `keypress` actions with `duration_ms` for held
WASD movement and requires fresh post-action evidence.

For a byte-pipeline diagnostic on an explicitly bound `pixels_only` task, call
`dcc_cua_task_call` with `method: "snapshot"` and
`params: {"capture_diagnostics": true}`. The flag defaults to false; semantic
snapshots reject the opt-in. The final observation's
`capture_provenance.capture_diagnostics` contains SHA-256 digests of native BGRA
and canonical PNG RGBA bytes, four bounded 256-bin channel histograms, timings,
and separate provenance for the existing first and final captures. The fourth
native byte is reported as a raw high byte, without an alpha interpretation.
Both raw captures and their publication fences remain required. Only the final
frame is PNG-encoded; the discarded validation frame reports `png_encoded=false`
and zero PNG encoding time while retaining its optional byte summaries.
This adds no capture, normalization, retry, activation, or input permission.

For repeatable Windows measurements, run the ignored
`owned_window_snapshot_latency` test in `dcc-cua-e2e` with `--features gui-e2e`.
Set `DCC_CUA_LATENCY_DIRECTORY` to an existing empty private directory. The test
publishes `binding.json` and waits up to three minutes: report its exact
provider/runtime/PID/HWND before creating the `begin` file in that directory.
It measures default and diagnostic snapshots on disposable native windows,
decodes their PNGs, verifies ownership and cleanup, and writes `results.json`.
Keep other builds stopped while sampling. These SDK timings exclude MCP
transport, perception, and input; encoding-only timings are a separate metric.
