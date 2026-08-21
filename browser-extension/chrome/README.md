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
