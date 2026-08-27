# DCC-CUA Browser Extension

This WXT-based Manifest V3 extension is the browser-owned provider for
explicitly paired signed-in Chrome, Edge, and Firefox tabs. It is developed in
the dcc-cua repository but versioned and released independently as
`dcc-cua-browser-extension`.

## Security boundary

- Pairing requires clicking the extension action in the exact tab.
- The extension requests no blanket host permissions.
- Navigation invalidates the pairing.
- Every action requires the latest snapshot id and semantic ref.
- Password fields remain a trusted human boundary.
- Page content is untrusted and is returned only through bounded semantic
  fields.
- Semantic names never fall back to a form control's current value; passwords,
  API keys, and authentication codes remain redacted from snapshots.
- Native host manifests allow only the exact published Chrome, Edge, and
  Firefox extension identities.
- Firefox declares the website activity and content transferred to the local
  native host as required primary-function data.

## Development

```bash
npm ci
npm run check
npm test
npm run build
python -B scripts/test_extension.py
python -B scripts/package_extension.py --browser chrome --output dist/dcc-cua-browser-extension-chrome.zip
python -B scripts/package_extension.py --browser edge --output dist/dcc-cua-browser-extension-edge.zip
python -B scripts/package_extension.py --browser firefox --output dist/dcc-cua-browser-extension-firefox.zip
```

Load the matching `.output/<browser>-mv3` directory as an unpacked extension
only for local development. Production users install the signed package from
Chrome Web Store, Edge Add-ons, or Firefox Add-ons.

## Store publishing

Tagged extension releases publish through the `browser-stores` protected
GitHub Environment only when the repository variable
`DCC_CUA_BROWSER_STORE_PUBLISH_READY` is `true`. Configure required reviewers
on that Environment so a release cannot mutate a store without explicit user
authorization.

Environment variables:

- `CHROME_WEBSTORE_WORKLOAD_IDENTITY_PROVIDER`
- `CHROME_WEBSTORE_SERVICE_ACCOUNT`
- `CHROME_WEBSTORE_PUBLISHER_ID`
- `CHROME_WEBSTORE_EXTENSION_ID`
- `EDGE_ADDONS_CLIENT_ID`
- `EDGE_ADDONS_PRODUCT_ID`

Environment secrets:

- `EDGE_ADDONS_API_KEY`
- `FIREFOX_AMO_API_KEY`
- `FIREFOX_AMO_API_SECRET`

Create and verify each owned store listing once before enabling the ready
variable. Chrome uses GitHub OIDC and Workload Identity Federation rather than
a stored service-account private key. Edge and Firefox secrets are exposed only
to their approved jobs and are never passed as command arguments. Safari
distribution remains a separate App Store Connect workflow.

Run the `Browser store readiness preflight` workflow manually. Its first phase
recaptures the exact GitHub release, tag, workflow run/head, artifact numeric ID,
server digest and expiry, then verifies that the default branch may enter the
`browser-stores` environment. Its second phase runs only after that gate and
performs GET-only store readbacks. Chrome uses a five-minute token with the
`chromewebstore.readonly` scope; Firefox reads the fixed manifest GUID. The Edge
Update API cannot prove a product/version without a prior mutation operation, so
that platform remains `human_action_required` until an authoritative read-only
source exists. The workflow emits only redacted name/presence receipts and never
uploads, submits, publishes, edits a listing, or enables the ready variable.
