# Continuity contract for visual CUA tasks

Long-running visual work must not be represented as a list of coordinates. The
agent carries a small, explicit state machine and every action advances an
observation chain:

```text
observe(frame, target, epoch)
  -> ground candidate (bound to frame + epoch)
  -> act(candidate)
  -> receipt + transition fence
  -> publish observation(epoch + 1)
  -> revalidate target and continue
```

`dcc-cua-protocol::continuity` provides the transport-neutral primitives:
`TargetBinding`, `ObservationFrame`, `ActionReceipt`, and
`validate_next_observation`. A stale frame, changed process/window, incomplete
receipt, or non-advancing epoch fails closed. This is intentionally below a
vision model: models propose candidates, while the host owns exact-target and
freshness validation.

## QQ classic farm harvest example

The following is a workflow shape, not live QQ acceptance. It must be bound to
the actual QQ mini-program process and native window handle before execution.

1. Capture the farm grid and create `frame-0` with the QQ target binding.
2. Ground one harvestable plot by semantic identity (not a remembered pixel).
3. Dispatch `harvest(plot_id)` and wait for its completion receipt and
   transition fence.
4. Consume that receipt, publish the next frame/epoch, and revalidate the same
   QQ target before selecting another plot.
5. Repeat until the semantic grid reports no harvestable plots; verify the
   completion state and save the episode trace.

If a client cannot return the chained receipt and observation epoch, batching is
restricted to actions that cannot invalidate semantic references. Harvest clicks
must therefore remain single-step actions unless the client implements this
contract.

## Continuity layers

- **Working state:** current goal, active subgoal, latest frame/observation,
  target binding, receipt and epoch.
- **Episodic trace:** immutable frame, candidate, receipt, verification and
  recovery records for replay and diagnosis.
- **Skill memory:** reusable policies such as “scan grid, harvest one plot,
  refresh, repeat”, with no coordinates or window handles baked in.

Visual models should run on keyframes, semantic deltas, action completion, or
uncertainty—not on every transport round trip. Accessibility/UIA/DOM evidence
remains the preferred grounding route; pixels are a fallback for candidate
discovery and verification.

The contract supports continuity; it does not claim that a model can yet solve
all long-horizon visual tasks. Progress should be measured with long-horizon
success, stale-action rate, recovery rate, grounding accuracy, replans, and
vision latency.
