"""Read content-free performance counters for one completed Codex Host turn."""

import argparse
import json
from pathlib import Path


def turn_metrics(path, turn_id):
    latest = baseline = None
    active = False
    measured = False
    reset = False
    methods = {}
    fields = ("input_tokens", "cached_input_tokens", "output_tokens")
    with path.open("rb") as source:
        for line in source:
            row = json.loads(line)
            if row.get("type") != "event_msg":
                continue
            event = row["payload"]
            kind = event.get("type")
            if kind == "task_started":
                if active:
                    raise ValueError("selected turn has no completion receipt")
                active = event.get("turn_id") == turn_id
                if active:
                    baseline = latest
            elif kind == "token_count" and event.get("info"):
                info = event["info"]
                total = info.get("total_token_usage")
                if not isinstance(total, dict):
                    continue
                if active:
                    if baseline is None and total == info.get("last_token_usage"):
                        baseline = dict.fromkeys(fields, 0)
                    if latest and any(
                        total.get(f, 0) < latest.get(f, 0) for f in fields
                    ):
                        reset = True
                    measured |= total != latest
                latest = total
            elif active and kind == "item_completed":
                item = event.get("item", {})
                if (
                    item.get("type") == "McpToolCall"
                    and item.get("server") == "dccCuaTaskAuthorization"
                ):
                    method = item["tool"]
                    if method == "dcc_cua_task_call" and isinstance(
                        item.get("arguments"), dict
                    ):
                        method = item["arguments"].get("method", method)
                    stats = methods.setdefault(method, {"calls": 0, "max_ms": None})
                    stats["calls"] += 1
                    duration = item.get("duration")
                    if (
                        isinstance(duration, dict)
                        and "secs" in duration
                        and "nanos" in duration
                    ):
                        ms = round(
                            duration["secs"] * 1000 + duration["nanos"] / 1_000_000, 2
                        )
                        stats["max_ms"] = max(stats["max_ms"] or 0, ms)
            elif active and kind == "task_complete" and event.get("turn_id") == turn_id:
                usage = None
                if measured and not reset and baseline is not None:
                    usage = {
                        f: latest[f] - baseline[f]
                        for f in fields
                        if f in latest and f in baseline
                    }
                cached = (usage or {}).get("cached_input_tokens")
                input_tokens = (usage or {}).get("input_tokens")
                return {
                    "scope": "codex_host_turn",
                    "elapsed_ms": event.get("duration_ms"),
                    "time_to_first_token_ms": event.get("time_to_first_token_ms"),
                    "model_usage": usage,
                    "cached_input_fraction": cached / input_tokens
                    if cached is not None and input_tokens
                    else None,
                    "dcc_cua_methods": methods,
                }
    raise ValueError("completed turn not found")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rollout", type=Path)
    parser.add_argument("turn_id")
    args = parser.parse_args()
    try:
        metrics = turn_metrics(args.rollout, args.turn_id)
    except (ValueError, OSError) as error:
        parser.exit(1, f"{error}\n")
    print(json.dumps(metrics, indent=2))


if __name__ == "__main__":
    main()
