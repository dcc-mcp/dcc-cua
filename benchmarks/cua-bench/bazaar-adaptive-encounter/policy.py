"""Pure decision policy shared by the benchmark evaluator and its tests."""


def best_safe_candidate(scenario: dict) -> dict | None:
    safe = [
        candidate
        for candidate in scenario["candidates"]
        if candidate["level"] <= scenario["maxSafeOpponentLevel"]
        and (
            not scenario.get("mustPreserveActiveEffect", False)
            or candidate.get("preservesActiveEffect", True)
        )
    ]
    return max(safe, key=lambda candidate: candidate["currentBuildUtility"], default=None)
