# dcc-cua agent plugin

## Current authorization status

On Windows, the packaged MCP server exposes the bounded authorization card and
reports `available` from `authorization_integration_status`. Authorization is
created only after a protected native user-presence check: Windows Hello/PIN/
biometric through `UserConsentVerifier` when configured, otherwise the user
must physically press F12, F11, then F10 in the native DCC-CUA prompt. The
fallback rejects Windows low-level keyboard events marked injected or
lower-integrity injected. Clicking the prompt, typing card text, a chat message
saying “authorize,” client metadata, environment variables, and redirected
stdin never create authority.

The prompt displays the exact application, proposal, PID/HWND, immutable scope
digest, and expiry retained by the runtime. Only a successful protected check
can invoke the private process-local issuer. Scope is still single-use for
session open, short-lived, revalidated before every action, and revocable.
MCP Apps `app`-only tool visibility limits routing but is not itself treated as
human authorization.

On non-Windows systems, the packaged server remains diagnostic-only and reports
`integration_required` until an equivalent protected user-presence host is
implemented. The cross-client signed-receipt verifier exists in the core API,
but the packaged MCP runtime does not accept arbitrary model-relayed receipts.

The previous bridge created an issuer after checking the MCP parent's package
or signed product identity. That check does not prove invocation from a human
surface. It also rejected desktop clients whose MCP launcher is an unpackaged
signed helper without executable version resources. Parent identity is now
diagnostic only; either result leaves the protected verifier decision unchanged.
No ancestor traversal, publisher-only allowlist, CLI flag, environment variable,
stdin acknowledgement, or generic automation fallback enables it.

This keeps the previous process-identity-only path disabled. Parent identity is
diagnostic context only and never selects or bypasses the protected verifier.

[ADR 0027](adr/0027-cross-client-task-authorization.md) defines the common
challenge/receipt protocol and the packaged Windows verifier boundary. It does
not certify cloud or non-Windows confirmation surfaces, browser-store
publication, or any action outside the exact retained task scope.

Existing trusted **in-process** embeddings can still hold the move-only
`TrustedTaskAuthorizationIssuer` on their authenticated user-input side and
install only its validator in `HostSecurityServices`. This library boundary is
unchanged. It does not turn a model-driven MCP process into a trusted embedding.

## Plugin discovery

This checkout is also a standalone Codex-compatible agent plugin. The manifest
is [.codex-plugin/plugin.json](../.codex-plugin/plugin.json), and it exposes the
repository `skills/` directory plus the local `dcc-cua mcp-server` bridge.
The card is exposed by the packaged Windows server. Its private tools accept
only server-generated proposal IDs; approval text, signatures, grant IDs, and
caller-selected embedding labels are rejected.

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
discovery works on a platform without a protected confirmation host. On
Windows, `confirmation_method` reports the verifier selected by the runtime.

`browser_prepare` and `browser_snapshot` are session-bound Host operations.
`missing field task_grant_id` means the caller did not supply an existing
authorized session's complete request, not that it should invent a grant.
Use one persistent, exact-target session through the trusted embedding after
integration. Never weaken PID/native-window/tab/origin or fresh-evidence checks.

Browser or store validation still requires a fresh exact-target binding and a
real human confirmation in the installed client. Account verification,
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
