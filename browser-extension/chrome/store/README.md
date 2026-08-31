# Store submission checklist

Package validation is not submission readiness. No store listing, review or
publication state is established by this directory. Confirm the live state in
each owned developer dashboard before changing anything; do not create a
duplicate item because a CI probe was not run.

## Prepared assets

| Item | Location | State |
| --- | --- | --- |
| Extension and toolbar icons | `../public/icons/` | 16, 32, 48 and 128px PNGs; packaged and checked |
| Store logo | `assets/logo-300.png` | 300px PNG; owner artwork review pending |
| Small promotional tile | `assets/promo-440x280.png` | 440 x 280px PNG; owner artwork review pending |
| Listing text and permission rationale | [listing.md](listing.md) | Draft for owner review |
| Privacy description | [privacy.md](privacy.md) | Draft; verify the complete deployed data path before approval |
| Real product screenshots | Not supplied | Capture only after trusted integration and real runtime acceptance |

Chrome requires a packaged 128px PNG, a small promotional tile and at least one
real screenshot (1280 x 800 or 640 x 400). The prepared tile is promotional
artwork, **not** a substitute for that screenshot.
[Chrome image requirements](https://developer.chrome.com/docs/webstore/images).

Edge accepts the prepared 300px logo and 440 x 280px tile. A logo is required;
the promotional tile and screenshots are optional. Its listing also needs a
full description and accurate privacy disclosures.
[Edge publishing requirements](https://learn.microsoft.com/en-us/microsoft-edge/extensions/publish/publish-extension).

Firefox needs the built package and the matching readable source archive. Keep
the fixed Gecko ID and the existing Firefox 140+ data-consent declarations.
Review listing details, data practices and reviewer instructions before submit.
[AMO submission guide](https://extensionworkshop.com/documentation/publish/submitting-an-add-on/),
[Firefox data consent](https://extensionworkshop.com/documentation/develop/firefox-builtin-data-consent/).

The native runtime targets desktop browsers. Select only validated desktop
platforms in AMO; do not claim Android support. `web-ext` 10.6.0 reports no
errors and one Android minimum-version warning for the current manifest:
Android's built-in consent starts at 142, whereas desktop supports it from
140. Do not lower consent requirements, increase the desktop minimum merely
to hide the warning, or infer Android runtime support from a manifest value.
Review this platform distinction during AMO validation.

## Release gates

1. Install a released runtime and plugin that expose the automation-first
   `start_task` MCP surface. The connected Agent Host or IDE permission layer is
   the user-authorization boundary; a paired extension tab does not bypass its
   sandbox or permissions. Do not claim that every Agent Host works without a
   real exact-target runtime check.
2. Run `npm ci`, `npm run check`, `npm test`, `npm run build`, and
   `python -B scripts/test_extension.py` in the extension directory using Node
   22 or later. Existing CI and release jobs run the icon checks; no store
   credential or account action is needed for these checks.
3. Merge through review and let the independent extension release process
   assign a new version. Never replace an already-published version's ZIP.
   Verify release source SHA, artifact ID, hashes and expiry using the existing
   read-only readiness workflow. Its result does not prove UI acceptance.
4. Verify the owned listing ID, current draft/version and distribution channel
   in each store through project-owned DCC-CUA, with fresh runtime/PID/window/
   tab/origin binding. Read back each approved edit or upload and retain its
   receipt. Do not switch providers if authorization is unavailable.
5. Have the owner approve artwork, actual screenshots and privacy text against
   the deployed runtime/client. Publish an approved, accessible policy URL;
   do not point a store at this unapproved draft or certify declarations by
   inference from source alone.
6. Stop at the actual account verification, CAPTCHA/2FA, payment, agreement or
   final irreversible publish confirmation presented by the platform. Obtain
   the required human decision there. Only a fresh store receipt/readback can
   establish submission or publication.

The CI publishing path additionally requires the protected `browser-stores`
environment, exact listing IDs, credentials and explicit ready gate described
in [the extension README](../README.md). Do not toggle that gate merely because
builds or artifact hashes pass.
