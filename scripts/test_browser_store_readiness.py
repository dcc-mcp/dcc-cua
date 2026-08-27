from __future__ import annotations

import copy
import contextlib
import importlib.util
import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location(
    "browser_store_readiness", ROOT / "scripts" / "browser_store_readiness.py"
)
assert SPEC and SPEC.loader
READINESS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(READINESS)
SCRIPT = ROOT / "scripts" / "browser_store_readiness.py"


def ready_snapshot() -> dict[str, object]:
    configuration = {name: True for name in READINESS.EXPECTED_CONFIGURATION_NAMES}
    return {
        "configuration": configuration,
        "environment": {
            "name": "browser-stores",
            "deployment_branch_policy": {
                "protected_branches": True,
                "custom_branch_policies": False,
            },
            "default_branch": "main",
            "default_branch_protected": True,
        },
        "github": {
            "repository": {"id": 123, "full_name": "dcc-mcp/dcc-cua"},
            "source_sha": "a" * 40,
            "tag": "dcc-cua-browser-extension-v1.2.3",
            "tag_target_sha": "a" * 40,
            "release_id": 456,
            "release_target_sha": "a" * 40,
            "artifact": {
                "id": 789,
                "name": "dcc-cua-browser-extension",
                "digest": "sha256:" + "b" * 64,
                "expired": False,
                "expires_at": "2099-01-01T00:00:00Z",
                "workflow_run": {
                    "id": 1011,
                    "head_sha": "a" * 40,
                    "repository_id": 123,
                    "head_repository_id": 123,
                },
            },
        },
        "action_pins": {"valid": True, "unpinned": []},
        "stores": {
            name: {
                "permission": "granted",
                "item": "exists",
                "version": "1.2.3",
                "state": "published",
            }
            for name in ("chrome", "edge", "firefox")
        },
    }


class BrowserStoreReadinessTests(unittest.TestCase):
    def assert_failure_receipt(self, serialized: str) -> dict[str, object]:
        self.assertEqual(1, serialized.count("\n"))
        receipt = json.loads(serialized)
        self.assertFalse(receipt["ready"])
        self.assertEqual("not_ready", receipt["overall_state"])
        self.assertEqual("preflight_failed", receipt["terminal_reason"])
        return receipt

    def run_cli(self, *arguments: object) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, "-B", str(SCRIPT), *(str(value) for value in arguments)],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_missing_names_are_stable_and_never_echo_values(self) -> None:
        snapshot = ready_snapshot()
        snapshot["configuration"]["EDGE_ADDONS_API_KEY"] = False
        receipt = READINESS.build_receipt(snapshot)
        self.assertFalse(receipt["ready"])
        self.assertEqual("not_ready", receipt["overall_state"])
        self.assertIn("EDGE_ADDONS_API_KEY", receipt["platforms"]["edge"]["missing_configuration"])
        self.assertNotIn("configured-secret", json.dumps(receipt, sort_keys=True))

    def test_protected_only_environment_rejects_unprotected_main(self) -> None:
        snapshot = ready_snapshot()
        snapshot["environment"]["default_branch_protected"] = False
        receipt = READINESS.build_receipt(snapshot)
        self.assertEqual("not_ready", receipt["overall_state"])
        self.assertEqual("default_branch_not_eligible", receipt["environment"]["reason"])

    def test_missing_or_malformed_environment_policy_fails_closed(self) -> None:
        mutations = (
            lambda environment: environment.pop("deployment_branch_policy"),
            lambda environment: environment.__setitem__("deployment_branch_policy", "invalid"),
            lambda environment: environment.__setitem__(
                "deployment_branch_policy", {"protected_branches": False}
            ),
        )
        expected_reasons = (
            "environment_policy_missing",
            "environment_policy_invalid",
            "environment_policy_invalid",
        )
        for mutate, expected_reason in zip(mutations, expected_reasons, strict=True):
            with self.subTest(expected_reason=expected_reason):
                snapshot = ready_snapshot()
                snapshot["environment"]["default_branch_protected"] = False
                mutate(snapshot["environment"])
                receipt = READINESS.build_receipt(snapshot)
                self.assertFalse(receipt["ready"])
                self.assertEqual("not_ready", receipt["overall_state"])
                self.assertFalse(receipt["environment"]["valid"])
                self.assertEqual(expected_reason, receipt["environment"]["reason"])

    def test_no_item_requires_human_action(self) -> None:
        snapshot = ready_snapshot()
        snapshot["stores"]["chrome"]["item"] = "missing"
        receipt = READINESS.build_receipt(snapshot)
        self.assertEqual("human_action_required", receipt["platforms"]["chrome"]["state"])

    def test_unknown_store_state_fails_closed(self) -> None:
        snapshot = ready_snapshot()
        snapshot["stores"]["firefox"]["state"] = "provider-new-state"
        receipt = READINESS.build_receipt(snapshot)
        self.assertEqual("not_ready", receipt["platforms"]["firefox"]["state"])
        self.assertEqual("unknown_item_state", receipt["platforms"]["firefox"]["reason"])

    def test_expired_or_mismatched_artifact_is_not_ready(self) -> None:
        for mutation in ("expired", "repository"):
            with self.subTest(mutation=mutation):
                snapshot = ready_snapshot()
                if mutation == "expired":
                    snapshot["github"]["artifact"]["expired"] = True
                else:
                    snapshot["github"]["artifact"]["workflow_run"]["repository_id"] = 999
                receipt = READINESS.build_receipt(snapshot)
                self.assertFalse(receipt["github_identity"]["valid"])
                self.assertEqual("not_ready", receipt["overall_state"])

    def test_source_tag_release_artifact_head_drift_fails_closed(self) -> None:
        snapshot = ready_snapshot()
        snapshot["github"]["artifact"]["workflow_run"]["head_sha"] = "c" * 40
        receipt = READINESS.build_receipt(snapshot)
        self.assertFalse(receipt["github_identity"]["valid"])
        self.assertIn("source_head_drift", receipt["github_identity"]["reasons"])

    def test_api_permission_denial_is_not_ready(self) -> None:
        snapshot = ready_snapshot()
        snapshot["stores"]["edge"]["permission"] = "denied"
        receipt = READINESS.build_receipt(snapshot)
        self.assertEqual("not_ready", receipt["platforms"]["edge"]["state"])
        self.assertEqual("read_permission_denied", receipt["platforms"]["edge"]["reason"])

    def test_receipt_is_stable_and_redacts_untrusted_provider_fields(self) -> None:
        snapshot = ready_snapshot()
        snapshot["stores"]["chrome"]["provider_message"] = "token=never-print-this"
        first = READINESS.serialize_receipt(READINESS.build_receipt(snapshot))
        second = READINESS.serialize_receipt(READINESS.build_receipt(copy.deepcopy(snapshot)))
        self.assertEqual(first, second)
        self.assertNotIn("never-print-this", first)

    def test_cli_failure_emits_one_stable_redacted_terminal_receipt(self) -> None:
        sensitive = "token-never-print P:\\private\\checkout\\snapshot.json"
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "receipt.json"
            stdout = io.StringIO()
            stderr = io.StringIO()
            argv = [
                "browser_store_readiness.py",
                "--repository",
                "dcc-mcp/dcc-cua",
                "--output",
                str(output),
            ]
            with (
                mock.patch.object(sys, "argv", argv),
                mock.patch.object(
                    READINESS,
                    "collect_github_snapshot",
                    side_effect=READINESS.ReadinessError(sensitive),
                ),
                contextlib.redirect_stdout(stdout),
                contextlib.redirect_stderr(stderr),
            ):
                exit_code = READINESS.main()

            terminal = stdout.getvalue()
            self.assertEqual(1, exit_code)
            self.assertEqual(1, terminal.count("\n"))
            self.assertEqual(terminal, output.read_text(encoding="utf-8"))
            receipt = json.loads(terminal)
            self.assertFalse(receipt["ready"])
            self.assertEqual("not_ready", receipt["overall_state"])
            self.assertEqual("preflight_failed", receipt["terminal_reason"])
            combined = terminal + stderr.getvalue()
            self.assertNotIn("token-never-print", combined)
            self.assertNotIn("private", combined)
            self.assertNotIn("Traceback", combined)
            self.assertEqual("browser store readiness preflight failed\n", stderr.getvalue())

    def test_malformed_snapshot_still_writes_terminal_receipt(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            snapshot = root / "malformed.json"
            output = root / "receipt.json"
            snapshot.write_text('{"secret": "do-not-print"', encoding="utf-8")
            stdout = io.StringIO()
            stderr = io.StringIO()
            argv = [
                "browser_store_readiness.py",
                "--snapshot",
                str(snapshot),
                "--output",
                str(output),
            ]
            with (
                mock.patch.object(sys, "argv", argv),
                contextlib.redirect_stdout(stdout),
                contextlib.redirect_stderr(stderr),
            ):
                exit_code = READINESS.main()
            self.assertEqual(1, exit_code)
            self.assertEqual(stdout.getvalue(), output.read_text(encoding="utf-8"))
            self.assertNotIn("do-not-print", stdout.getvalue() + stderr.getvalue())
            self.assertNotIn(str(snapshot), stdout.getvalue() + stderr.getvalue())

    def test_subprocess_failures_keep_receipt_sinks_isolated_and_redacted(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            malformed = root / "malformed-secret-name.json"
            malformed.write_text('{"token": "never-print"', encoding="utf-8")
            unreadable = root / "snapshot-is-a-directory"
            unreadable.mkdir()

            cases = {
                "invalid_cli": (
                    ("--output", root / "invalid-cli.json", "--definitely-invalid"),
                    root / "invalid-cli.json",
                    True,
                ),
                "malformed_input": (
                    ("--snapshot", malformed, "--output", root / "malformed.json"),
                    root / "malformed.json",
                    True,
                ),
                "read_failure": (
                    ("--snapshot", unreadable, "--output", root / "read-failure.json"),
                    root / "read-failure.json",
                    True,
                ),
                "unwritable_output": (
                    ("--snapshot", malformed, "--output", unreadable),
                    unreadable,
                    False,
                ),
            }
            for name, (arguments, output, output_is_file) in cases.items():
                with self.subTest(name=name):
                    result = self.run_cli(*arguments)
                    self.assertEqual(1, result.returncode)
                    self.assert_failure_receipt(result.stdout)
                    self.assertEqual(
                        "browser store readiness preflight failed\n", result.stderr
                    )
                    combined = result.stdout + result.stderr
                    self.assertNotIn("never-print", combined)
                    self.assertNotIn(str(root), combined)
                    self.assertNotIn("Traceback", combined)
                    if output_is_file:
                        self.assertEqual(result.stdout, output.read_text(encoding="utf-8"))

    def test_broken_stdout_does_not_escape_or_remove_requested_receipt(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "broken-stdout.json"
            bootstrap = f"""
import importlib.util
import pathlib
import sys

spec = importlib.util.spec_from_file_location("readiness", {str(SCRIPT)!r})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class BrokenStdout:
    def write(self, value):
        raise BrokenPipeError("sensitive broken sink")
    def flush(self):
        return None

sys.stdout = BrokenStdout()
sys.argv = ["browser_store_readiness.py", "--output", {str(output)!r}, "--definitely-invalid"]
raise SystemExit(module.main())
"""
            result = subprocess.run(
                [sys.executable, "-B", "-c", bootstrap],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(1, result.returncode)
            self.assertEqual("", result.stdout)
            self.assertEqual("browser store readiness preflight failed\n", result.stderr)
            receipt_text = output.read_text(encoding="utf-8")
            self.assert_failure_receipt(receipt_text)
            self.assertNotIn("Traceback", result.stderr)

    def test_action_pins_fail_closed(self) -> None:
        snapshot = ready_snapshot()
        snapshot["action_pins"] = {"valid": False, "unpinned": ["actions/checkout@v7"]}
        receipt = READINESS.build_receipt(snapshot)
        self.assertEqual("not_ready", receipt["overall_state"])
        self.assertFalse(receipt["action_pins"]["valid"])
        self.assertEqual(1, receipt["action_pins"]["unpinned_count"])

    def test_all_three_ready_dry_readback_is_ready_but_publish_remains_disabled(self) -> None:
        receipt = READINESS.build_receipt(ready_snapshot())
        self.assertTrue(receipt["ready"])
        self.assertEqual("ready", receipt["overall_state"])
        self.assertFalse(receipt["publishing_enabled"])
        self.assertTrue(all(item["ready"] for item in receipt["platforms"].values()))

    def test_manual_workflow_is_get_only_sha_pinned_and_never_runs_on_push_or_pr(self) -> None:
        workflow_path = ROOT / ".github" / "workflows" / "browser-store-preflight.yml"
        workflow = workflow_path.read_text(encoding="utf-8")
        self.assertIn("workflow_dispatch:", workflow)
        self.assertNotIn("pull_request:", workflow)
        self.assertNotIn("\n  push:", workflow)
        self.assertIn("chromewebstore.readonly", workflow)
        self.assertEqual(
            {"valid": True, "unpinned": []},
            READINESS.audit_action_pins([workflow_path]),
        )
        self.assertLess(
            workflow.index("inspect-github-readiness:"),
            workflow.index("environment: browser-stores"),
        )
        for name in READINESS.EXPECTED_CONFIGURATION_NAMES:
            self.assertIn(name, workflow)

    def test_required_ci_executes_the_exact_receipt_contract_module(self) -> None:
        workflow = (ROOT / ".github" / "workflows" / "ci-checks.yml").read_text(
            encoding="utf-8"
        )
        self.assertEqual(
            {"valid": True, "reasons": []},
            READINESS.audit_required_ci_contract(workflow),
        )
        self.assertTrue((ROOT / "scripts" / "test_browser_store_readiness.py").is_file())

    def test_required_ci_contract_rejects_disabled_or_decoyed_steps(self) -> None:
        workflow = (ROOT / ".github" / "workflows" / "ci-checks.yml").read_text(
            encoding="utf-8"
        )
        command = "python -B -m unittest scripts.test_browser_store_readiness"
        step = f"      - run: {command}"
        mutations = {
            "job_if_false": workflow.replace(
                "  policy:\n", "  policy:\n    if: false\n", 1
            ),
            "step_if_false": workflow.replace(step, step + "\n        if: false", 1),
            "continue_on_error": workflow.replace(
                step, step + "\n        continue-on-error: true", 1
            ),
            "comment_decoy": workflow.replace(step, "      # " + step.strip(), 1),
            "scalar_decoy": workflow.replace(step, f"      - name: {command}", 1),
            "multiline_decoy": workflow.replace(
                step, f"      - run: |\n          echo {command}", 1
            ),
            "disabled_unused_job": workflow.replace(step + "\n", "", 1).replace(
                "\n  verify:",
                "\n  disabled-receipt-contract:\n"
                "    if: false\n"
                "    runs-on: ubuntu-latest\n"
                "    steps:\n"
                f"      - run: {command}\n\n"
                "  verify:",
                1,
            ),
        }
        for name, mutation in mutations.items():
            with self.subTest(name=name):
                audit = READINESS.audit_required_ci_contract(mutation)
                self.assertFalse(audit["valid"])
                self.assertTrue(audit["reasons"])

    def test_edge_probe_never_mutates_to_test_access(self) -> None:
        observation = READINESS.probe_edge(
            {
                "EDGE_ADDONS_API_KEY": "redacted",
                "EDGE_ADDONS_CLIENT_ID": "configured",
                "EDGE_ADDONS_PRODUCT_ID": "configured",
            }
        )
        self.assertEqual("unverifiable", observation["permission"])
        self.assertEqual("unknown", observation["item"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
