# In-process Qt capture boundary

This is groundwork for [capture issue 391](https://github.com/dcc-mcp/dcc-cua/issues/391),
not a new runtime capability. No released snapshot tool accepts `in_process_qt`
as a result of this change. No disconnected-desktop acceptance is claimed.

## Ownership

A DCC adapter must own its Qt objects and document lifecycle. The external CUA
Host cannot implement `QWidget.grab()` by injecting a startup file or evaluating
caller-supplied Python. The adapter's existing main-thread dispatcher is the
appropriate execution port; the existing Qt inspector supplies widget discovery.
Keep host-side capture inside that process and route it through project UI Control.

The next adapter integration needs a constructor-owned registration containing:

- the connection/task owner, exact host process birth and native root window;
- one exact widget identity, with ambiguous object names rejected;
- an adapter-owned document generation and ready/not-ready state;
- a bounded queue with explicit full, expired and disconnected outcomes;
- a host lifecycle callback that removes the registration, pending commands and
  document-load callback at task end or disconnect.

The caller selects an advertised mode and discovered widget, never a script,
startup-file path, arbitrary callback or different PID. An adapter that cannot
provide its GUI-thread and teardown guarantees must advertise this mode as
unsupported. Disconnecting the desktop does not grant new authority.

The shared `dcc_cua_protocol::in_process_qt` module now defines the adapter
boundary for this exchange. It validates the connection, task, process,
window, document-generation and widget-generation binding, carries the
bounded queue policy, and provides typed readiness, command status and failure
values. These types make the adapter contract explicit without treating a
declaration as proof that a live DCC adapter has installed the hook.

Core capability discovery is tracked separately in
[core issue 2867](https://github.com/dcc-mcp/dcc-mcp-core/issues/2867). A schema
enum is not evidence that any live adapter implements that mode. Supported
declarations must remain tied to the connected host and its actual probe result.

## Shared frame quality contract

`dcc_cua_protocol::frame_quality` defines a pure assessment of complete, tightly
packed BGRA bytes. It performs no UI operation and retains no pixels. Native
and future adapter ports can use the same vocabulary after their ownership,
freshness, geometry and lifecycle checks.

The assessment records dimensions, pixel count, per-channel RGB mean and
population standard deviation in thousandths, and per-channel Shannon entropy
in thousandths of a bit. Histograms and alpha/high-byte data are not exposed.
BI_RGB's fourth byte does not establish content and is ignored.

The initial blank threshold is exact uniformity in all three RGB channels.
This catches uniform black, white and colored frames without inventing a
near-blank heuristic. Classification uses occupied histogram bins, not rounded
statistics; sparse differences cannot disappear through rounding. A deliberately
solid widget is reported as `blank_capture` even if it was intentionally painted.
Conversely, `informative` means only nonuniform RGB pixels, never that the right
widget, graph or scene was captured. Input authority still requires the owning
runtime's independent evidence checks.

Blank outcomes retain the attempted capture mode and numeric statistics.
Zero dimensions, incomplete/extra bytes and buffers above the existing binary
transport ceiling are typed data failures, not valid frames. Backends must remove
row padding before assessment; no successful crop or implicit repair is inferred.

The module is not wired into existing capture publication yet. Its focused tests
qualify the numeric/wire contract only. Integration must add assessment before
returning success, preserve blank evidence in the tool response, and verify that
no blank frame mints an observation or input token.

## Remaining acceptance

Before closing issue 391, implement the adapter capture/hook port and discovery,
then validate an exact named Qt widget in a disconnected Windows desktop session.
Retain nonuniform native-size pixels and their statistics. Exercise an intentional
blank target, absent/destroyed/ambiguous widgets, document-generation changes,
queue overflow, readiness timeout and disconnect/task-end teardown. Prove no
startup files or hooks remain. Generated BGRA fixtures alone cannot establish any
of those native or lifecycle results.
