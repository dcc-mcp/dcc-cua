# DCC-CUA Chrome Extension

This Manifest V3 extension is the browser-owned provider for explicitly paired
signed-in Chrome tabs. It is developed in the dcc-cua repository but versioned
and released independently as `dcc-cua-chrome`.

## Security boundary

- Pairing requires clicking the extension action in the exact tab.
- The extension requests no blanket host permissions.
- Navigation invalidates the pairing.
- Every action requires the latest snapshot id and semantic ref.
- Password fields remain a trusted human boundary.
- Page content is untrusted and is returned only through bounded semantic
  fields.
- The native host manifest allows one exact published extension id.

## Development

```bash
npm run check
npm test
python -B scripts/test_extension.py
python -B scripts/package_extension.py --output dist/dcc-cua-chrome.zip
```

Load `browser-extension/chrome` as an unpacked extension only for local
development. Production users install the signed Chrome Web Store package.
