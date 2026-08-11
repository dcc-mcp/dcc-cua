# DCC CUA Control Surface V1

This directory preserves the original **Control Surface Audit / Design Lock
V1** as a self-contained, offline design snapshot. It is intentionally kept for
comparison and future iteration; it is not an implementation specification.

The design explores:

- single-owner boundaries between the CUA cursor overlay, DCC CUA Core, and the
  DCC CUA indicator;
- a compact one-line safety banner with application identity, agent identity,
  activity, and an always-visible Esc stop affordance;
- windowed and full-screen placement behavior;
- explicit text for connecting, ready, observing, pointer, keyboard,
  navigation, waiting, recording, and stopping states;
- a fail-closed data-flow contract for identity, placement, activity, and
  capture policy.

## Preview

Open [index.html](index.html) directly, or serve the repository root locally:

```powershell
python -m http.server 8765
```

Then visit:

```text
http://127.0.0.1:8765/docs/design/control-surface-v1/
```

## Authority boundary

Values shown in the snapshot, including the 44 px banner, are historical design
choices. Current source contracts and validated themes remain authoritative;
for example, the independently tested target-frame treatment uses a 45-DIP
inward gradient and has separate multi-monitor acceptance requirements.

Application names and icons are included only as illustrative UI references.
Maya, 3ds Max, and The Bazaar are trademarks of their respective owners.
