"""Run the two example macros on a disposable foreground Blender window via DCC-CUA."""
import argparse
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from test_owned_workflow_e2e import attest, client, structured


ROOT = Path(__file__).resolve().parents[1]
CASES = [("model", "blender-extrude-column.json"), ("rig", "blender-rig-keyframes.json")]
NO_CONSOLE = getattr(subprocess, "CREATE_NO_WINDOW", 0)


def unique_window(process, directory):
    pid = process.pid
    if sys.platform == "darwin":
        finder = directory / "find-window"
        swift = directory / "find.swift"
        swift.write_text("""
import CoreGraphics
import Foundation
let pid = Int(CommandLine.arguments[1])!
let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
for window in windows where window[kCGWindowOwnerPID as String] as? Int == pid && window[kCGWindowLayer as String] as? Int == 0 {
    if let id = window[kCGWindowNumber as String] as? Int { print(id) }
}
""", encoding="utf-8")
        subprocess.run(["swiftc", str(swift), "-o", str(finder)], check=True, timeout=120)
        command = [str(finder), str(pid)]
    elif os.name == "nt":
        command = ["powershell", "-NoProfile", "-Command",
                   f"(Get-Process -Id {pid} -ErrorAction Stop).MainWindowHandle.ToInt64()"]
    else:
        command = ["wmctrl", "-lp"]
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"Blender exited before the fixture was ready (exit {process.returncode})")
        if not (directory / "ready").is_file():
            time.sleep(0.1)
            continue
        lines = subprocess.check_output(command, text=True, timeout=5, creationflags=NO_CONSOLE).splitlines()
        if sys.platform.startswith("linux"):
            candidates = [int(parts[0], 16) for line in lines
                          if len(parts := line.split()) >= 3 and parts[2] == str(pid)]
        else:
            candidates = [int(line) for line in lines if line.isdecimal() and int(line) > 0]
        if len(candidates) == 1:
            return candidates[0]
        time.sleep(0.1)
    if not (directory / "ready").exists():
        raise RuntimeError("the Blender scene fixture did not reach its first UI timer")
    raise RuntimeError("no unique native window for the ready Blender process")


def angle(first, second):
    norm = math.sqrt(sum(v * v for v in first) * sum(v * v for v in second))
    cosine = abs(sum(a * b for a, b in zip(first, second))) / norm
    return math.degrees(2 * math.acos(min(1, cosine)))


def verified(case, state, before):
    if state["mode"] != "OBJECT" or set(state["objects"]) - before != {state.get("name")}:
        return False
    if case == "model":
        return (state.get("name", "").startswith("macrocolumn") and state.get("vertices") == 8
                and state.get("faces") == 6 and state.get("height") == 2 and state.get("scale") == [3, 1, 1])
    bones = state.get("bones", [])
    if state["frame"] != 9 or len(bones) != 3 or bones[0]["parent"] is not None:
        return False
    if [(b["head"], b["tail"]) for b in bones] != [([0,0,0], [0,0,1]), ([0,0,1], [0,0,2]), ([0,0,2], [1,0,2])]:
        return False
    if any(not bones[i]["connected"] or bones[i]["parent"] != bones[i-1]["name"] for i in (1, 2)):
        return False
    curves = state.get("curves", [])
    for path in {c["path"] for c in curves if c["path"].endswith(".rotation_quaternion")}:
        channels = sorted((c for c in curves if c["path"] == path), key=lambda c: c["index"])
        if len(channels) != 4 or not all([p[0] for p in c["points"]] == [1, 9] for c in channels):
            continue
        first, last = ([c["points"][i][1] for c in channels] for i in (0, 1))
        if abs(angle(first, last) - 30) < 0.05 and 10 < angle(first, [c["mid"] for c in channels]) < 20:
            return True
    return False


def wait_scene(directory, predicate):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        file = directory / "scene.json"
        if file.is_file():
            state = json.loads(file.read_text(encoding="utf-8"))
            if predicate(state):
                return state
        time.sleep(0.03)
    raise RuntimeError("Blender scene readback did not confirm the expected state")


def checked_result(response, method, report):
    if response.get("isError"):
        diagnostic = {"method": method}
        for item in response.get("content", []):
            if item.get("type") == "text":
                try:
                    error = json.loads(item["text"])
                except ValueError:
                    continue
                if isinstance(error, dict):
                    diagnostic.update(code=error.get("error_code", error.get("code")), reason=error.get("reason"))
                    break
        report["tool_failure"] = diagnostic
        raise RuntimeError(f"{method} failed: {diagnostic.get('code')} ({diagnostic.get('reason')})")
    return structured(response)


def benchmark(binary, pid, window, directory, report):
    manifest = json.loads(subprocess.check_output([binary, "manifest"], text=True, creationflags=NO_CONSOLE))
    version = manifest["version"]
    print(f"provider=dcc-cua runtime={version} pid={pid} hwnd={window}", flush=True)
    report.update(provider="dcc-cua", runtime=version, pid=pid, window_handle=window)
    capability = manifest["runtime"].get("exact_window_pixels", {}).get("key_sequence")
    actions = [{"action": name, "input_kind": "raw_input", "secret_input": False,
                "authorization_category": "raw_input"} for name in ("click", "key_sequence") if capability]
    methods = ["snapshot"]
    if capability:
        methods.append("execute_action")
    if os.name == "nt":
        methods.extend(("get_window_state", "change_window_state"))
    start = time.perf_counter()
    with client(binary, os.environ.copy(), None) as mcp:
        report["initialize_ms"] = round((time.perf_counter() - start) * 1000)
        start = time.perf_counter()
        response = mcp.tool("start_task", {
            "application_label": "Disposable Blender workflow benchmark", "surface": "window",
            "observation_mode": "pixels_only", "target_process_id": pid, "target_window_handle": window,
            "allowed_methods": methods,
            "allowed_actions": actions, "ttl_minutes": 5})
        report["start_task_ms"] = round((time.perf_counter() - start) * 1000)
        started = checked_result(response, "start_task", report)
        task = started["task_id"]
        mcp.tasks.append(task)
        assert attest(started) == (pid, window)
        assert started["runtime_version"] == version
        (directory / "bound").touch()

        def call(method, params):
            begin = time.perf_counter()
            result = mcp.call(task, method, params)
            metrics = {"method": method, "roundtrip_ms": round((time.perf_counter() - begin) * 1000),
                       "image_base64_bytes": sum(len(v["data"]) for v in result.get("content", []) if v["type"] == "image")}
            report["calls"].append(metrics)
            receipt = checked_result(result, method, report)
            if receipt.get("success") is False:
                raise RuntimeError(f"{method} failed: {receipt.get('reason', 'unknown')}")
            return receipt

        if os.name == "nt":
            native_state = call("get_window_state", {})["state"]
            if not native_state["foreground"]:
                call("change_window_state", {"operation": "restore_activate"})
        snapshot = call("snapshot", {})
        observation = snapshot["observation"]
        if sys.platform == "darwin":
            report["accessibility_granted"] = observation["capture_provenance"]["accessibility_granted"]
        assert (observation["process_id"], observation["window_handle"]) == (pid, window)
        if not capability:
            raise RuntimeError("guarded_key_sequence_v1 is absent from the native runtime manifest")
        state = wait_scene(directory, lambda s: "viewport_fraction" in s)
        x, y = state["viewport_fraction"]
        click = {"action": "click", "input_kind": "raw_input", "intent": "navigate", "delivery_mode": "foreground",
                 "x": round(x * observation["width"]), "y": round(y * observation["height"])}
        result = call("execute_action", {"observation_id": snapshot["observation_id"], "action": click, "capture_after": True})
        observation_id = result["post_snapshot"]["observation_id"]
        for case, example in CASES:
            before = set(state["objects"])
            action = json.loads((ROOT / "examples" / example).read_text(encoding="utf-8"))
            begin = time.perf_counter()
            result = call("execute_action", {"observation_id": observation_id, "action": action, "capture_after": True})
            # The execution frame lease has ended by the time the ordinary
            # banner status is sampled. The native receipt covers the actual
            # dispatch interval: each completed chord passed its frame guard.
            delivery = result["result"]["delivery"]
            timing = delivery["key_sequence_timing"]
            if (result["status"] != "succeeded" or not delivery["post_dispatch_validated"]
                    or timing["steps"] != len(action["keys"])):
                raise RuntimeError(f"{case} did not complete the guarded sequence")
            state = wait_scene(directory, lambda s: verified(case, s, before))
            report["cases"].append({"case": case, "steps": len(action["keys"]), "verified": True,
                                    "native_timing": timing,
                                    "verified_ms": round((time.perf_counter() - begin) * 1000),
                                    "scene": state})
            observation_id = result["post_snapshot"]["observation_id"]
        report["metrics"] = structured(mcp.tool("dcc_cua_metrics", {}))
        mcp.stop(task)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--blender", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    start = time.perf_counter()
    report = {"platform": sys.platform, "calls": [], "cases": [], "success": False}
    try:
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            env = dict(os.environ, DCC_CUA_BLENDER_BENCH=folder, BLENDER_USER_CONFIG=str(directory / "config"))
            process = subprocess.Popen([str(args.blender.resolve()), "--factory-startup", "-p", "80", "180",
                                        "960", "540", "--python", str(ROOT / "scripts/fixtures/blender_scene_readback.py")],
                                       env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                       creationflags=NO_CONSOLE)
            try:
                window = unique_window(process, directory)
                try:
                    benchmark(args.binary.resolve(), process.pid, window, directory, report)
                except Exception:
                    scene = directory / "scene.json"
                    if scene.is_file():
                        report["last_scene"] = json.loads(scene.read_text(encoding="utf-8"))
                    raise
                report["success"] = True
            finally:
                process.terminate()
                try:
                    process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
    except Exception as error:
        report["failure"] = str(error)
        raise
    finally:
        report["workflow_wall_ms"] = round((time.perf_counter() - start) * 1000)
        args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
