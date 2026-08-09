"""Deterministic adaptive-encounter task for Cua-Bench.

The task isolates one decision boundary from a live Bazaar run. It deliberately
uses neutral assets and resettable data instead of claiming that a network game
session is reproducible.
"""

import json
import os
from pathlib import Path

import cua_bench as cb

from policy import best_safe_candidate


ROOT = Path(__file__).parent
CASES = json.loads((ROOT / "cases.json").read_text(encoding="utf-8"))
WINDOW_PID: int | None = None


@cb.tasks_config(split="train")
def load() -> list[cb.Task]:
    provider = os.environ.get("DCC_CUA_BENCH_PROVIDER", "simulated")
    default_os = "win11" if provider == "simulated" else "windows"
    os_type = os.environ.get("DCC_CUA_BENCH_OS", default_os)
    return [
        cb.Task(
            task_id=scenario["id"],
            description=(
                "Inspect all three encounter cards, then select the highest-value "
                "candidate that is safe for the CURRENT run state. Do not force a "
                "fixed build or always choose the same position. PvP wins and prestige "
                "define run phase; monster outcomes are not PvP losses. Preserve any "
                "explicitly active stash/passive or positional relationship unless a "
                "verified strict improvement is offered. Right-click reveals a card; "
                "left-click selects it."
            ),
            metadata={"scenario": scenario},
            computer={
                "provider": provider,
                "setup_config": {
                    "os_type": os_type,
                    "width": 1280,
                    "height": 800,
                    "background": "#080a12",
                },
            },
        )
        for scenario in CASES
    ]


@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession | cb.MobileSession):
    global WINDOW_PID
    WINDOW_PID = await session.launch_window(
        html=(ROOT / "gui" / "index.html").read_text(encoding="utf-8"),
        title="DCC CUA Adaptive Encounter Bench",
        width=1120,
        height=700,
    )
    scenario = json.dumps(task_cfg.metadata["scenario"], ensure_ascii=False)
    await session.execute_javascript(WINDOW_PID, f"window.setScenario({scenario})")


@cb.evaluate_task(split="train")
async def evaluate(
    task_cfg: cb.Task,
    session: cb.DesktopSession | cb.MobileSession,
) -> list[float]:
    if WINDOW_PID is None:
        return [0.0, 0.0, 0.0]
    result = await session.execute_javascript(WINDOW_PID, "window.__benchmarkResult")
    if not isinstance(result, dict):
        return [0.0, 0.0, 0.0]

    scenario = task_cfg.metadata["scenario"]
    expected = best_safe_candidate(scenario)
    selected_id = result.get("selectedId")
    selected = next(
        (candidate for candidate in scenario["candidates"] if candidate["id"] == selected_id),
        None,
    )
    safe = selected is not None and selected["level"] <= scenario["maxSafeOpponentLevel"]
    outcome = 1.0 if expected and selected_id == expected["id"] else 0.5 if safe else 0.0

    inspected = len(set(result.get("inspectedIds") or []))
    evidence = min(inspected / scenario["requiredInspections"], 1.0)
    interactions = max(int(result.get("interactionCount") or 0), 0)
    budget = scenario["actionBudget"]
    efficiency = 1.0 if interactions <= budget else max(0.0, budget / interactions)
    return [float(outcome), float(evidence), float(efficiency)]


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession | cb.MobileSession):
    """Oracle validates task wiring; it is not an agent performance baseline."""
    if WINDOW_PID is None:
        return
    scenario = task_cfg.metadata["scenario"]
    for candidate in scenario["candidates"]:
        await session.execute_javascript(
            WINDOW_PID, f"window.inspectCandidate({json.dumps(candidate['id'])})"
        )
    expected = best_safe_candidate(scenario)
    if expected:
        await session.execute_javascript(
            WINDOW_PID, f"window.selectCandidate({json.dumps(expected['id'])})"
        )
