# dcc-cua agent plugin

## Current authorization status

The packaged MCP server exposes the same client-managed authorization flow on
Windows, Linux, and macOS. `authorization_integration_status` reports
`confirmation_method=client_managed`: Codex, DSH, Claude, WorkBuddy, or another
connected Agent host owns the user/tool approval decision. DCC-CUA does not
open Windows Hello or an F-key confirmation prompt.

`prepare_task_authorization` returns the exact retained application, target,
PID/HWND or owned-browser launch spec, immutable scope digest, allowed methods,
final action/risk scopes, browser origins, and expiry. After its own approval,
the Agent host calls `authorize_task` with only that proposal ID. DCC-CUA then
issues a short-lived process-local receipt that is single-use for session open,
revalidated before every action, and revocable.

This delegates approval, not scope construction. Caller-supplied grant IDs,
capabilities, receipts, free-form approval text, target changes, and scope
widening are rejected. Parent process identity remains diagnostic only. CLI
arguments, environment variables, and redirected stdin do not authorize.

[ADR 0028](adr/0028-delegate-task-authorization-to-agent-hosts.md) defines the
portable Agent-host trust boundary. The core challenge/receipt and lease
enforcement from [ADR 0027](adr/0027-cross-client-task-authorization.md)
remains in force.

Existing trusted **in-process** embeddings can still hold the move-only
`TrustedTaskAuthorizationIssuer` on their authenticated user-input side and
install only its validator in `HostSecurityServices`. This library boundary is
unchanged. It does not turn a model-driven MCP process into a trusted embedding.

## Plugin discovery

This checkout is also a standalone Codex-compatible agent plugin. The manifest
is [.codex-plugin/plugin.json](../.codex-plugin/plugin.json), and it exposes the
repository `skills/` directory plus the local `dcc-cua mcp-server` bridge.
The optional card is exposed by every packaged server. The same tools are also
normal MCP tools so clients without MCP Apps can inspect, authorize, start, and
revoke tasks. They accept only server-generated proposal IDs; approval text,
signatures, grant IDs, and caller-selected embedding labels are rejected.

The repository marketplace installs the bounded
`plugins/dcc-cua-computer-use` package. It contains only the MCP bridge
manifest and configuration; Rust sources, build output, and the repository
`target/` directory are never copied into the Codex plugin cache. The root
manifest remains available for development checkouts that also need the
repository Skills.

## Installation

Install the checkout as a local plugin using the Codex plugin installation flow,
or point a development Codex session at the repository checkout. Install the
matching `dcc-cua` binary on `PATH`, then start a new Codex task so the MCP
server is discovered. The skill files can also be copied into an agent's skills
directory when local plugin installation is unavailable.

```powershell
codex plugin marketplace add .
codex plugin add dcc-cua-computer-use@dcc-cua
```

The checkout also includes `.claude-plugin/marketplace.json` and a portable
`.mcp.json`. DSH, Claude, CodeBuddy, and WorkBuddy should import the bridge
through their native plugin or MCP configuration and launch
`dcc-cua mcp-server` directly. Each host must apply its own approval policy
before calling `authorize_task`; a shell changes only the diagnostic parent
identity.

Reconnect the MCP server after installing or upgrading it. Check
`authorization_integration_status` before proposing browser work. A missing
status tool is a plugin/startup/discovery problem. A current runtime reports
schema `dcc-cua.authorization-integration.v2` and
`confirmation_method=client_managed` on every supported platform.

`browser_prepare` and `browser_snapshot` are session-bound Host operations.
`missing field task_grant_id` means the caller did not supply an existing
authorized session's complete request, not that it should invent a grant.
Use one persistent, exact-target session through the trusted embedding after
integration. Never weaken PID/native-window/tab/origin or fresh-evidence checks.

Browser or store validation still requires a fresh exact-target binding and
the installed Agent host's authorization. Account verification,
CAPTCHA/2FA, agreements, payments, and the final irreversible store-publication
step remain separate human boundaries.

## Other agent hosts

Hosts that support the `.codex-plugin/plugin.json` layout can load the checkout
directly. Hosts that only support an `SKILL.md` directory can load the individual
directories under `skills/`; `skills/cua-cli` is the base contract and
`skills/game-cua-acceptance` adds the packaged-game workflow.

The game workflow deliberately keeps the Host session alive so the user-facing
ControlBanner remains visible. It uses bounded `keypress` actions with
`duration_ms` for held WASD movement and requires fresh post-action evidence.
