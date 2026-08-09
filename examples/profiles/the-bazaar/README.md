# The Bazaar profile package

This example separates three authorities:

- `profile.json` supplies deterministic application vocabulary and a bounded loopback state-source contract.
- `SKILL.md` supplies decision policy, including evidence-gated pivots and transaction postconditions.
- `companion/` is an optional, independently built read-only process that tails `Player.log` and indexes `GameData.db`. The profile never launches it and it exposes no action endpoint.
- `knowledge/card-identities.seed.json` supplies reviewed external aliases that users may copy into their own local knowledge cache.

The ten-win corpus is a population prior, not a route to force. Runtime decisions prioritize the current verified board, active and broken relationships, recent combat records, and only then historical builds. Economy and progress values that cannot be derived from the log must be supplied as volatile observations with provenance; do not publish one player's live values in a reusable package.

## Validate and install

```powershell
cargo run -p dcc-cua-cli -- profile validate examples/profiles/the-bazaar
cargo run -p dcc-cua-cli -- profile install examples/profiles/the-bazaar
```

Installation copies the declared package contents atomically to `~/.dcc-cua/profiles/the-bazaar`.

## Run the optional companion

Copy `companion/companion.example.json` to the user-owned configuration path
`~/.dcc-cua/config/profiles/the-bazaar/companion.json`, replace the two game
paths, and keep progress observations local. Never place mutable configuration
inside the installed package: `profile install --replace` atomically replaces
that directory. Then:

```powershell
cargo run --manifest-path examples/profiles/the-bazaar/companion/Cargo.toml --release -- --config $env:USERPROFILE\.dcc-cua\config\profiles\the-bazaar\companion.json
```

Read state through the same CLI contract used by MCP or another agent platform:

```powershell
dcc-cua profile-state --profile-file ~/.dcc-cua/profiles/the-bazaar/profile.json
```

## Local card identity cache

The authoritative identity chain is `Player.log instance ID -> local GameData.db template UUID`. BazaarDB is an optional external reference on that exact local UUID, never the runtime authority for card text or patch behavior. Copy the reviewed seed to a user-owned cache and point `cardIdentityCachePath` at its absolute path:

```powershell
New-Item -ItemType Directory -Force $env:USERPROFILE\.dcc-cua\knowledge\the-bazaar
Copy-Item knowledge\card-identities.seed.json $env:USERPROFILE\.dcc-cua\knowledge\the-bazaar\card-identities.json
```

The companion loads and validates this file once, exposes cache hits as `externalReferences`, and never writes it. A record is accepted only when its local template exists, its canonical name exactly matches the current local database, its BazaarDB ID and URL are canonical, and it carries patch/date/match provenance. Add a mapping only after a current card is resolved locally and the external page is verified. Query the website only on a cache miss or when its recorded patch needs refreshing; do not make gameplay depend on an undocumented remote API. A stale external patch is useful reference metadata, not evidence that overrides local card data.

All input still goes through the DCC CUA Host with TaskGrant, fresh observation, PID, and HWND fences. The companion cannot click, unlock Windows, supply credentials, or bypass the visible control boundary.
