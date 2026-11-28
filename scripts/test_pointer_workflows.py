"""Verify a scripted pointer sequence on a disposable Blender viewport."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

from test_owned_workflow_e2e import attest, client, structured

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "skills/cua-cli/scripts"))
from action_sequence import window_sequence


def benchmark_pointer(binary, pid, hwnd, directory, report):
    version = json.loads(subprocess.check_output([binary, "manifest"], text=True))["version"]
    print(f"provider=dcc-cua runtime={version} pid={pid} hwnd={hwnd}", flush=True)
    report.update(provider="dcc-cua", runtime=version, pid=pid, hwnd=hwnd, journal=[])
    with client(binary, os.environ.copy(), None) as mcp:
        def checked(result):
            if result.get("isError"):
                error = json.loads(result["content"][0]["text"])
                raise RuntimeError(str(error.get("error_code") or error.get("error") or error.get("code"))[:200])
            return structured(result)

        opened = checked(mcp.tool("start_task", {
            "application_label": "Disposable Blender pointer benchmark", "surface": "window",
            "observation_mode": "pixels_only" if sys.platform.startswith("linux") else "semantic",
            "target_process_id": pid, "target_window_handle": hwnd, "ttl_minutes": 3,
            "allowed_methods": ["snapshot", "execute_action"],
            "allowed_actions": [{"action": name, "input_kind": "raw_input", "secret_input": False,
                                 "authorization_category": "raw_input"} for name in ("move", "click", "drag")]}))
        task = opened["task_id"]
        mcp.tasks.append(task)
        assert attest(opened) == (pid, hwnd) and opened["runtime_version"] == version
        (directory / "bound").touch()

        def call(method, params):
            start = time.perf_counter()
            response = mcp.call(task, method, params)
            report["calls"].append({"method": method, "roundtrip_ms": round((time.perf_counter()-start)*1000)})
            return checked(response)

        snapshot = call("snapshot", {})
        from test_blender_workflows import wait_scene
        scene = wait_scene(directory, lambda s: "pointer_targets" in s)
        source, center, destination = scene["pointer_targets"]
        obs = snapshot["observation"]
        def pixel(fraction):
            return {"x": round(fraction[0]*obs["width"]), "y": round(fraction[1]*obs["height"])}
        common = {"input_kind": "raw_input", "intent": "ordinary_edit", "delivery_mode": "foreground"}
        steps = [dict(common, action="move", **pixel(source)), dict(common, action="click", **pixel(center)),
                 dict(common, action="drag", path=[pixel(source), pixel(destination)], duration_ms=320, steps=8)]
        report["script"] = steps
        log = directory / "pointer.jsonl"
        offset = len(log.read_text().splitlines()) if log.is_file() else 0

        def verify(step, receipt):
            nonlocal offset
            def near(event, target):
                return abs(event[1]-target[0]) < .06 and abs(event[2]-target[1]) < .09
            until = time.monotonic() + 3
            while time.monotonic() < until:
                raw = log.read_text().splitlines() if log.is_file() else []
                events = [json.loads(line) for line in raw[offset:]]
                moves = [e for e in events if e[0] == "move"]
                downs = [e for e in events if e[0] == "down"]
                ups = [e for e in events if e[0] == "up"]
                if step["action"] == "move":
                    ok = not downs and not ups and any(near(e, source) and not e[3] for e in moves)
                elif step["action"] == "click":
                    ok = len(downs) == len(ups) == 1 and all(near(e, center) for e in downs + ups)
                else:
                    ok = (len(downs) == len(ups) == 1 and near(downs[0], source) and near(ups[0], destination)
                          and any(e[3] and source[0]+.05 < e[1] < destination[0]-.05 for e in moves))
                if ok:
                    offset = len(raw)
                    return events
                time.sleep(.02)
            report["last_events"] = events
            raise RuntimeError(f"Blender did not confirm {step['action']}")

        window_sequence(call, (pid, hwnd), snapshot, steps, report["journal"], verify)
        report["metrics"] = checked(mcp.tool("dcc_cua_metrics", {}))
        mcp.stop(task)
