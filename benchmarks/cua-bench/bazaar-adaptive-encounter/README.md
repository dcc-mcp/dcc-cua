# Adaptive encounter benchmark

This resettable Cua-Bench task measures one decision boundary extracted from a
live Bazaar run: inspect three candidates and choose the highest current-build
utility that does not exceed the run's safe opponent level. Run phase is based
on PvP wins and prestige, not monster outcomes. One variant also makes an
explicit stash/passive and positional relationship a hard preservation
constraint, so blindly taking the highest nominal utility fails. Four variants
move the safe ceiling and the best position, so a fixed-position policy fails.

The UI uses neutral generated shapes and synthetic cases. It is a policy and
interaction regression test, not a replay of the game and not evidence that a
complete network run is deterministic.

## Run

Install Cua-Bench in a separate Python 3.12 environment. Do not add it to the
shipped `dcc-cua` runtime.

```powershell
cb interact benchmarks/cua-bench/bazaar-adaptive-encounter --variant-id 0
cb interact benchmarks/cua-bench/bazaar-adaptive-encounter --oracle --variant-id 0
```

The default provider is Cua-Bench's simulated desktop. A native Windows run can
be selected without changing task code:

```powershell
$env:DCC_CUA_BENCH_PROVIDER = 'native'
$env:DCC_CUA_BENCH_OS = 'windows'
cb interact benchmarks/cua-bench/bazaar-adaptive-encounter --variant-id 0
```

For an agent run, register a Cua-Bench custom agent whose `perform_task` uses
`dcc-cua host-jsonl` as its control bridge. Start the bridge with
`--metrics-output`, then gate its finalized metrics next to Cua-Bench's three
scores (`outcome`, `inspection evidence`, `interaction efficiency`):

```powershell
python benchmarks/cua-bench/bazaar-adaptive-encounter/metrics_gate.py `
  artifacts/cua-bench/dcc-cua-metrics.json `
  --live-state artifacts/cua-bench/live-observation-state.json `
  --max-actions 4 --max-moves 0 --max-snapshots 4 --max-json-bytes 65536 `
  --min-recent-fps 8 --max-capture-ms 125 --max-frame-age-ms 250
```

The external metrics gate prevents a correct answer obtained by excessive
actions, observations, or JSON traffic from passing the development budget.
Token accounting remains owned by Cua-Bench's custom-agent `AgentResult`; JSON
wire bytes are only the dcc-cua transport proxy. When `--live-state` is supplied,
the same gate also requires an active persistent capture feed with recent frame
rate, capture latency, freshness, and zero-failure evidence; lifetime average
FPS alone is intentionally insufficient.
