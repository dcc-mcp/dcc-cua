# Windows discovery outcomes and executor context

Window inventory is metadata discovery, not process-presence evidence or an
authorization to capture, activate, or control a candidate. A process can exist
without a top-level window visible to the current executor. Launcher exit and
an empty inventory do not authorize starting another application or adopting
an unrelated existing instance.

On Windows the native inventory boundary returns an optional result: `None`
selects the existing SDK route on other platforms; a Windows error must propagate
through both runtime discovery and the early CLI list path without an SDK retry.
Successful `list` output retains its existing array shape. Failed enumeration
or incompatible/unverified executor desktop context returns a failure, never a
successful empty/partial array. A completed unfiltered inventory can contain
explicitly incomplete rows (`metadata_complete=false` and `metadata_errors`).
Metadata failure in an unrelated window does not hide fully resolved matches.
Queries reject unknown metadata only when that candidate could still match all
known selectors; they do not silently filter it into absence or disregard a
potential ambiguity. PID/HWND/title/name and on-screen filtering use this same
query reducer in CLI listing/selection, bounded waits and session resolution.

The CLI failure code is `window_discovery_unknown`. Its typed
`details.window_inventory` records `complete=false`, native API/error context
and partial numeric PID/HWND candidates. It does not include window titles or
executable paths. Candidates remain diagnostic only. Native process-name, title,
owner and rectangle errors retain their reason; failed metadata cannot fabricate
an empty identity or valid zero rectangle. A successful empty inventory describes
only the queried windows in the verified context, never application absence.

Readiness reports and production fast gates use the same executor-context
reducer. It checks the current window station, thread desktop, input desktop
identity when readable, and the thread desktop's input ownership. A known
mismatch or unknown ownership blocks local inventory, observation and activation
admission in this executor, retaining the original probe errors and disabling
blind-retry advice. This does not establish that another independently authorized
Host or capture route is unavailable. It grants no desktop/session/identity
switch. Existing exact-window capture proof remains independently required.

In a verified compatible context, a cursor/input-surface failure still blocks
input separately from observation; it does not become a blanket capture denial.
The existing verified Default-thread fallback for an unreadable input-desktop
probe also requires current thread ownership. No native probes are retried to
test whether a denied context has recovered.

For Windows native inventory rows, discovery `--app` accepts the full executable
filename or its exact `.exe` stem, case-insensitively: `Figma` and `Figma.exe`
both match `Figma.exe`, while `Figma Helper.exe` does not. This is not a prefix,
process-family or title match. Unknown names never match. Other platforms retain
their existing exact-name contract. PID/HWND/title predicates remain conjunctive;
ambiguity and final exact binding checks are unchanged. No Electron parent-child
handoff or automatic instance adoption is introduced.

Validation uses host-free outcome reducers and injected metadata failures,
including partial/empty enumeration, OS5 preservation, unknown ownership,
desktop mismatch with and without cursor errors, compatible cursor-only denial,
numeric candidate serialization and selector spelling/identity constraints.
These regressions do not prove permission recovery or native Figma acceptance.
