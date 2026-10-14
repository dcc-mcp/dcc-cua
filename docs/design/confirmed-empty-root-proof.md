# Confirmed empty above-target roots

A visible, uncloaked native root can have zero-area bounds. Rejecting every such
root prevents publication even when it contributes no desktop pixels. Unknown
geometry must still refuse publication; visibility and window styles alone do
not establish an empty footprint.

The Windows adapter delegates geometry classification to a pure policy. Only a
non-target root may produce a confirmed-empty decision, and only when:

- Native visibility is true and the successful cloaking measurement is zero.
- The initial physical DWM rectangle is monotone, representable and zero-area.
- A successful subsequent Win32 measurement has exactly the same raw edges.
- One fresh DWM measurement after Win32 also has exactly those raw edges.

The confirmation is one bounded query, with no retries or fallback geometry.
Positive roots retain their original fast path and acquire no additional native
queries. Target zero-area geometry, failed queries, inconsistent raw edges,
inverted rectangles and arithmetic overflow remain refusals.

The decision retains its original HWND and target-root identity. A confirmed
empty root still appears in the z-order collection and consumes the existing
enumeration budget. Its contribution flag is false; its measured visibility is
not rewritten. Explicit capture diagnostics report the distinct
`confirmed_empty_above_target` decision and all three raw measurements. Failed
confirmations preserve the original bounds refusal and add bounded confirmation
diagnostics. Every positive-area intersection, including a one-pixel overlap,
remains an occluder.

No target binding, capture publication fence, authorization, foreground policy,
task lifecycle or input scope changes. Transparent, no-activate and process/class
identities are not exceptions. New native qualification and a fresh target task
are required before claiming that a real application capture succeeds.
