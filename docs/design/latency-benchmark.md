# DCC-CUA latency measurements

Measure before changing the runtime. PNG encode CPU time is one component;
it cannot establish an end-to-end speedup. Keep protocol, native Host and agent
workflow measurements separate. None of these measurements authorize UI access.

## Protocol baseline without desktop access

Use a private copy of a release artifact. Verify its archive checksum and install
manifest first. Do not replace an installed binary or terminate existing MCPs.
Record the release/source commit, artifact hash, OS and power/load conditions
alongside the report. Run baseline and candidate alternately under comparable load.

```powershell
vx uv run --no-project python scripts/cua_latency.py protocol `
  --binary C:/private/runtime/dcc-cua.exe `
  --expected-sha256 <manifest-binary-sha256> --expected-version 1.14.0 `
  --trials 30 --warm-samples 10 --output C:/private/evidence/protocol.json
vx uv run --no-project python scripts/cua_latency.py summarize C:/private/evidence/protocol.json
```

The only requests are `initialize`, `notifications/initialized` and `ping`.
The runner creates one process per trial, retains the exact process handle and
closes stdin after each trial. A bounded exit timeout triggers termination of that
owned child only, recorded as failed EOF cleanup. There are no task, discovery,
Host, screenshot, accessibility or input calls. A failed request ends that trial;
there is no replay. Evidence files are created exclusively, never overwritten.
Connection diagnostics use a fresh sibling directory beside the output, protecting
other owners' registry records from terminal-record retention.

| Phase/stage | Measured interval |
| --- | --- |
| cold/startup | Before process spawn through validated initialize response and initialized notification write |
| warm/end_to_end | Client serialization/write through parsed, correlated ping result on the same connection |
| cold/cleanup | Stdin EOF through process exit (includes fallback wait if EOF exit fails) |

The cold measurement includes process creation and MCP initialization, but not
native Host initialization. It does not flush the OS page cache. Warm samples
share a process; report trials and samples per process, and do not treat every
ping as an independent process-start experiment. This baseline excludes model and
Agent Host round trips and cannot prove desktop-task performance or orphan recovery.

## Native task baseline after desktop coordination

Before observing or entering input, register the exact disposable project-owned
target and desktop window with the coordinating chat. Publish `provider=dcc-cua`,
runtime version, PID, full process birth and HWND; use the project's authorization
and exact-target routing. Preserve other applications, browser sessions and daemons.

Use a fixed fixture, target dimensions, observation mode and operation. Capture
each of these intervals at its actual boundary, without inventing absent stages:

| Stage | Boundary |
| --- | --- |
| startup | Task start through successful target-bound session readiness |
| queue | Host dispatch admission through start of execution |
| capture | Native capture call through encoded exact-target image availability |
| uia | Accessibility request through bounded tree/token result |
| action | Admitted mutation through native action completion |
| verification | Post-action observation plus fixture assertion of the intended effect |
| end_to_end | Client request send through verified operation completion |
| cleanup | Task stop through session acknowledgement and owned Host completion |

For each operation record raw monotonic durations in milliseconds, outcome, trial
identity, scenario, source/binary hash and binding. Keep task-start and first-operation
cold samples separate from warm operations in that same task. A pixels-only run
has no UIA samples. Existing `host-jsonl.metrics.v3` totals/maxima cannot reconstruct
percentiles or queue time. `capture_diagnostics` is opt-in and adds work: measure
instrumented and normal runs separately. The existing ignored SDK fixture
`owned_window_snapshot_latency` provides capture measurements only, not MCP E2E.

The summarizer accepts an already-collected, private native report in this shape:

```json
{
  "schema": "dcc-cua.latency.v1",
  "boundary": "native_task",
  "samples": [{
    "trial_id": "example-only",
    "scenario": "fixture-click-capture-after",
    "phase": "warm",
    "stage": "end_to_end",
    "duration_ms": 123.0,
    "outcome": "success",
    "binding": {
      "provider": "dcc-cua", "runtime": "1.14.0",
      "pid": 42, "process_birth": "2026-10-24T00:00:00Z", "hwnd": 123,
      "observation_mode": "pixels_only", "width": 1280, "height": 720
    }
  }]
}
```

This example is synthetic. The summarizer validates structure, not authenticity
or authorization. Put source/hash/scenario details in binding when comparing runs;
all binding fields participate in grouping. Never mix different input operations,
diagnostics settings or cleanup contracts in one distribution. The report can
include additional private metadata; do not store screenshot content or tool args.

Percentiles use nearest rank and keep outliers. Errors and timeouts remain in
attempt counts, outside success-only percentiles. Missing stages remain unmeasured.
Below 100 successes the tail warning is set: p95 is descriptive, not a stable
performance guarantee. Stage percentiles overlap and must never be added or
subtracted to derive end-to-end percentiles. Compare correctness, rejection and
cleanup rates alongside latency before accepting an optimization.
