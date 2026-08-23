# dcc-cua agent plugin

This checkout is also a standalone Codex-compatible agent plugin. The manifest
is [.codex-plugin/plugin.json](../.codex-plugin/plugin.json), and it exposes the
repository `skills/` directory plus the local `dcc-cua mcp-server` bridge.
The bridge renders an inline task-start authorization card and keeps the
move-only issuer in the same process as the Host validator.

The repository marketplace installs the bounded
`plugins/dcc-cua-computer-use` package. It contains only the MCP bridge
manifest and configuration; Rust sources, build output, and the repository
`target/` directory are never copied into the Codex plugin cache. The root
manifest remains available for development checkouts that also need the
repository Skills.

## Codex

Install the checkout as a local plugin using the Codex plugin installation flow,
or point a development Codex session at the repository checkout. Install the
matching `dcc-cua` binary on `PATH`, then start a new Codex task so the MCP
server is discovered. The skill files can also be copied into an agent's skills
directory when local plugin installation is unavailable.

```powershell
codex plugin marketplace add .
codex plugin add dcc-cua-computer-use@dcc-cua
```

The authorization issuer is created only when Windows verifies that the MCP
server's immediate parent is the signed, packaged Codex desktop runtime. A
shell, redirected stdin client, unpackaged host, or unsupported platform fails
closed before the MCP server reads a request. Additional host platforms need
their own native embedding attestor; model-visible arguments and environment
variables are never accepted as an authorization bypass.

Before a mutating workflow, the model renders one exact PID/HWND proposal. The
card shows the closed Host-method list plus sensitive action, risk, and browser
origin scopes. The user types `授权` or `AUTHORIZE` in the inline card. The
app-only tool registers the server-held proposal; it accepts no scopes,
credentials, or secret values.
Authorized actions execute through the same process-local broker without native
action popups. Expiry, revocation, target changes, or scope mismatches fail
closed and never fall back to a modal prompt.

## Other agent hosts

Hosts that support the `.codex-plugin/plugin.json` layout can load the checkout
directly. Hosts that only support an `SKILL.md` directory can load the individual
directories under `skills/`; `skills/cua-cli` is the base contract and
`skills/game-cua-acceptance` adds the packaged-game workflow.

The game workflow deliberately keeps the Host session alive so the user-facing
ControlBanner remains visible. It uses bounded `keypress` actions with
`duration_ms` for held WASD movement and requires fresh post-action evidence.
