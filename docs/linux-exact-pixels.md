# Read-only exact pixels on Linux X11

This incremental implementation is stacked after #315 (Windows exact pixels
and native recording) and #317 (bounded passive capture preparation). Merge and
validate those foundations first, then revalidate this increment against the
resulting main. A Linux CI pass does not validate their native Windows behavior.

The Linux route accepts `snapshot --pid PID --window-id XID --pixels-only`.
It is deliberately limited to an authenticated local Unix X11 display without a
compositor, a viewable unobscured rectangular window, and 24-bit TrueColor pixels.
It never starts an accessibility session, invokes ImageMagick, captures/crops the
desktop, changes focus, synthesizes input, records video, or changes permissions.
`--activate`, raw input, live capture, capture preparation and recording remain
unsupported for this route. macOS, Wayland/XWayland, remote X11, shaped windows,
composited desktops and other pixel formats fail closed. These limits are also
advertised in the platform manifest; protocol method names are not capabilities.

The pinned upstream Linux backend already offers X11 capture, but its persistent
MIT-SHM/XGetImage cache and ImageMagick fallback do not expose the per-session
window lifecycle, before/after publication proof or bounded transport needed
here. This narrow adapter uses the same already-resolved x11rb dependency for a
fresh GetImage on the retained exact-target connection; it does not duplicate
upstream accessibility or input APIs. Broader upstream backend reuse should wait
for equivalent evidence/deadline contracts rather than weakening these gates.

A retained X11 connection subscribes to the selected window's lifecycle before
reading its PID metadata. `/proc` start time and executable bind the process;
destruction or owner-property changes revoke the window even if its XID and PID
are immediately reused. Geometry, viewability, ancestor clipping and stacking are
checked before and after a fresh GetImage and PNG encoding. Layout events during
capture discard the frame. Any failed capture requires a new explicit session.
Native transport reads/writes share a three-second deadline; nonblocking local
connect fails immediately if unavailable. Stop drains an in-flight read before
releasing the connection. There is no implicit reconnect or retargeting.

X11 itself trusts its local same-user clients. `_NET_WM_PID` is client-provided
metadata, not a kernel security boundary; this route is not suitable for an
untrusted/malicious X server or peer client. OCR, dialog semantics, destructive
button authorization, and cross-application isolation are not provided here.

## Verification

Linux CI compiles the exact pull-request head and runs two real Xvfb checks:

- The core session test verifies a fresh second frame, no SDK/provider/input
  calls, refusal after destroy/recreate with the same PID/XID, and cancellation
  while a private Xvfb server stalls, followed by bounded stop without publication.
- The release CLI test verifies actual red and blue PNG pixels, exact selection
  between two windows with one PID, and rejection of wrong PID, activation,
  Wayland, remote DISPLAY, occlusion, hidden and closed targets. Failed commands
  must not create an output image.

These synthetic native X11 checks do not constitute Wayland, native macOS,
composited desktop, or production DCC acceptance. Linux runtime behavior is not
considered verified until these exact-head CI checks pass. No version or release
scope is changed by this increment.
