"""Gate dcc-cua transport/interaction cost alongside a Cua-Bench outcome."""

import argparse
import json
from pathlib import Path


def evaluate_metrics(
    report: dict,
    *,
    max_actions: int,
    max_moves: int,
    max_snapshots: int,
    max_json_bytes: int,
) -> dict:
    total_json_bytes = int(report.get("json_input_bytes", 0)) + int(
        report.get("json_output_bytes", 0)
    )
    checks = {
        "schema_v2": report.get("schema") == "dcc-cua.host-jsonl.metrics.v2",
        "completed": report.get("run_status") == "succeeded"
        and report.get("transport_success") is True,
        "no_protocol_errors": int(report.get("errors_total", 0)) == 0,
        "action_budget": int(report.get("action_requests_total", 0)) <= max_actions,
        "movement_budget": int((report.get("action_kinds") or {}).get("move", 0))
        <= max_moves,
        "snapshot_budget": int(report.get("standalone_snapshot_requests_total", 0))
        <= max_snapshots,
        "wire_budget": total_json_bytes <= max_json_bytes,
    }
    return {
        "passed": all(checks.values()),
        "checks": checks,
        "observed": {
            "actions": int(report.get("action_requests_total", 0)),
            "moves": int((report.get("action_kinds") or {}).get("move", 0)),
            "standaloneSnapshots": int(report.get("standalone_snapshot_requests_total", 0)),
            "jsonBytes": total_json_bytes,
            "elapsedMs": int(report.get("elapsed_ms", 0)),
        },
    }


def evaluate_live_observation(
    document: dict,
    *,
    min_recent_fps: float,
    max_capture_ms: int,
    max_frame_age_ms: int,
) -> dict:
    state = document.get("result", document)
    recent_fps = state.get("recent_effective_fps")
    capture_ms = state.get("last_capture_duration_ms")
    frame_age_ms = state.get("latest_frame_age_ms")
    checks = {
        "active": state.get("active") is True,
        "persistent_capture": state.get("capture_mode") == "persistent_wgc",
        "no_capture_failures": int(state.get("capture_failures", 0)) == 0,
        "recent_fps": isinstance(recent_fps, (int, float))
        and float(recent_fps) >= min_recent_fps,
        "capture_latency": isinstance(capture_ms, (int, float))
        and float(capture_ms) <= max_capture_ms,
        "frame_freshness": isinstance(frame_age_ms, (int, float))
        and float(frame_age_ms) <= max_frame_age_ms,
    }
    return {
        "passed": all(checks.values()),
        "checks": checks,
        "observed": {
            "captureMode": state.get("capture_mode"),
            "recentFps": recent_fps,
            "captureMs": capture_ms,
            "frameAgeMs": frame_age_ms,
            "captureFailures": int(state.get("capture_failures", 0)),
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("metrics", type=Path)
    parser.add_argument("--max-actions", type=int, default=4)
    parser.add_argument("--max-moves", type=int, default=0)
    parser.add_argument("--max-snapshots", type=int, default=4)
    parser.add_argument("--max-json-bytes", type=int, default=65_536)
    parser.add_argument("--live-state", type=Path)
    parser.add_argument("--min-recent-fps", type=float, default=8.0)
    parser.add_argument("--max-capture-ms", type=int, default=125)
    parser.add_argument("--max-frame-age-ms", type=int, default=250)
    args = parser.parse_args()
    report = json.loads(args.metrics.read_text(encoding="utf-8"))
    result = evaluate_metrics(
        report,
        max_actions=args.max_actions,
        max_moves=args.max_moves,
        max_snapshots=args.max_snapshots,
        max_json_bytes=args.max_json_bytes,
    )
    if args.live_state:
        live_document = json.loads(args.live_state.read_text(encoding="utf-8"))
        live_result = evaluate_live_observation(
            live_document,
            min_recent_fps=args.min_recent_fps,
            max_capture_ms=args.max_capture_ms,
            max_frame_age_ms=args.max_frame_age_ms,
        )
        result["liveObservation"] = live_result
        result["passed"] = result["passed"] and live_result["passed"]
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
