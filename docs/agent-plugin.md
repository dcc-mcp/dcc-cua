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

The checkout also includes `.claude-plugin/marketplace.json` and a portable
`.mcp.json`. Hosts without Codex plugin support may launch `dcc-cua mcp-server`
directly through their native MCP configuration.

After installation, confirm that `tools/list` exposes exactly `start_task`,
`task_status`, `stop_task`, and `dcc_cua_task_call`, while `resources/list` is
empty. A stale authorization tool or card means an older runtime/plugin is
still loaded; reconnect or restart the Agent Host after upgrading.

Before the first observation or input, report the `provider=dcc-cua`, runtime
version, exact PID, and exact HWND returned by `start_task`. Use one persistent
task for the exact target, take fresh observations, verify every mutation, and
call `stop_task` on success, failure, interruption, or abandonment.

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
