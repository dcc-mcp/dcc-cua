"""Measure owned MCP protocol processes or summarize already-authorized native traces.

No tool calls, target discovery, screenshots, or input are performed by this runner.
"""
from __future__ import annotations

import argparse
from collections import defaultdict
import hashlib
import json
import math
from pathlib import Path
import platform
import queue
import re
import subprocess
import threading
import time
import uuid


SCHEMA = "dcc-cua.latency.v1"
STAGES = ("startup", "queue", "capture", "uia", "action", "verification", "end_to_end", "cleanup")


def percentile(values, quantile):
    """Nearest rank: keep the original observations, including tail outliers."""
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * quantile) - 1)]


def summarize(report):
    if report.get("schema") != SCHEMA:
        raise ValueError("unsupported latency schema")
    boundary = report.get("boundary")
    if boundary not in ("mcp_protocol", "native_task"):
        raise ValueError("boundary must be mcp_protocol or native_task")
    groups = defaultdict(list)
    for sample in report["samples"]:
        phase, stage, outcome = (sample.get(key) for key in ("phase", "stage", "outcome"))
        if phase not in ("cold", "warm") or stage not in STAGES:
            raise ValueError("invalid phase or stage")
        if outcome not in ("success", "error", "timeout"):
            raise ValueError("invalid outcome")
        duration = sample.get("duration_ms")
        if isinstance(duration, bool) or not isinstance(duration, (int, float)) or not math.isfinite(duration) or duration < 0:
            raise ValueError("duration_ms must be finite and nonnegative")
        if boundary == "native_task":
            binding = sample.get("binding", {})
            if not isinstance(binding, dict):
                raise ValueError("binding must be an object")
            if not isinstance(sample.get("scenario"), str) or not sample["scenario"].strip():
                raise ValueError("native sample requires a scenario")
            if not isinstance(sample.get("trial_id"), str) or not sample["trial_id"].strip():
                raise ValueError("native sample requires a trial_id")
            for key in ("runtime", "pid", "process_birth", "hwnd", "observation_mode", "width", "height"):
                if key not in binding or binding[key] is None:
                    raise ValueError(f"native sample requires binding.{key}")
            if binding.get("provider") != "dcc-cua":
                raise ValueError("native sample requires provider=dcc-cua")
            for key in ("pid", "hwnd", "width", "height"):
                if type(binding[key]) is not int or binding[key] <= 0:
                    raise ValueError(f"native binding.{key} must be a positive integer")
            for key in ("runtime", "process_birth", "observation_mode"):
                if not isinstance(binding[key], str) or not binding[key].strip():
                    raise ValueError(f"native binding.{key} must be nonempty")
            for key, length in (("source_revision", 40), ("binary_sha256", 64)):
                if not re.fullmatch(f"[0-9a-f]{{{length}}}", str(binding.get(key, ""))):
                    raise ValueError(f"native binding.{key} requires a full lowercase hash")
        else:
            binding = {}
            if stage not in ("startup", "end_to_end", "cleanup"):
                raise ValueError("protocol probe cannot measure native stages")
        # Never pool different targets, dimensions, runtime versions or modes.
        key = (phase, stage, sample.get("scenario", "protocol"), json.dumps(binding, sort_keys=True))
        groups[key].append(sample)
    result = []
    for (phase, stage, scenario, binding), samples in sorted(groups.items()):
        successes = [s["duration_ms"] for s in samples if s["outcome"] == "success"]
        result.append({
            "phase": phase, "stage": stage, "scenario": scenario, "binding": json.loads(binding),
            "attempts": len(samples), "successes": len(successes),
            "errors": sum(s["outcome"] == "error" for s in samples),
            "timeouts": sum(s["outcome"] == "timeout" for s in samples),
            "p50_ms": percentile(successes, .50) if successes else None,
            "p95_ms": percentile(successes, .95) if successes else None,
            "max_ms": max(successes) if successes else None,
            "tail_sample_warning": len(successes) < 100,
        })
    return {
        "schema": SCHEMA, "boundary": boundary, "percentile_method": "nearest_rank",
        "groups": result,
        "unmeasured_stages": [stage for stage in STAGES if not any(s["stage"] == stage for s in report["samples"])],
        "notes": ["Percentiles include successful attempts only; inspect errors and timeouts separately.",
                  "Stage percentiles must not be summed or subtracted to infer end-to-end percentiles.",
                  "Cold means a fresh process/task, not a cold OS file cache.",
                  "Fewer than 100 successes provides sparse tail evidence."],
    }


class ProtocolProcess:
    """One owned child; EOF first, bounded fallback only for this exact child."""

    def __init__(self, command, timeout):
        self.timeout = timeout
        self.responses = queue.Queue()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.request_id = 0
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL, bufsize=0)

    def start_reader(self):
        self.reader.start()

    def _read(self):
        try:
            while line := self.process.stdout.readline(1024 * 1024 + 1):
                if len(line) > 1024 * 1024:
                    self.responses.put(ValueError("oversized protocol response"))
                    return
                self.responses.put(json.loads(line))
        except (OSError, ValueError):
            self.responses.put(ValueError("invalid protocol response"))
        finally:
            self.responses.put(EOFError("protocol output closed"))

    def request(self, method, params=None):
        self.request_id += 1
        message = {"jsonrpc": "2.0", "id": self.request_id, "method": method}
        if params is not None:
            message["params"] = params
        self.process.stdin.write((json.dumps(message) + "\n").encode())
        self.process.stdin.flush()
        try:
            response = self.responses.get(timeout=self.timeout)
        except queue.Empty:
            raise TimeoutError("protocol response timed out") from None
        if isinstance(response, Exception):
            raise response
        if not isinstance(response, dict) or response.get("jsonrpc") != "2.0" or response.get("id") != self.request_id or "error" in response or "result" not in response:
            raise ValueError("uncorrelated or failed protocol response")
        return response["result"]

    def initialized(self):
        self.process.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        self.process.stdin.flush()

    def close(self):
        started = time.perf_counter_ns()
        errors = []
        eof_exit = True
        exit_confirmed = False
        try:
            self.process.stdin.close()
        except OSError as error:
            errors.append(type(error).__name__)
        try:
            self.process.wait(timeout=self.timeout)
            exit_confirmed = True
        except subprocess.TimeoutExpired:
            eof_exit = False
            try:
                # Only the exact child owned by this trial may be terminated.
                self.process.kill()
                self.process.wait(timeout=self.timeout)
                exit_confirmed = True
            except (OSError, subprocess.TimeoutExpired) as error:
                errors.append(type(error).__name__)
        except OSError as error:
            errors.append(type(error).__name__)
        # Do not close a stream underneath an unresolved reader or block on its lock.
        if self.reader.ident is not None:
            self.reader.join(timeout=self.timeout)
        if self.reader.is_alive():
            errors.append("ReaderUnresolved")
        else:
            try:
                self.process.stdout.close()
            except OSError as error:
                errors.append(type(error).__name__)
        clean = eof_exit and exit_confirmed and self.process.returncode == 0 and not errors
        return clean, (time.perf_counter_ns() - started) / 1e6, errors, exit_confirmed


def probe(command, trials, warm_samples, timeout, expected_version):
    samples, children = [], []
    aborted = False
    for trial in range(trials):
        started = time.perf_counter_ns()
        trial_id = str(uuid.uuid4())
        child = None

        def record(phase, stage, began, outcome="success", reason=None):
            samples.append({"trial_id": trial_id, "phase": phase, "stage": stage,
                            "duration_ms": (time.perf_counter_ns() - began) / 1e6,
                            "outcome": outcome, "reason": reason})

        phase, stage = "cold", "startup"
        try:
            child = ProtocolProcess(command, timeout)
            children.append({"trial_id": trial_id, "pid": child.process.pid})
            child.start_reader()
            result = child.request("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                                 "clientInfo": {"name": "dcc-cua-latency", "version": "1"}})
            info = result.get("serverInfo") if isinstance(result, dict) else None
            if not isinstance(info, dict) or info.get("version") != expected_version:
                raise ValueError("runtime version mismatch")
            child.initialized()
            record(phase, stage, started)
            for _ in range(warm_samples):
                phase, stage = "warm", "end_to_end"
                started = time.perf_counter_ns()
                if child.request("ping") != {}:
                    raise ValueError("unexpected ping result")
                record(phase, stage, started)
        except (EOFError, ValueError, OSError, TimeoutError, RuntimeError) as error:
            record(phase, stage, started, "timeout" if isinstance(error, TimeoutError) else "error", type(error).__name__)
            if child is None:
                aborted = True
        finally:
            if child is not None:
                cleanup_started = time.perf_counter_ns()
                try:
                    clean, elapsed, errors, exit_confirmed = child.close()
                except (OSError, subprocess.TimeoutExpired) as error:
                    clean, exit_confirmed = False, False
                    elapsed = (time.perf_counter_ns() - cleanup_started) / 1e6
                    errors = [type(error).__name__]
                children[-1].update(eof_exit=clean, exit_confirmed=exit_confirmed)
                samples.append({"trial_id": trial_id, "phase": "cold", "stage": "cleanup",
                                "duration_ms": elapsed, "outcome": "success" if clean else "error",
                                "errors": errors, "exit_confirmed": exit_confirmed})
                if not exit_confirmed or errors:
                    aborted = True
        if aborted:
            break
    return {"schema": SCHEMA, "boundary": "mcp_protocol", "samples": samples, "children": children,
            "runtime_version": expected_version, "host": platform.platform(),
            "trials": trials, "run_aborted": aborted,
            "warm_samples_per_process": warm_samples,
            "measurement": "spawn-to-initialize response; warm ping round trip; EOF-to-exit",
            "excludes": ["model", "Agent Host", "native Host startup", "queue", "capture", "uia", "action", "verification"]}


def positive_int(value):
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError("must be positive")
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    measure = commands.add_parser("protocol")
    measure.add_argument("--binary", type=Path, required=True)
    measure.add_argument("--expected-sha256", required=True)
    measure.add_argument("--expected-version", required=True)
    measure.add_argument("--trials", type=positive_int, default=30)
    measure.add_argument("--warm-samples", type=positive_int, default=10)
    measure.add_argument("--timeout", type=positive_int, default=10)
    measure.add_argument("--output", type=Path, required=True)
    analyze = commands.add_parser("summarize")
    analyze.add_argument("input", type=Path)
    args = parser.parse_args()
    if args.command == "summarize":
        print(json.dumps(summarize(json.loads(args.input.read_text(encoding="utf-8"))), indent=2))
        return 0
    binary = args.binary.resolve(strict=True)
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    if digest != args.expected_sha256.lower():
        parser.error("binary SHA256 mismatch")
    # Reserve before launching any process; never overwrite another run's evidence.
    with args.output.open("x", encoding="utf-8") as output:
        # Connection diagnostics must not prune or overwrite other owners' records.
        diagnostics = args.output.with_name(args.output.name + ".connections")
        diagnostics.mkdir(mode=0o700, exist_ok=False)
        report = probe([str(binary), "mcp-server", "--diagnostics-dir", str(diagnostics.resolve())],
                       args.trials, args.warm_samples, args.timeout, args.expected_version)
        report["binary_sha256"] = digest
        report["runner_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        report["binary_unchanged"] = hashlib.sha256(binary.read_bytes()).hexdigest() == digest
        report["summary"] = summarize(report)
        json.dump(report, output, indent=2, allow_nan=False)
    print(json.dumps(report["summary"], indent=2))
    return 0 if report["binary_unchanged"] and all(s["outcome"] == "success" for s in report["samples"]) else 1


if __name__ == "__main__":
    raise SystemExit(main())
