# Steam embedded Chromium profile

This package is declarative routing data for a Steam store page rendered by
Steam's Chromium surface. It addresses the case where exact-window capture is
available but native UI Automation exposes no useful controls.

The Host must bind one exact process ID and native window handle and must match
the observed window/runtime version before using the profile. Capability probes
try the bounded `browser_dom` bridge first and may inspect the accessibility
bridge when advertised. A missing or ambiguous bridge is a hard failure.

The `install` flow is intentionally constrained to:

1. locate the exact store URL/tab in a fresh snapshot;
2. resolve the unique enabled `install_button` semantic element;
3. click that explicit DOM/accessibility element; and
4. take a post-action snapshot and verify `installed_state`.

Coordinates, keyboard shortcuts, credentials, executable commands, and Steam
security-confirmation bypasses are not represented or permitted by this profile.

Validate offline with:

```text
cargo run -p dcc-cua-cli -- profile validate examples/profiles/steam-chromium
cargo test -p dcc-cua-semantic-profiles
```
