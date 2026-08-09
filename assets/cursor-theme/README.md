# DCC CUA cursor theme

`dcc-cua.cua-theme` is the compiled CUA v2 theme embedded by `dcc-cua` and
installed as `com.dcc-mcp.cursor` in CUA's standard per-user theme store.
It reuses CUA's canonical 12-state vector cursor and applies the DCC CUA purple
palette; application identity remains the real executable icon in the dynamic
control banner.

Rebuild from a compatible CUA v2 `.lottie` source:

```powershell
python scripts/build-dcc-cursor-theme.py <cua.default.lottie> --output assets/cursor-theme/dcc-cua.lottie
cua-cursor-theme build assets/cursor-theme/dcc-cua.lottie --output assets/cursor-theme/dcc-cua.cua-theme
cua-cursor-theme validate assets/cursor-theme/dcc-cua.cua-theme
```

The source design and this derivative are MIT licensed. See the repository and
upstream CUA license notices.
