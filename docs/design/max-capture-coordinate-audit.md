# Max out-of-desktop refusal: coordinate audit

The supplied refusal binds an exact process/window identity on runtime 1.12.2 to
`visible_desktop_proof/target_outside_desktop`, with target bounds
`[0,0,3841,2281]` and `whole_desktop_fallback=false`. This is a historical
refusal, not fresh proof that those identities or screen conditions still exist.
No capture retry, pixel read, target input, process scan or window mutation was
performed by this source-maintenance task.

Both tag `v1.12.2` and the current source use the same coordinate contract:

- Enter per-monitor-v2 thread DPI awareness and restore the caller's scope.
- Read physical DWM extended-frame bounds for the exact target HWND.
- Represent bounds as `[x,y,width,height]`, with `width=right-left` and
  `height=bottom-top`; no inclusive-edge `+1` is added.
- Read virtual-screen origin and extent through `SM_XVIRTUALSCREEN`,
  `SM_YVIRTUALSCREEN`, `SM_CXVIRTUALSCREEN`, `SM_CYVIRTUALSCREEN` inside that scope.
- Require the complete target rectangle inside that desktop rectangle, using
  wide integer sums. Equal right/bottom edges pass; a one-pixel excess fails.

The supplied target width is 3841 physical pixels. It would exceed a desktop
width of 3840 at the same origin, but the actual desktop measurement and DPI
were not supplied. They must not be guessed from a screenshot size, another
session's monitor, or an older task. Added pure cases cover equal edges, a
one-pixel excess on either axis, and a genuinely 3841-wide matching desktop.
They do not reproduce the user's live display environment.

The currently available tool catalog exposes no project-owned target metadata
call for this exact binding. The checked-in `window-state` schema reports native
bounds/DPI but does not establish virtual-desktop or monitor layout by itself.
The broader `capture-proof` diagnostic also walks occluding roots, outside this
monitor's permitted scope; it was not executed. No provider was re-enabled and
no new MCP or alternate input channel was started to fill the gap.

No coordinate-conversion defect has been established. Preserve the refusal,
visibility proof and disabled whole-desktop fallback. The minimal ordinary
window action is for the user to restore the specified Max window from maximized
state and place its entire frame within the visible desktop. The original Max
owner remains responsible for fresh exact binding and any later authorized
operation. Existing reopen authorization is already acknowledged; this is not
another request to restart or reauthorize Max.
