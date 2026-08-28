# dcc-cua agent plugin

## Current authorization status

The packaged MCP server is **diagnostic only** until a trusted human
confirmation transport is integrated. It advertises
`authorization_integration_status`, which returns `integration_required`, the
runtime version, and the next integration owner. It does not advertise an
authorization card or issuer tools and cannot open a task or accept a signed
receipt. Installation, a chat message saying “authorize,” and a successful MCP
handshake do not constitute task authorization.

The previous bridge created an issuer after checking the MCP parent's package
or signed product identity. That check does not prove invocation from a human
surface. It also rejected desktop clients whose MCP launcher is an unpackaged
signed helper without executable version resources. Parent identity is now
diagnostic only; either success or failure leaves authorization unavailable.
No ancestor traversal, publisher-only allowlist, CLI flag, environment variable,
stdin acknowledgement, or generic automation fallback enables it.

This intentionally disables the previous process-identity-only inline card
path, including on formerly recognized clients. The card and task-engine tests
remain implementation fixtures, not a production user-input boundary.

[ADR 0027](adr/0027-cross-client-task-authorization.md) proposes the common
challenge/receipt protocol and separates repository work from host work for
Codex desktop/cloud/CLI, Cursor, WorkBuddy, and CodeBuddy CLI. It is a design
contract, **not an implemented signature verifier or certification of any
client's confirmation surface**.

Existing trusted **in-process** embeddings can still hold the move-only
`TrustedTaskAuthorizationIssuer` on their authenticated user-input side and
install only its validator in `HostSecurityServices`. This library boundary is
unchanged. It does not turn a model-driven MCP process into a trusted embedding.

## Plugin discovery

This checkout is also a standalone Codex-compatible agent plugin. The manifest
is [.codex-plugin/plugin.json](../.codex-plugin/plugin.json), and it exposes the
repository `skills/` directory plus the local `dcc-cua mcp-server` bridge.
The historical inline card implementation is retained for task-engine tests;
it is not exposed by the packaged server without a trusted input transport.

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
`.mcp.json`. Claude, CodeBuddy, and WorkBuddy should import the bridge through
their native plugin or MCP configuration and launch `dcc-cua mcp-server`
directly. A shell changes the diagnostic parent identity but cannot provide
human authorization either way.

Reconnect the MCP server after installing or upgrading it. Check
`authorization_integration_status` before proposing browser work. A missing
status tool is a plugin/startup/discovery problem; `integration_required` means
discovery works but no trusted human confirmation transport exists. A new task
or reinstall alone cannot supply that transport.

`browser_prepare` and `browser_snapshot` are session-bound Host operations.
`missing field task_grant_id` means the caller did not supply an existing
authorized session's complete request, not that it should invent a grant.
Use one persistent, exact-target session through the trusted embedding after
integration. Never weaken PID/native-window/tab/origin or fresh-evidence checks.

Real browser or store validation remains blocked until a human confirmation
surface, verifier, exact-target binding, and revocation channel have all been
implemented and tested together. Report that boundary instead of promising a
card that the user cannot see.

## Other agent hosts

Hosts that support the `.codex-plugin/plugin.json` layout can load the checkout
directly. Hosts that only support an `SKILL.md` directory can load the individual
directories under `skills/`; `skills/cua-cli` is the base contract and
`skills/game-cua-acceptance` adds the packaged-game workflow.

The game workflow deliberately keeps the Host session alive so the user-facing
ControlBanner remains visible. It uses bounded `keypress` actions with
`duration_ms` for held WASD movement and requires fresh post-action evidence.
