"""Run the two example macros on a disposable foreground Blender window via DCC-CUA."""
import argparse
import json
import math
import os
from pathlib import Path
import re
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
        command = [str(directory / "blender-launcher"), "find", ""]
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
        if sys.platform == "darwin":
            pid_file = directory / "pid"
            if not pid_file.is_file():
                time.sleep(0.1)
                continue
            pid = int(pid_file.read_text())
            command[-1] = str(pid)
        lines = subprocess.check_output(command, text=True, timeout=5, creationflags=NO_CONSOLE).splitlines()
        if sys.platform.startswith("linux"):
            candidates = [int(parts[0], 16) for line in lines
                          if len(parts := line.split()) >= 3 and parts[2] == str(pid)]
        else:
            candidates = [int(line) for line in lines if line.isdecimal() and int(line) > 0]
        if len(candidates) == 1:
            return pid, candidates[0]
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
                    # Keep only content-free native facts in the public report.
                    native = re.search(r"guarded physical input failed: ([A-Za-z]+)", str(error.get("message", "")))
                    if native:
                        diagnostic["reason"] = native.group(1)
                    message = str(error.get("message", ""))
                    reason = message.split(": ", 1)[-1]
                    if not native and reason in {
                        "guarded native pixel input requires the connected task deadline",
                        "the exact macOS window lacks AX focus",
                        "the exact macOS foreground window must be unobscured",
                        "no exposed macOS execution frame",
                        "macOS foreground window or execution frame lost exposure",
                        "macOS execution frame or Escape monitor unavailable",
                        "macOS input requires Accessibility and Screen Recording permission",
                        "macOS application is not frontmost",
                        "macOS window does not fit its display",
                        "macOS WindowServer bounds changed",
                        "macOS execution frame has no visible band",
                        "macOS window is covered by another on-screen surface",
                        "macOS WindowServer exposure could not be verified",
                        "the exact X11 window lacks keyboard focus",
                        "the exact X11 target is occluded; pixels were discarded",
                        "X11 input geometry or instance changed",
                        "X11 target is clipped or shaped by an ancestor",
                        "the exact X11 target is outside its screen",
                        "no exposed X11 execution frame band",
                    }:
                        diagnostic["reason"] = reason
                    details = error.get("details") or {}
                    for field in ("phase", "action_attempted", "input_sent", "completion"):
                        if field in details:
                            diagnostic[field] = details[field]
                    fence = details.get("native_input_fence") or {}
                    for field in ("unobscured", "point_visible", "hit_is_target_or_child", "redaction_count"):
                        if field in fence:
                            diagnostic[field] = fence[field]
                    break
        report["tool_failure"] = diagnostic
        raise RuntimeError(f"{method} failed: {diagnostic.get('code')} ({diagnostic.get('reason')})")
    return structured(response)


def benchmark(binary, pid, window, directory, report, terminal_snapshot):
    manifest = json.loads(subprocess.check_output([binary, "manifest"], text=True, creationflags=NO_CONSOLE))
    version = manifest["version"]
    print(f"provider=dcc-cua runtime={version} pid={pid} hwnd={window}", flush=True)
    report.update(provider="dcc-cua", runtime=version, pid=pid, window_handle=window)
    capability = manifest["runtime"].get("exact_window_pixels", {}).get("key_sequence")
    actions = [{"action": name, "input_kind": "raw_input", "secret_input": False,
                "authorization_category": "raw_input"} for name in ("click", "key_sequence") if capability]
    methods = ["snapshot"]
    if os.name == "nt" or sys.platform.startswith("linux"):
        methods.append("get_window_state")
    if capability:
        methods.append("execute_action")
    if os.name == "nt":
        methods.append("change_window_state")
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
            captured = receipt.get("observation") or (receipt.get("post_snapshot") or {}).get("observation", {})
            visibility = captured.get("capture_provenance", {}).get("visibility", {})
            if "redacted_rectangles" in visibility:
                metrics["redacted_rectangles"] = visibility["redacted_rectangles"][:16]
            return receipt

        if os.name == "nt" or sys.platform.startswith("linux"):
            native_state = call("get_window_state", {})["state"]
            report["initial_window"] = {k: native_state[k] for k in ("foreground", "bounds", "visible_bounds", "minimized") if k in native_state}
            if not native_state["foreground"]:
                if os.name == "nt":
                    call("change_window_state", {"operation": "restore_activate"})
                else:
                    deadline = time.monotonic() + 5
                    while not native_state["foreground"] and time.monotonic() < deadline:
                        time.sleep(0.15)
                        native_state = call("get_window_state", {})["state"]
                    if not native_state["foreground"]:
                        raise RuntimeError("Blender did not receive X11 keyboard focus")
        snapshot = call("snapshot", {})
        observation = snapshot["observation"]
        report["capture_size"] = [observation["width"], observation["height"]]
        if sys.platform == "darwin":
            provenance = observation["capture_provenance"]
            for field in ("accessibility_granted", "exact_ax_focus", "input_exposure", "native_window_bounds"):
                report[field] = provenance.get(field)
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
            capture_after = case != CASES[-1][0] or terminal_snapshot
            timings = []
            if case == "rig":
                boundary = action["keys"].index("ALT+R")
                advance = dict(action, keys=action["keys"][:boundary])
                arrived = call("execute_action", {"observation_id": observation_id, "action": advance, "capture_after": True})
                timings.append(confirmed_sequence(arrived, advance))
                wait_scene(directory, lambda s: s["mode"] == "POSE" and s["frame"] == 9
                           and len(s.get("bones", [])) == 3 and bool(s.get("curves")))
                observation_id = arrived["post_snapshot"]["observation_id"]
                action = dict(action, keys=action["keys"][boundary:])
            result = call("execute_action", {"observation_id": observation_id, "action": action, "capture_after": capture_after})
            # The execution frame lease has ended by the time the ordinary
            # banner status is sampled. The native receipt covers the actual
            # dispatch interval: each completed chord passed its frame guard.
            timings.append(confirmed_sequence(result, action))
            state = wait_scene(directory, lambda s: verified(case, s, before))
            report["cases"].append({"case": case, "steps": sum(t["steps"] for t in timings), "verified": True,
                                    "capture_after": capture_after,
                                    "batch_timings": timings,
                                    "verified_ms": round((time.perf_counter() - begin) * 1000),
                                    "scene": state})
            if capture_after:
                observation_id = result["post_snapshot"]["observation_id"]
        report["metrics"] = structured(mcp.tool("dcc_cua_metrics", {}))
        mcp.stop(task)


def confirmed_sequence(result, action):
    delivery = result["result"]["delivery"]
    timing = delivery["key_sequence_timing"]
    if (result["status"] != "succeeded" or not delivery["post_dispatch_validated"]
            or timing["steps"] != len(action["keys"])):
        raise RuntimeError("Blender input did not complete the guarded sequence")
    return timing


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--blender", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--terminal-snapshot", action="store_true",
                        help="include the terminal screenshot when comparing image transport")
    parser.add_argument("--window-geometry", type=int, nargs=4, metavar=("X", "Y", "WIDTH", "HEIGHT"),
                        default=(40, 64, 700, 420), help="placement of the disposable Blender window")
    args = parser.parse_args()
    start = time.perf_counter()
    report = {"platform": sys.platform, "calls": [], "cases": [], "success": False}
    try:
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            env = dict(os.environ, DCC_CUA_BLENDER_BENCH=folder, BLENDER_USER_CONFIG=str(directory / "config"))
            # Leave space for the title bar, taskbar and exterior execution
            # frame even on the hosted Windows and macOS virtual displays.
            launch = [str(args.blender.resolve())]
            if sys.platform == "darwin":
                helper = directory / "blender-launcher"
                subprocess.run(["swiftc", str(ROOT / "scripts/fixtures/blender_macos.swift"), "-o", str(helper)],
                               check=True, timeout=120)
                launch = [str(helper), "launch", folder, str(args.blender.resolve().parents[2])]
            process = subprocess.Popen([*launch, "--factory-startup", "-p", *map(str, args.window_geometry),
                                        "--python", str(ROOT / "scripts/fixtures/blender_scene_readback.py")],
                                       env=env, stdin=subprocess.PIPE if sys.platform == "darwin" else subprocess.DEVNULL,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                       creationflags=NO_CONSOLE)
            try:
                pid, window = unique_window(process, directory)
                try:
                    benchmark(args.binary.resolve(), pid, window, directory, report, args.terminal_snapshot)
                except Exception:
                    scene = directory / "scene.json"
                    if scene.is_file():
                        report["last_scene"] = json.loads(scene.read_text(encoding="utf-8"))
                    raise
                report["success"] = True
            finally:
                if sys.platform == "darwin":
                    process.stdin.close()
                else:
                    process.terminate()
                try:
                    process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
                if sys.platform == "darwin" and process.returncode:
                    report["success"] = False
                    raise RuntimeError("LaunchServices fixture cleanup did not confirm exit")
    except Exception as error:
        report["failure"] = str(error)
        if error.__context__ and str(error) == "MCP task or process cleanup failed":
            report["failure"] = str(error.__context__)
            report["cleanup_failure"] = str(error)
        raise
    finally:
        report["workflow_wall_ms"] = round((time.perf_counter() - start) * 1000)
        args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
