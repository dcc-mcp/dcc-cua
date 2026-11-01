"""Real Windows MCP -> Host -> Core owned-install/uninstall GUI acceptance.

Run explicitly with --binary <candidate dcc-cua.exe> on an authorized test
desktop. Only a compiled repository fixture and its own temporary artifacts
are changed. Unit tests run with: python -m unittest scripts.test_owned_workflow_e2e
"""
import argparse
from contextlib import contextmanager
import ctypes
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
import unittest


METHODS = ["snapshot", "execute_action", "get_window_state"]
ACTIONS = [{"action": "click", "input_kind": "semantic", "secret_input": False,
            "authorization_category": category} for category in ("task_grant", "content_change")]
MAX_RESPONSE_BYTES = 16 * 1024 * 1024


def structured(response):
    assert response.get("isError") is not True, "MCP tool reported an error"
    result = response.get("structuredContent")
    assert isinstance(result, dict), "MCP tool omitted structured content"
    return result


def attest(started):
    assert started.get("provider") == "dcc-cua", "unexpected automation provider"
    version = started.get("runtime_version")
    target = started.get("target", {})
    pid, hwnd = target.get("process_id"), target.get("window_handle")
    assert isinstance(version, str) and version, "runtime version was not attested"
    assert type(pid) is int and pid > 0, "exact fixture PID was not attested"
    assert type(hwnd) is int and hwnd > 0, "exact fixture HWND was not attested"
    print(f"provider=dcc-cua runtime={version} pid={pid} hwnd={hwnd}", flush=True)
    return pid, hwnd


def button_ref(snapshot, name, target):
    """Require a unique current native token; no screen-coordinate inference."""
    assert snapshot.get("target", {}).get("process_id") == target[0], "snapshot PID changed"
    assert snapshot.get("target", {}).get("window_handle") == target[1], "snapshot HWND changed"
    observation = snapshot.get("observation", {})
    assert isinstance(snapshot.get("observation_id"), str) and snapshot["observation_id"], "fresh observation missing"
    assert observation.get("observation_id") == snapshot.get("observation_id"), "observation fence missing"
    assert observation.get("process_id") == target[0] and observation.get("window_handle") == target[1]
    elements = snapshot.get("root", {}).get("elements", [])
    candidates = [element for element in elements
                  if element.get("label") == name and element.get("enabled") is True
                  and element.get("offscreen") is not True
                  and str(element.get("role", "")).lower() in {"button", "pushbutton"}]
    assert len(candidates) == 1, "fixture button missing or ambiguous"
    token = candidates[0].get("element_token")
    index = candidates[0].get("element_index")
    assert isinstance(token, str) and 0 < len(token) <= 512, "fresh native button token missing"
    assert type(index) is int and index >= 0, "fresh native button index missing"
    return {"element_token": token, "element_index": index}


class OwnedFixtureProcess:
    """A retained native handle lets failed-test cleanup avoid PID reuse."""
    def __init__(self, pid, expected_executable):
        from ctypes import wintypes
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        self.kernel.OpenProcess.restype = wintypes.HANDLE
        self.kernel.QueryFullProcessImageNameW.argtypes = [wintypes.HANDLE, wintypes.DWORD,
                                                         wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)]
        self.kernel.QueryFullProcessImageNameW.restype = wintypes.BOOL
        self.kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        self.kernel.WaitForSingleObject.restype = wintypes.DWORD
        self.kernel.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
        self.kernel.TerminateProcess.restype = wintypes.BOOL
        self.kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        self.kernel.CloseHandle.restype = wintypes.BOOL
        self.handle = self.kernel.OpenProcess(0x00101001, False, pid)
        assert self.handle, "could not retain exact synthetic fixture process"
        image = ctypes.create_unicode_buffer(32768)
        length = wintypes.DWORD(len(image))
        if not self.kernel.QueryFullProcessImageNameW(self.handle, 0, image, ctypes.byref(length)) \
                or Path(image.value).resolve() != expected_executable.resolve():
            self.kernel.CloseHandle(self.handle)
            self.handle = None
            raise AssertionError("owned process does not match the compiled synthetic fixture")

    def close(self):
        try:
            exited = self.kernel.WaitForSingleObject(self.handle, 1000) == 0
            if not exited:
                # Only the verified fixture's retained handle is eligible for emergency cleanup.
                assert self.kernel.TerminateProcess(self.handle, 91), "synthetic fixture cleanup failed"
                assert self.kernel.WaitForSingleObject(self.handle, 5000) == 0, "synthetic fixture was not reaped"
            return exited
        finally:
            self.kernel.CloseHandle(self.handle)
            self.handle = None


class Mcp:
    def __init__(self, binary, environment, expected_fixture):
        self.process = subprocess.Popen([str(binary), "mcp-server"], env=environment,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True, encoding="utf-8",
                                        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        self.replies = queue.Queue(maxsize=8)
        self.sequence = 0
        self.tasks = []
        self.expected_fixture = expected_fixture
        self.owned_processes = []
        self.stderr = bytearray()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        self.error_reader = threading.Thread(target=self._read_errors, daemon=True)
        self.error_reader.start()
        try:
            self.rpc("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                   "clientInfo": {"name": "owned-workflow-fixture", "version": "1"}})
        except BaseException:
            if self.process.poll() is None:
                self.process.kill()
            self.process.wait(timeout=5)
            self.process.stdin.close()
            self.reader.join(timeout=2)
            self.error_reader.join(timeout=2)
            self.process.stdout.close()
            self.process.stderr.close()
            raise

    def _read(self):
        try:
            while True:
                line = self.process.stdout.readline(MAX_RESPONSE_BYTES + 1)
                if not line:
                    self.replies.put(None)
                    return
                if len(line.encode("utf-8")) > MAX_RESPONSE_BYTES or not line.endswith("\n"):
                    raise AssertionError("MCP response exceeded bounded frame")
                self.replies.put(json.loads(line))
        except (ValueError, OSError, AssertionError) as error:
            self.replies.put(error)

    def _read_errors(self):
        value = self.process.stderr.read(1024 * 1024 + 1)
        self.stderr.extend(value.encode("utf-8"))

    def rpc(self, method, params):
        self.sequence += 1
        request = {"jsonrpc": "2.0", "id": self.sequence, "method": method, "params": params}
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()
        try:
            reply = self.replies.get(timeout=45)
        except queue.Empty as error:
            raise AssertionError("MCP response timed out") from error
        assert isinstance(reply, dict), "MCP response stream failed"
        assert reply.get("id") == self.sequence and "error" not in reply, "MCP RPC failed"
        return reply["result"]

    def tool(self, name, arguments):
        return self.rpc("tools/call", {"name": name, "arguments": arguments})

    def call(self, task, method, params=None):
        return self.tool("dcc_cua_task_call", {"task_id": task, "method": method,
                                               "params": params or {}})

    def start(self, recipe):
        result = structured(self.tool("start_task", {
            "application_label": "disposable software fixture", "surface": "window",
            "workflow_recipe_id": recipe, "allowed_methods": METHODS,
            "allowed_actions": ACTIONS, "ttl_minutes": 1}))
        task = result["task_id"]
        self.tasks.append(task)
        target = attest(result)
        self.owned_processes.append(OwnedFixtureProcess(target[0], self.expected_fixture))
        return task, result, target

    def click(self, task, target, name, *, progress_complete=False, lifecycle_transition=False):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            snapshot = structured(self.call(task, "snapshot", {"max_depth": 12, "max_nodes": 128}))
            candidates = [element for element in snapshot.get("root", {}).get("elements", [])
                          if element.get("label") == name and element.get("enabled") is True
                          and element.get("offscreen") is not True]
            if candidates:
                reference = button_ref(snapshot, name, target)
                break
            time.sleep(0.05)
        else:
            raise AssertionError("fresh fixture button did not appear before deadline")
        if progress_complete:
            elements = snapshot.get("root", {}).get("elements", [])
            assert any(element.get("label") == "Progress: 100%" for element in elements), "native progress state was not verified"
            assert any(element.get("label") == "fixture-progress"
                       and str(element.get("role", "")).lower() == "progressbar" for element in elements), "native progress control missing"
        response = self.call(task, "execute_action", {
            "observation_id": snapshot["observation_id"],
            "accessibility_state_id": snapshot["accessibility_state_id"],
            "action": {"action": "click", "input_kind": "semantic", "intent": "ordinary_edit",
                       **reference}, "capture_after": False})
        if lifecycle_transition:
            # Closing this exact HWND may invalidate action completion evidence.
            # Send once, then prove the new HWND or exit+artifact; never retry input.
            assert isinstance(response, dict), "fixture lifecycle action response missing"
        else:
            assert structured(response).get("success") is True, "fixture input failed"

    def stop(self, task):
        result = structured(self.tool("stop_task", {"task_id": task}))
        assert result.get("status") == "stopped", "task cleanup did not finish"
        self.tasks.remove(task)
        return result

    def close(self):
        failed = False
        code = -1
        for task in list(self.tasks):
            try:
                self.stop(task)
            except (AssertionError, OSError):
                failed = True
        try:
            try:
                self.process.stdin.close()
            except OSError:
                failed = True
            try:
                code = self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
                code, failed = -1, True
            self.reader.join(timeout=2)
            self.error_reader.join(timeout=2)
            self.process.stdout.close()
            self.process.stderr.close()
        finally:
            for process in self.owned_processes:
                try:
                    failed = not process.close() or failed
                except (AssertionError, OSError):
                    failed = True
        assert not failed and code == 0, "MCP task or process cleanup failed"


@contextmanager
def client(binary, environment, fixture):
    value = Mcp(binary, environment, fixture)
    try:
        yield value
    finally:
        value.close()


def build_fixture(root):
    compiler = Path(os.environ.get("WINDIR", r"C:\Windows")) / "Microsoft.NET/Framework64/v4.0.30319/csc.exe"
    assert compiler.is_file(), "system C# compiler required for disposable GUI fixture"
    source = Path(__file__).with_name("fixtures") / "owned_workflow_fixture.cs"
    binary = root / "OwnedWorkflowFixture.exe"
    result = subprocess.run([str(compiler), "/nologo", "/target:winexe", "/optimize+", "/warnaserror+",
                             "/r:System.Windows.Forms.dll", "/r:System.Drawing.dll",
                             "/out:" + str(binary), str(source)], capture_output=True, timeout=30)
    assert result.returncode == 0 and binary.is_file(), "disposable GUI fixture compilation failed"
    return binary


def recipe(root, executable, identifier, mode, *, timeout_ms=30000):
    present = mode != "uninstall"
    result = {"relative_path": "installed/fixture.txt", "present": present}
    if present:
        result["sha256"] = hashlib.sha256(b"synthetic-installed-v1").hexdigest()
    return {"id": identifier, "kind": "uninstall" if mode == "uninstall" else "install",
            "executable": str(executable), "executable_sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
            "arguments": [str(root), mode], "working_directory": str(root), "artifact_root": str(root),
            "result": result, "allowed_exit_codes": [0], "timeout_ms": timeout_ms,
            "allow_cancel": True, "gui": True, "allowed_methods": METHODS, "allowed_actions": ACTIONS}


def workflow_status(client_, task):
    value = structured(client_.tool("task_status", {"task_id": task}))
    result = value.get("workflow")
    assert isinstance(result, dict), "task omitted owned workflow result"
    return result


def wait_result(client_, task, expected, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = workflow_status(client_, task)
        if result.get("state") not in {"running", "verifying"}:
            assert result.get("state") == expected, "unexpected verified workflow outcome"
            assert result.get("process_exited") is True, "workflow outcome lacks observed process exit"
            if expected == "succeeded":
                assert result.get("result_verified") is True, "success lacks independent result verification"
            return result
        time.sleep(0.05)
    raise AssertionError("verified workflow outcome timed out")


def finish_replaced_window(client_, task, original):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        rebound = structured(client_.call(task, "workflow_rebind"))
        identity = rebound.get("target", {})
        assert identity.get("process_id") == original[0], "fixture process changed during window transition"
        assert rebound.get("task_id") == task, "workflow task identity changed"
        if identity.get("window_handle") != original[1]:
            target = attest(rebound)
            break
        time.sleep(0.05)
    else:
        raise AssertionError("fixture HWND did not change before transition deadline")
    client_.click(task, target, "fixture-finish", lifecycle_transition=True)


def run_acceptance(binary, fixture_parent=None):
    assert os.name == "nt", "owned executable pinning acceptance currently requires Windows"
    if fixture_parent is not None:
        fixture_parent = Path(fixture_parent).resolve(strict=True)
        assert fixture_parent.is_dir(), "fixture parent must be an existing directory"
    cases = []
    with tempfile.TemporaryDirectory(prefix="dcc-cua-owned-workflow-", dir=fixture_parent) as directory:
        root = Path(directory).resolve()
        (root / ".fixture-root").write_text("dcc-cua-owned-workflow-v1", encoding="utf-8")
        fixture = build_fixture(root)
        environment_id = "disposable-windows-fixture"
        session_id = "fixture-" + os.urandom(16).hex()
        policy = {"version": 1, "environment_id": environment_id, "session_id": session_id,
                  "expires_at_unix_ms": int(time.time() * 1000) + 240000,
                  "recipes": [recipe(root, fixture, "install", "install"),
                              recipe(root, fixture, "uninstall", "uninstall"),
                              recipe(root, fixture, "failure", "fail"),
                              recipe(root, fixture, "timeout", "hold", timeout_ms=12000),
                              recipe(root, fixture, "cancel", "hold")]}
        policy_path = root / "host-policy.json"
        policy_path.write_text(json.dumps(policy), encoding="utf-8")
        environment = dict(os.environ, DCC_CUA_WORKFLOW_POLICY=str(policy_path),
                           DCC_CUA_ENVIRONMENT_ID=environment_id, DCC_CUA_AGENT_HOST_SESSION_ID=session_id)
        with client(binary, environment, fixture) as runtime:
            task, _, target = runtime.start("install")
            runtime.click(task, target, "fixture-primary")
            runtime.click(task, target, "fixture-next", progress_complete=True, lifecycle_transition=True)
            finish_replaced_window(runtime, task, target)
            result = wait_result(runtime, task, "succeeded")
            assert result.get("exit_code") == 0 and (root / "installed/fixture.txt").read_bytes() == b"synthetic-installed-v1"
            runtime.stop(task)
            cases.append("install_gui_exit_and_artifact")
            task, _, target = runtime.start("uninstall")
            runtime.click(task, target, "fixture-primary")
            runtime.click(task, target, "fixture-next", progress_complete=True, lifecycle_transition=True)
            finish_replaced_window(runtime, task, target)
            result = wait_result(runtime, task, "succeeded")
            assert result.get("exit_code") == 0 and not (root / "installed/fixture.txt").exists()
            runtime.stop(task)
            cases.append("normal_uninstall_gui_exit_and_absence")
            denied = runtime.tool("start_task", {"application_label": "fixture", "surface": "window",
                "workflow_recipe_id": "uninstall", "allowed_methods": METHODS, "allowed_actions": ACTIONS})
            assert denied.get("isError") is True, "consumed uninstaller recipe was replayed"
            cases.append("uninstaller_one_use")
            task, _, target = runtime.start("failure")
            runtime.click(task, target, "fixture-primary", lifecycle_transition=True)
            assert wait_result(runtime, task, "failed").get("exit_code") == 23
            runtime.stop(task)
            cases.append("failure_exit_not_success")
            task, _, _ = runtime.start("timeout")
            wait_result(runtime, task, "timed_out")
            runtime.stop(task)
            cases.append("deadline_without_polling_input")
            task, _, _ = runtime.start("cancel")
            runtime.stop(task)
            wait_result(runtime, task, "cancelled")
            cases.append("cancel_reaps_exact_owned_child")
        events = (root / "ui-events.txt").read_text(encoding="utf-8").splitlines()
        assert events.count("progress_complete") == 2 and events.count("window_replaced") == 2 and events.count("finish_clicked") == 2
    print(json.dumps({"passed": True, "cases": cases, "scope": "disposable_fixture_only"}))


class ContractTests(unittest.TestCase):
    def snapshot(self):
        return {"target": {"process_id": 42, "window_handle": 77}, "observation_id": "fresh",
                "observation": {"observation_id": "fresh", "process_id": 42, "window_handle": 77,
                                "source_rect": [100, 200, 460, 240], "width": 460, "height": 240},
                "root": {"elements": [{"label": "fixture-primary", "role": "button", "enabled": True,
                    "element_index": 7, "element_token": "native-fresh-token"}]}}

    def test_fresh_exact_button_token_is_required(self):
        self.assertEqual(button_ref(self.snapshot(), "fixture-primary", (42, 77)),
                         {"element_index": 7, "element_token": "native-fresh-token"})

    def test_foreign_target_stale_observation_and_duplicate_button_fail_closed(self):
        for change in ("pid", "hwnd", "stale", "duplicate", "no_token"):
            snapshot = self.snapshot()
            if change == "pid": snapshot["target"]["process_id"] = 99
            elif change == "hwnd": snapshot["target"]["window_handle"] = 99
            elif change == "stale": snapshot["observation"]["observation_id"] = "old"
            elif change == "duplicate": snapshot["root"]["elements"] *= 2
            else: del snapshot["root"]["elements"][0]["element_token"]
            with self.subTest(change=change), self.assertRaises(AssertionError):
                button_ref(snapshot, "fixture-primary", (42, 77))

    def test_provider_and_boolean_identity_are_refused(self):
        for provider, pid in (("other", 42), ("dcc-cua", True)):
            with self.subTest(provider=provider, pid=pid), self.assertRaises(AssertionError):
                attest({"provider": provider, "runtime_version": "test", "target": {"process_id": pid, "window_handle": 77}})

    def test_error_content_cannot_be_treated_as_workflow_success(self):
        with self.assertRaises(AssertionError):
            structured({"isError": True, "structuredContent": {"status": "succeeded"}})

    def test_observed_coordinates_do_not_substitute_for_missing_native_token(self):
        snapshot = self.snapshot()
        del snapshot["root"]["elements"][0]["element_token"]
        snapshot["root"]["elements"][0]["frame"] = {"x": 124, "y": 270, "w": 180, "h": 40}
        with self.assertRaises(AssertionError):
            button_ref(snapshot, "fixture-primary", (42, 77))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--fixture-parent", type=Path,
                        help="existing authorized directory for disposable fixture files")
    options = parser.parse_args()
    try:
        run_acceptance(Path(options.binary).resolve(strict=True), options.fixture_parent)
    except (AssertionError, OSError, subprocess.SubprocessError, KeyError, ValueError) as error:
        reason = str(error) if isinstance(error, AssertionError) else type(error).__name__
        print(json.dumps({"passed": False, "error": "owned_workflow_acceptance_failed", "reason": reason}), file=sys.stderr)
        raise SystemExit(1)
