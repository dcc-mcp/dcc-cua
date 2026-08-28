# Browser artwork provenance

These proposed store assets derive from the repository's existing CUA cursor
mark. They are original AI-assisted artwork prepared on 2026-08-28, not product
screenshots or evidence of store approval. The project owner should review the
artwork before a store submission.

| Source | SHA-256 |
| --- | --- |
| `browser-icon-source.png` | `aba84d4a4a3b21f669ca3812d6b1081484043170f4c04bce83e7bbf7e2e108cf` |
| `promo-source.png` | `8356d2a7133c175466d477166dffc2c86c3f7c0342785a844d20d791694671a8` |

The icon source has a white background. The 128px packaged variant adds 16px
of real transparent padding; it does not use a painted transparency grid.
The promotional tile has no product UI, testimonials or approval badges.

Regenerate the checked-in PNGs from the repository root:

```sh
uv run --python 3.13 --with pillow==11.3.0 python -B browser-extension/chrome/scripts/generate_store_artwork.py
```

Pillow is needed only to regenerate artwork. Normal builds copy the checked-in
PNGs; packaging verifies their declared dimensions and exact source bytes.
Store-only artwork stays outside the installable browser ZIP.
