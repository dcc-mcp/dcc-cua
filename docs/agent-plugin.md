# dcc-cua agent plugin

This checkout is also a standalone Codex-compatible agent plugin. The manifest
is [.codex-plugin/plugin.json](../.codex-plugin/plugin.json), and it exposes the
repository `skills/` directory, including `cua-cli`, `cua-profile-authoring`,
and `game-cua-acceptance`.

## Codex

Install the checkout as a local plugin using the Codex plugin installation flow,
or point a development Codex session at the repository checkout. The skill
files can also be copied into an agent's skills directory when local plugin
installation is unavailable.

## Other agent hosts

Hosts that support the `.codex-plugin/plugin.json` layout can load the checkout
directly. Hosts that only support an `SKILL.md` directory can load the individual
directories under `skills/`; `skills/cua-cli` is the base contract and
`skills/game-cua-acceptance` adds the packaged-game workflow.

The game workflow deliberately keeps the Host session alive so the user-facing
ControlBanner remains visible. It uses bounded `keypress` actions with
`duration_ms` for held WASD movement and requires fresh post-action evidence.
