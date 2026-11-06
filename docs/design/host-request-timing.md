# Opt-in Host request timing

## Decision and scope

The first diagnostic boundary is a Host request, not a complete desktop task.
An embedding Agent Host can supply a `HostTimingObserver` through
`HostSecurityServices::with_timing_observer`. The default is disabled. IPC
requests cannot enable or replace this observer, and it grants no authority.
Normal control responses, image attachments and existing metrics are unchanged.
The Host adapter boxes its large dispatch future to bound stack use in both
modes. Disabled timing is not a claim of zero allocations or zero overhead.

The observer receives a bounded, content-free `HostTimingSample` after handling
finishes or the handling future is dropped. It must return promptly; use a
bounded non-blocking sink and account for discarded samples in the embedding
application. Do not perform disk or network I/O in `record`. Observers must not
panic; unwind panics are contained without replacing the operation result.
A slow observer can still delay its caller, and an abort build cannot contain
a panic.

## Recorded boundaries

| Field or phase | Start and end | Coverage limit |
| --- | --- | --- |
| request `elapsed_ns` | Entry to Host handling through evidence finalization, or destruction of that handling future | Excludes frame reading, response rewriting/writing and observer delivery |
| `session_preparation` | Existing granted window-session construction/start call | Excludes preauthorization and final authorization; desktop-session startup is not instrumented |
| `raw_input_queue` | Waiting for the action helper's OS-input FIFO guard | ExecuteAction and ExecuteDesktopAction only; excludes lock ownership time and other direct lock sites |
| `snapshot` | Explicit window/popup capture orchestration, or accessibility-only provider observation | Includes provider checks/encoding performed by that route; not a separate GPU or PNG measurement |
| `native_action` | Existing session `perform_action` call | A driver operation including its internal checks, not just OS input injection |
| `post_action_wait` | Existing explicit post-action-delay helper | Includes its interruption checks; does not add or change a delay |
| `post_observation` | Existing post-action accessibility or screenshot call | Does not add an observation or change its mode |
| `condition_wait` | Existing `wait_for` handler or `wait_for_window` provider call | Includes polling and predicate work; retains the original deadline and cancellation rules |
| `verification` | Existing provider `verify_state` call | Excludes image response preparation and Host evidence finalization |
| `session_cleanup` | Existing explicit window `stop_session` call | Excludes automatic idle/disconnect cleanup and desktop-session cleanup |

Stages are aggregated per request in a fixed-size collector. Missing stages are
absent, not zero. `count` and `total_ns` cover futures that returned, including
those returning an error. `dropped_count` counts entered futures that did not
return and contributes no completed duration. It does not establish whether
native work stopped or an effect occurred. Request outcomes `returned`, `error`
and `dropped` describe handling, not business success, rollback or input release.
An `Ok` response may contain a refusal, timeout, unknown effect or failed
post-observation. Preserve and inspect the original response contract.

All durations use a monotonic clock. The request total contains stage time and
other work; stages and request totals must not be added into an end-to-end total.
Nanosecond units do not imply nanosecond measurement accuracy.
Unpolled requests and frames rejected before entering the handler have no
sample. Connection capacity admission and same-connection head-of-line waiting
are outside the handler clock; `raw_input_queue` measures only the documented
action FIFO acquisition.

## Privacy and ownership

Records contain a schema, process-local request sequence, closed request/phase
and handling-outcome enums, durations and counts. They contain no caller request
ID, session/task ID, method string, parameters, results, error text, paths, URLs,
window identity, coordinates, accessibility content, pixels or pixel summaries.
The local sequence identifies a sample, not a trusted task binding. Concurrent
requests may publish out of sequence; timing alone cannot associate them with
one business task. Keep any authorized task correlation in the embedding host.

No authorization, freshness/generation, native-instance, geometry/DPI,
Validation/Publication, input ownership or cleanup condition is weakened.
Timing does not add retries. Partial actions and failed post-observations retain
their existing unknown-effect and fresh-observation requirements. Callback
delivery occurs after the handling future and its input guards are destroyed.

## Validation and remaining measurements

Ordinary tests use synthetic futures and in-memory observers. They check
unchanged return values, errors, dropped/timeout futures, bounded stage records,
isolation, input-guard lifetime and content-free serialization. Protocol tests
use a fake channel that cannot dispatch to a desktop and compare response frames
with timing disabled/enabled. Connection-generated random IDs are normalized only
where necessary for comparison.

The ignored release probe compares disabled/enabled timing on the same synthetic
work in alternating paired batches. It reports instrumentation overhead only.
Its sink, payload, batch size and measured boundary must accompany the results;
do not extrapolate them to screenshot speed or desktop-task latency. Ordinary
tests impose no wall-clock performance threshold.
The disabled comparison uses this version's wrapper, not the previous
unwrapped Host. It does not measure the adapter's additional boxing or prove
that the default path has no regression against the previous version.

Connection/session negotiation, transport, MCP conversion, model/Agent Host
round trips, automatic cleanup, WGC initialization/readback/encoding sub-stages
and full task completion remain separate measurements. Do not infer queue time
by subtracting client and Host timings. Before native measurements, follow the
ownership, runtime-identity and fair-comparison requirements in
[latency-benchmark.md](latency-benchmark.md). Use task assertions and report
failures, unknown outcomes and cleanup alongside successful latency samples.
