from __future__ import annotations

import copy
import contextlib
import importlib.util
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
import urllib.error
import urllib.parse
from pathlib import Path
from types import SimpleNamespace
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
    configuration["DCC_CUA_BROWSER_STORE_PUBLISH_READY"] = "false"
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
                    "status": "completed",
                    "conclusion": "success",
                    "event": "push",
                    "path": ".github/workflows/release-please.yml",
                    "head_branch": "main",
                },
            },
        },
        "action_pins": {"valid": True, "unpinned": []},
        "stores": {
            name: {
                "permission": "granted",
                "item": "exists",
                "version": "1.2.3",
                "state": {
                    "chrome": "published",
                    "edge": "in_store",
                    "firefox": "public",
                }[name],
                **(
                    {"taken_down": False, "warned": False}
                    if name == "chrome"
                    else {}
                ),
            }
            for name in ("chrome", "edge", "firefox")
        },
    }


class BrowserStoreReadinessTests(unittest.TestCase):
    def audit_workflow(self, workflow: str) -> dict[str, object]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "repository"
            path = root / ".github" / "workflows" / "workflow.yml"
            path.parent.mkdir(parents=True)
            path.write_text(workflow, encoding="utf-8")
            action = root / ".github" / "actions" / "select-macos-toolchain"
            action.mkdir(parents=True)
            (action / "action.yml").write_text(
                "name: local action\nruns:\n  using: composite\n  steps: []\n",
                encoding="utf-8",
            )
            return READINESS.audit_action_pins([path])

    @staticmethod
    def local_action_workflow(reference: str) -> str:
        return f"""
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      - uses: {reference}
"""

    def test_adr_documents_external_identity_and_firefox_owner_boundaries(self) -> None:
        adr = (ROOT / "docs" / "adr" / "0026-read-browser-store-readiness.md").read_text(
            encoding="utf-8"
        )
        self.assertIn("positive signed 64-bit", adr)
        self.assertIn("authenticated profile identity", adr)
        self.assertIn("same account", adr)

    @staticmethod
    def github_api_fixture(path: str) -> tuple[int, object]:
        responses: dict[str, tuple[int, object]] = {
            "": (
                200,
                {
                    "id": 123,
                    "full_name": "dcc-mcp/dcc-cua",
                    "default_branch": "main",
                },
            ),
            "releases?per_page=100": (
                200,
                [
                    {
                        "id": 456,
                        "tag_name": "dcc-cua-browser-extension-v1.2.3",
                        "target_commitish": "a" * 40,
                        "draft": False,
                        "prerelease": False,
                        "published_at": "2098-01-01T00:00:00Z",
                    }
                ],
            ),
            "git/ref/tags/dcc-cua-browser-extension-v1.2.3": (
                200,
                {"object": {"sha": "a" * 40}},
            ),
            "actions/artifacts?name=dcc-cua-browser-extension&per_page=100": (
                200,
                {
                    "artifacts": [
                        {
                            "id": 789,
                            "name": "dcc-cua-browser-extension",
                            "created_at": "2098-01-01T00:00:00Z",
                            "workflow_run": {"id": 1011, "head_sha": "a" * 40},
                        }
                    ]
                },
            ),
            "actions/artifacts/789": (
                200,
                {
                    "id": 789,
                    "name": "dcc-cua-browser-extension",
                    "digest": "sha256:" + "b" * 64,
                    "expired": False,
                    "expires_at": "2099-01-01T00:00:00Z",
                    "workflow_run": {"id": 1011},
                },
            ),
            "actions/runs/1011": (
                200,
                {
                    "id": 1011,
                    "head_sha": "a" * 40,
                    "repository": {"id": 123},
                    "head_repository": {"id": 123},
                    "status": "completed",
                    "conclusion": "success",
                    "event": "push",
                    "path": ".github/workflows/release-please.yml",
                    "head_branch": "main",
                },
            ),
            "environments/browser-stores": (
                200,
                {
                    "name": "browser-stores",
                    "deployment_branch_policy": {
                        "protected_branches": True,
                        "custom_branch_policies": False,
                    },
                },
            ),
            "branches/main": (200, {"name": "main", "protected": True}),
            "environments/browser-stores/variables?per_page=100": (
                200,
                {"variables": []},
            ),
            "environments/browser-stores/secrets?per_page=100": (
                200,
                {"secrets": []},
            ),
            "actions/variables?per_page=100": (200, {"variables": []}),
        }
        return responses[path]

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

    def test_provider_specific_store_states_fail_closed(self) -> None:
        cases = (
            ("chrome", "public"),
            ("chrome", "listed"),
            ("firefox", "published"),
            ("firefox", "published_to_testers"),
        )
        for platform, state in cases:
            with self.subTest(platform=platform, state=state):
                snapshot = ready_snapshot()
                snapshot["stores"][platform]["state"] = state
                platform_receipt = READINESS.build_receipt(snapshot)["platforms"][
                    platform
                ]
                self.assertFalse(platform_receipt["ready"])
                self.assertIsNone(platform_receipt["item_state"])
                self.assertEqual("unknown_item_state", platform_receipt["reason"])

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

    def test_numeric_identities_require_exact_positive_ints(self) -> None:
        paths = (
            ("repository", "id"),
            ("release_id",),
            ("artifact", "id"),
            ("artifact", "workflow_run", "id"),
            ("artifact", "workflow_run", "repository_id"),
            ("artifact", "workflow_run", "head_repository_id"),
        )
        for path in paths:
            for value in (True, False, 0, -1, 1 << 63, 1.0, "1", [], {}):
                with self.subTest(path=path, value=value):
                    snapshot = ready_snapshot()
                    target = snapshot["github"]
                    for component in path[:-1]:
                        target = target[component]
                    target[path[-1]] = value
                    receipt = READINESS.build_receipt(snapshot)
                    self.assertFalse(receipt["github_identity"]["valid"])
                    self.assertEqual("not_ready", receipt["overall_state"])

    def test_external_identity_domain_accepts_only_signed_64_bit_positive_ints(self) -> None:
        self.assertEqual((1 << 63) - 1, READINESS.MAX_EXTERNAL_ID)
        for value in (1, (1 << 63) - 1):
            with self.subTest(valid=value):
                self.assertTrue(READINESS._external_id(value))
        for value in (True, False, 0, -1, 1 << 63, 1.0, "1", [], {}):
            with self.subTest(invalid=value):
                self.assertFalse(READINESS._external_id(value))

    def test_artifact_expired_requires_exact_false(self) -> None:
        values = (None, True, 0, 1, "false", [], {})
        for value in values:
            with self.subTest(value=value):
                snapshot = ready_snapshot()
                snapshot["github"]["artifact"]["expired"] = value
                receipt = READINESS.build_receipt(snapshot)
                self.assertFalse(receipt["github_identity"]["valid"])
                self.assertIn(
                    "invalid_artifact_expired",
                    receipt["github_identity"]["reasons"],
                )
                self.assertIsNone(receipt["github_identity"]["expired"])
        snapshot = ready_snapshot()
        snapshot["github"]["artifact"].pop("expired")
        receipt = READINESS.build_receipt(snapshot)
        self.assertFalse(receipt["github_identity"]["valid"])
        self.assertIn("invalid_artifact_expired", receipt["github_identity"]["reasons"])

    def test_source_tag_release_artifact_head_drift_fails_closed(self) -> None:
        snapshot = ready_snapshot()
        snapshot["github"]["artifact"]["workflow_run"]["head_sha"] = "c" * 40
        receipt = READINESS.build_receipt(snapshot)
        self.assertFalse(receipt["github_identity"]["valid"])
        self.assertIn("source_head_drift", receipt["github_identity"]["reasons"])

    def test_artifact_run_must_be_successful_release_push_on_main(self) -> None:
        mutations = {
            "status": "in_progress",
            "conclusion": "failure",
            "event": "pull_request",
            "path": ".github/workflows/ci-checks.yml",
            "head_branch": "feature/not-main",
        }
        for field, value in mutations.items():
            with self.subTest(field=field):
                snapshot = ready_snapshot()
                snapshot["github"]["artifact"]["workflow_run"][field] = value
                receipt = READINESS.build_receipt(snapshot)
                self.assertFalse(receipt["github_identity"]["valid"])
                self.assertIn(
                    "artifact_producer_mismatch",
                    receipt["github_identity"]["reasons"],
                )

    def test_github_snapshot_uses_contents_read_branch_endpoint(self) -> None:
        paths: list[str] = []

        def fake_get(repository: str, path: str, token: str, **kwargs: object):
            self.assertEqual("dcc-mcp/dcc-cua", repository)
            self.assertEqual("configured", token)
            paths.append(path)
            return self.github_api_fixture(path)

        with mock.patch.object(READINESS, "_github_get", side_effect=fake_get):
            snapshot = READINESS.collect_github_snapshot(
                "dcc-mcp/dcc-cua", "configured"
            )

        self.assertIn("branches/main", paths)
        self.assertNotIn("branches/main/protection", paths)
        self.assertTrue(snapshot["environment"]["default_branch_protected"])
        self.assertTrue(
            READINESS.build_receipt(snapshot)["environment"]["valid"],
            "a protected public main must remain eligible with Contents-read",
        )

    def test_collected_run_metadata_is_bound_into_receipt(self) -> None:
        for field, value in {
            "status": "queued",
            "conclusion": "failure",
            "event": "pull_request",
            "path": ".github/workflows/other.yml",
            "head_branch": "feature/not-main",
        }.items():
            with self.subTest(field=field):
                def fake_get(
                    repository: str, path: str, token: str, **kwargs: object
                ):
                    status, payload = self.github_api_fixture(path)
                    payload = copy.deepcopy(payload)
                    if path == "actions/runs/1011":
                        payload[field] = value
                    return status, payload

                with mock.patch.object(READINESS, "_github_get", side_effect=fake_get):
                    snapshot = READINESS.collect_github_snapshot(
                        "dcc-mcp/dcc-cua", "configured"
                    )
                receipt = READINESS.build_receipt(snapshot)
                self.assertFalse(receipt["github_identity"]["valid"])
                self.assertIn(
                    "artifact_producer_mismatch",
                    receipt["github_identity"]["reasons"],
                )

    def test_api_permission_denial_is_not_ready(self) -> None:
        snapshot = ready_snapshot()
        snapshot["stores"]["edge"]["permission"] = "denied"
        receipt = READINESS.build_receipt(snapshot)
        self.assertEqual("not_ready", receipt["platforms"]["edge"]["state"])
        self.assertEqual("read_permission_denied", receipt["platforms"]["edge"]["reason"])

    def test_provider_and_github_reads_reject_redirects_without_forwarding_credentials(self) -> None:
        cases = (
            (
                "provider_cross_origin",
                lambda: READINESS._request_json(
                    "https://provider.example/read",
                    headers={"Authorization": "Bearer provider-secret"},
                ),
                "https://attacker.example/collect",
                "https://provider.example/read",
            ),
            (
                "provider_downgrade",
                lambda: READINESS._request_json(
                    "https://provider.example/read",
                    headers={"Authorization": "Bearer provider-secret"},
                ),
                "http://provider.example/collect",
                "https://provider.example/read",
            ),
            (
                "github_cross_origin",
                lambda: READINESS._github_get("dcc-mcp/dcc-cua", "", "github-secret"),
                "https://attacker.example/collect",
                "https://api.github.com/repos/dcc-mcp/dcc-cua",
            ),
            (
                "github_downgrade",
                lambda: READINESS._github_get("dcc-mcp/dcc-cua", "", "github-secret"),
                "http://api.github.com/collect",
                "https://api.github.com/repos/dcc-mcp/dcc-cua",
            ),
        )
        for name, invoke, redirect_target, expected_url in cases:
            with self.subTest(name=name):
                opened: list[object] = []
                handlers: list[object] = []

                class RedirectingOpener:
                    def open(self, request: object, timeout: int):
                        opened.append(request)
                        raise urllib.error.HTTPError(
                            request.full_url,
                            302,
                            "redirect refused",
                            {"Location": redirect_target},
                            io.BytesIO(b"{}"),
                        )

                def fake_build_opener(*values: object):
                    handlers.extend(values)
                    return RedirectingOpener()

                with (
                    mock.patch.object(
                        READINESS.urllib.request,
                        "build_opener",
                        side_effect=fake_build_opener,
                    ),
                    mock.patch.object(
                        READINESS.urllib.request,
                        "urlopen",
                        side_effect=AssertionError(
                            "default redirect-capable transport used"
                        ),
                    ),
                ):
                    status, _ = invoke()

                self.assertEqual(302, status)
                self.assertEqual(1, len(opened))
                self.assertEqual(expected_url, opened[0].full_url)
                if "cross_origin" in name:
                    self.assertNotEqual(
                        urllib.parse.urlsplit(redirect_target).netloc,
                        urllib.parse.urlsplit(opened[0].full_url).netloc,
                    )
                self.assertTrue(
                    any(
                        isinstance(handler, READINESS._RejectRedirects)
                        for handler in handlers
                    )
                )

    def test_firefox_requires_authenticated_author_read(self) -> None:
        environment = {
            "FIREFOX_AMO_API_KEY": "api-key",
            "FIREFOX_AMO_API_SECRET": "api-secret",
        }
        detail = {
            "guid": READINESS.FIREFOX_ADDON_ID,
            "current_version": {"version": "1.2.3"},
            "status": "public",
        }
        profile = {"id": 123}
        authors = [{"user_id": 123, "role": "owner"}]
        with (
            mock.patch.object(
                READINESS,
                "_request_json",
                side_effect=((200, profile), (200, detail), (200, authors)),
            ) as request,
            mock.patch.object(READINESS, "_amo_jwt", return_value="jwt") as jwt,
        ):
            observation = READINESS.probe_firefox(environment)
        self.assertEqual("granted", observation["permission"])
        self.assertEqual("exists", observation["item"])
        self.assertEqual(3, request.call_count)
        self.assertTrue(request.call_args_list[0].args[0].endswith("/accounts/profile/"))
        self.assertTrue(request.call_args_list[2].args[0].endswith("/authors/"))
        self.assertEqual(
            [mock.call("api-key", "api-secret")] * 3,
            jwt.call_args_list,
        )

    def test_firefox_unrelated_valid_account_is_not_ready(self) -> None:
        environment = {
            "FIREFOX_AMO_API_KEY": "unrelated-key",
            "FIREFOX_AMO_API_SECRET": "unrelated-secret",
        }
        detail = {
            "guid": READINESS.FIREFOX_ADDON_ID,
            "current_version": {"version": "1.2.3"},
            "status": "public",
        }
        profile = {"id": 999}
        authors = [{"user_id": 123, "role": "owner"}]
        with mock.patch.object(
            READINESS,
            "_request_json",
            side_effect=((200, profile), (200, detail), (200, authors)),
        ):
            observation = READINESS.probe_firefox(environment)
        self.assertEqual("unverifiable", observation["permission"])
        self.assertEqual("unknown", observation["item"])
        snapshot = ready_snapshot()
        snapshot["stores"]["firefox"] = observation
        platform = READINESS.build_receipt(snapshot)["platforms"]["firefox"]
        self.assertFalse(platform["ready"])
        self.assertEqual("read_permission_unverifiable", platform["reason"])

    def test_firefox_rejects_unbounded_or_bool_caller_author_identities(self) -> None:
        environment = {
            "FIREFOX_AMO_API_KEY": "api-key",
            "FIREFOX_AMO_API_SECRET": "api-secret",
        }
        detail = {
            "guid": READINESS.FIREFOX_ADDON_ID,
            "current_version": {"version": "1.2.3"},
            "status": "public",
        }
        cases = (
            ({"id": 1 << 63}, [{"user_id": 1 << 63, "role": "owner"}]),
            ({"id": 1}, [{"user_id": True, "role": "owner"}]),
        )
        for profile, authors in cases:
            with self.subTest(profile=profile, authors=authors):
                with mock.patch.object(
                    READINESS,
                    "_request_json",
                    side_effect=((200, profile), (200, detail), (200, authors)),
                ):
                    observation = READINESS.probe_firefox(environment)
                self.assertEqual("unverifiable", observation["permission"])
                self.assertEqual("unknown", observation["item"])

    def test_chrome_taken_down_or_warned_fails_closed(self) -> None:
        for field, reason in (
            ("taken_down", "item_taken_down"),
            ("warned", "item_warned"),
        ):
            with self.subTest(field=field):
                snapshot = ready_snapshot()
                snapshot["stores"]["chrome"][field] = True
                platform = READINESS.build_receipt(snapshot)["platforms"]["chrome"]
                self.assertFalse(platform["ready"])
                self.assertEqual(reason, platform["reason"])
                self.assertTrue(platform[field])

    def test_chrome_probe_normalizes_removal_contract(self) -> None:
        base = {
            "itemId": "extension-id",
            "publishedItemRevisionStatus": {
                "state": "PUBLISHED",
                "distributionChannels": [{"crxVersion": "1.2.3"}],
            },
            "submittedItemRevisionStatus": {},
            "takenDown": False,
            "warned": False,
        }
        environment = {
            "CHROME_WEBSTORE_ACCESS_TOKEN": "redacted",
            "CHROME_WEBSTORE_PUBLISHER_ID": "publisher-id",
            "CHROME_WEBSTORE_EXTENSION_ID": "extension-id",
        }
        for field in ("takenDown", "warned"):
            with self.subTest(field=field):
                response = copy.deepcopy(base)
                response[field] = True
                with mock.patch.object(
                    READINESS, "_request_json", return_value=(200, response)
                ):
                    observation = READINESS.probe_chrome(environment, "1.2.3")
                self.assertIs(observation[READINESS.CHROME_REMOVAL_FIELDS[field]], True)
                snapshot = ready_snapshot()
                snapshot["stores"]["chrome"] = observation
                self.assertFalse(
                    READINESS.build_receipt(snapshot)["platforms"]["chrome"]["ready"]
                )

    def test_publish_ready_value_is_observed_without_enabling_actions(self) -> None:
        cases = {
            "missing": (None, False, "missing"),
            "false": ("false", False, "disabled"),
            "true": ("true", True, "enabled"),
        }
        for name, (value, enabled, state) in cases.items():
            with self.subTest(name=name):
                snapshot = ready_snapshot()
                if value is None:
                    snapshot["configuration"].pop(
                        "DCC_CUA_BROWSER_STORE_PUBLISH_READY"
                    )
                else:
                    snapshot["configuration"][
                        "DCC_CUA_BROWSER_STORE_PUBLISH_READY"
                    ] = value
                receipt = READINESS.build_receipt(snapshot)
                self.assertIs(receipt["publishing_enabled"], enabled)
                self.assertEqual(state, receipt["publishing_gate_state"])

    def test_receipt_is_stable_and_redacts_untrusted_provider_fields(self) -> None:
        snapshot = ready_snapshot()
        snapshot["stores"]["chrome"]["provider_message"] = "token=never-print-this"
        first = READINESS.serialize_receipt(READINESS.build_receipt(snapshot))
        second = READINESS.serialize_receipt(READINESS.build_receipt(copy.deepcopy(snapshot)))
        self.assertEqual(first, second)
        self.assertNotIn("never-print-this", first)

    def test_cli_redacts_every_untrusted_provider_string(self) -> None:
        sentinel = "TOKEN_NEVER_PRINT_P_PRIVATE_CHECKOUT"
        mutations = {
            "source_sha": ("github", "source_sha"),
            "tag": ("github", "tag"),
            "tag_target_sha": ("github", "tag_target_sha"),
            "release_target_sha": ("github", "release_target_sha"),
            "artifact_name": ("github", "artifact", "name"),
            "artifact_digest": ("github", "artifact", "digest"),
            "artifact_expires_at": ("github", "artifact", "expires_at"),
            "run_head_sha": ("github", "artifact", "workflow_run", "head_sha"),
            "run_status": ("github", "artifact", "workflow_run", "status"),
            "run_conclusion": ("github", "artifact", "workflow_run", "conclusion"),
            "run_event": ("github", "artifact", "workflow_run", "event"),
            "run_path": ("github", "artifact", "workflow_run", "path"),
            "run_head_branch": (
                "github",
                "artifact",
                "workflow_run",
                "head_branch",
            ),
            "chrome_permission": ("stores", "chrome", "permission"),
            "chrome_item": ("stores", "chrome", "item"),
            "chrome_version": ("stores", "chrome", "version"),
            "chrome_state": ("stores", "chrome", "state"),
            "chrome_taken_down": ("stores", "chrome", "taken_down"),
            "chrome_warned": ("stores", "chrome", "warned"),
            "edge_permission": ("stores", "edge", "permission"),
            "edge_item": ("stores", "edge", "item"),
            "edge_version": ("stores", "edge", "version"),
            "edge_state": ("stores", "edge", "state"),
            "firefox_permission": ("stores", "firefox", "permission"),
            "firefox_item": ("stores", "firefox", "item"),
            "firefox_version": ("stores", "firefox", "version"),
            "firefox_state": ("stores", "firefox", "state"),
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, path in mutations.items():
                with self.subTest(name=name):
                    snapshot = ready_snapshot()
                    target = snapshot
                    for component in path[:-1]:
                        target = target[component]
                    target[path[-1]] = f"{sentinel}_{name}_P:\\private\\checkout"
                    snapshot_path = root / f"{name}.json"
                    output_path = root / f"{name}-receipt.json"
                    snapshot_path.write_text(json.dumps(snapshot), encoding="utf-8")

                    result = self.run_cli(
                        "--snapshot", snapshot_path, "--output", output_path
                    )

                    self.assertEqual(1, result.returncode)
                    self.assertEqual(1, result.stdout.count("\n"))
                    self.assertEqual(
                        result.stdout, output_path.read_text(encoding="utf-8")
                    )
                    combined = result.stdout + result.stderr
                    self.assertNotIn(sentinel, combined)
                    self.assertNotIn("private", combined.lower())
                    self.assertNotIn("Traceback", combined)

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

    def test_action_pin_audit_reads_quoted_yaml_keys(self) -> None:
        workflow = """
name: quoted action key
on: workflow_dispatch
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      - "uses": actions/checkout@v7
"""
        self.assertEqual(
            {"valid": False, "unpinned": ["workflow.yml:actions/checkout@v7"]},
            self.audit_workflow(workflow),
        )

    def test_action_pin_audit_accepts_exact_structural_pin_classes(self) -> None:
        sha = "a" * 40
        digest = "b" * 64
        workflow = f"""
name: exact pins
on: workflow_dispatch
jobs:
  reusable:
    "uses": owner/repository/.github/workflows/reusable.yml@{sha}
  inspect:
    runs-on: ubuntu-latest
    steps:
      - uses: ./.github/actions/select-macos-toolchain
      - "uses": owner/repository/path/to/action@{sha}
      - uses: docker://registry.example.invalid/image@sha256:{digest}
"""
        self.assertEqual(
            {"valid": True, "unpinned": []},
            self.audit_workflow(workflow),
        )

    def test_local_action_must_exist_within_the_repository(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "repository"
            workflow = root / ".github" / "workflows" / "workflow.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                self.local_action_workflow("./.github/actions/missing"),
                encoding="utf-8",
            )
            audit = READINESS.audit_action_pins([workflow])
        self.assertFalse(audit["valid"])
        self.assertTrue(audit["unpinned"])

    def test_local_action_symlink_or_junction_escape_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            root = temporary / "repository"
            workflow = root / ".github" / "workflows" / "workflow.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                self.local_action_workflow("./.github/actions/escape"),
                encoding="utf-8",
            )
            outside = temporary / "outside-action"
            outside.mkdir()
            (outside / "action.yml").write_text("name: outside\n", encoding="utf-8")
            local = root / ".github" / "actions" / "escape"
            local.parent.mkdir(parents=True)
            if os.name == "nt":
                result = subprocess.run(
                    ["cmd", "/c", "mklink", "/J", str(local), str(outside)],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                self.assertEqual(0, result.returncode, result.stderr)
            else:
                os.symlink(outside, local, target_is_directory=True)
            audit = READINESS.audit_action_pins([workflow])
        self.assertFalse(audit["valid"])
        self.assertTrue(audit["unpinned"])

    def test_local_action_reparse_component_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "repository"
            workflow = root / ".github" / "workflows" / "workflow.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                self.local_action_workflow("./.github/actions/local"),
                encoding="utf-8",
            )
            local = root / ".github" / "actions" / "local"
            local.mkdir(parents=True)
            (local / "action.yml").write_text("name: local\n", encoding="utf-8")
            real_lstat = os.lstat

            def reparse_lstat(path: object):
                value = real_lstat(path)
                if Path(path) == local:
                    return SimpleNamespace(
                        st_mode=value.st_mode,
                        st_dev=value.st_dev,
                        st_ino=value.st_ino,
                        st_file_attributes=(
                            getattr(value, "st_file_attributes", 0) | 0x400
                        ),
                    )
                return value

            with mock.patch.object(READINESS.os, "lstat", side_effect=reparse_lstat):
                audit = READINESS.audit_action_pins([workflow])
        self.assertFalse(audit["valid"])

    def test_local_action_path_identity_replacement_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "repository"
            workflow = root / ".github" / "workflows" / "workflow.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                self.local_action_workflow("./.github/actions/local"),
                encoding="utf-8",
            )
            local = root / ".github" / "actions" / "local"
            local.mkdir(parents=True)
            (local / "action.yml").write_text("name: local\n", encoding="utf-8")
            real_lstat = os.lstat
            local_reads = 0

            def unstable_lstat(path: object):
                nonlocal local_reads
                value = real_lstat(path)
                if Path(path) == local:
                    local_reads += 1
                    if local_reads > 1:
                        return SimpleNamespace(
                            st_mode=value.st_mode,
                            st_dev=value.st_dev,
                            st_ino=value.st_ino + 1,
                            st_file_attributes=getattr(
                                value, "st_file_attributes", 0
                            ),
                        )
                return value

            with mock.patch.object(READINESS.os, "lstat", side_effect=unstable_lstat):
                audit = READINESS.audit_action_pins([workflow])
        self.assertFalse(audit["valid"])
        self.assertGreaterEqual(local_reads, 2)

    def test_action_pin_audit_rejects_mutable_structural_pin_classes(self) -> None:
        sha = "a" * 40
        cases = {
            "quoted_reusable_workflow": """
jobs:
  reusable:
    "uses": owner/repository/.github/workflows/reusable.yml@main
""",
            "expression": """
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      - uses: ${{ matrix.action }}
""",
            "docker_tag": """
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      - uses: docker://alpine:3.20
""",
            "flow_collection": """
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps: [{"uses": "actions/checkout@v7"}]
""",
            "local_parent_escape": """
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      - uses: ./../outside/action
""",
            "remote_parent_escape": f"""
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      - uses: owner/repository/../../outside@{sha}
""",
        }
        for name, workflow in cases.items():
            with self.subTest(name=name):
                audit = self.audit_workflow(workflow)
                self.assertFalse(audit["valid"])
                self.assertTrue(audit["unpinned"])

    def test_action_pin_audit_rejects_ambiguous_yaml_structure(self) -> None:
        cases = {
            "duplicate_mapping": """
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      - uses: owner/repository/action@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
        "uses": actions/checkout@v7
""",
            "anchor_alias": """
shared: &shared
  uses: actions/checkout@v7
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      - *shared
""",
        }
        for name, workflow in cases.items():
            with self.subTest(name=name):
                self.assertEqual(
                    {"valid": False, "unpinned": ["workflow.yml:invalid_workflow_yaml"]},
                    self.audit_workflow(workflow),
                )

    def test_action_pin_audit_fails_closed_without_yaml_parser(self) -> None:
        workflow = """
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      - uses: owner/repository/action@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
"""
        with mock.patch.object(READINESS, "yaml", None):
            self.assertEqual(
                {"valid": False, "unpinned": ["workflow.yml:invalid_workflow_yaml"]},
                self.audit_workflow(workflow),
            )

    def test_action_pin_audit_requires_unambiguous_executable_structure(self) -> None:
        sha = "a" * 40
        cases = {
            "missing_jobs": "name: no jobs\n",
            "empty_jobs": "jobs: {}\n",
            "job_without_execution": """
jobs:
  inspect:
    runs-on: ubuntu-latest
""",
            "job_with_uses_and_steps": f"""
jobs:
  inspect:
    uses: owner/repository/.github/workflows/reusable.yml@{sha}
    steps:
      - run: echo decoy
""",
            "empty_steps": """
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps: []
""",
            "step_with_uses_and_run": f"""
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      - uses: owner/repository/action@{sha}
        run: echo decoy
""",
        }
        for name, workflow in cases.items():
            with self.subTest(name=name):
                self.assertEqual(
                    {"valid": False, "unpinned": ["workflow.yml:invalid_workflow_yaml"]},
                    self.audit_workflow(workflow),
                )

    def test_action_pin_audit_ignores_comments_and_scalar_decoys(self) -> None:
        sha = "c" * 40
        workflow = f"""
name: "uses: actions/checkout@v7"
on: workflow_dispatch
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      # uses: actions/checkout@v7
      - name: "uses: docker://alpine:latest"
        run: echo safe
      - uses: owner/repository/action@{sha}
"""
        self.assertEqual(
            {"valid": True, "unpinned": []},
            self.audit_workflow(workflow),
        )

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
        self.assertEqual(2, workflow.count(READINESS.CI_YAML_INSTALL_COMMAND))
        self.assertEqual(
            "PyYAML==6.0.2\n",
            (ROOT / "scripts" / "requirements-browser-store-readiness.txt").read_text(
                encoding="utf-8"
            ),
        )
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

    def test_ci_receipt_contract_is_executable_but_not_branch_required(self) -> None:
        workflow = (ROOT / ".github" / "workflows" / "ci-checks.yml").read_text(
            encoding="utf-8"
        )
        self.assertEqual(
            {
                "valid": True,
                "reasons": [],
                "branch_required": False,
                "branch_required_evidence": "not_observed",
            },
            READINESS.audit_ci_contract(workflow),
        )
        self.assertTrue((ROOT / "scripts" / "test_browser_store_readiness.py").is_file())

    def test_ci_receipt_contract_reports_branch_requirement_only_when_observed(self) -> None:
        workflow = (ROOT / ".github" / "workflows" / "ci-checks.yml").read_text(
            encoding="utf-8"
        )
        not_required = READINESS.audit_ci_contract(
            workflow, branch_required_observed=False
        )
        required = READINESS.audit_ci_contract(
            workflow, branch_required_observed=True
        )
        self.assertFalse(not_required["branch_required"])
        self.assertEqual("observed_not_required", not_required["branch_required_evidence"])
        self.assertTrue(required["branch_required"])
        self.assertEqual("observed_required", required["branch_required_evidence"])

    def test_ci_receipt_contract_rejects_disabled_or_decoyed_surfaces(self) -> None:
        workflow = (ROOT / ".github" / "workflows" / "ci-checks.yml").read_text(
            encoding="utf-8"
        )
        command = "python -B -m unittest scripts.test_browser_store_readiness"
        step = f"      - run: {command}"
        mutations = {
            "missing_pull_request": workflow.replace("  pull_request:\n", "", 1),
            "impossible_pull_request_branch": workflow.replace(
                "  pull_request:\n",
                "  pull_request:\n    branches: [branch-that-cannot-be-main]\n",
                1,
            ),
            "top_level_shell_default": workflow.replace(
                "\npermissions:\n",
                "\ndefaults:\n  run:\n    shell: echo {0}\n\npermissions:\n",
                1,
            ),
            "top_level_permissions_write": workflow.replace(
                "  contents: read", "  contents: write", 1
            ),
            "self_hosted_runner": workflow.replace(
                "  policy:\n    runs-on: ubuntu-latest",
                "  policy:\n    runs-on: self-hosted",
                1,
            ),
            "unavailable_hosted_runner": workflow.replace(
                "  policy:\n    runs-on: ubuntu-latest",
                "  policy:\n    runs-on: ubuntu-2099",
                1,
            ),
            "job_dependency": workflow.replace(
                "  policy:\n    runs-on: ubuntu-latest",
                "  policy:\n    needs: browser-extension\n    runs-on: ubuntu-latest",
                1,
            ),
            "job_permissions": workflow.replace(
                "  policy:\n    runs-on: ubuntu-latest",
                "  policy:\n    permissions:\n      contents: read\n    runs-on: ubuntu-latest",
                1,
            ),
            "job_timeout_changed": workflow.replace(
                "  policy:\n    runs-on: ubuntu-latest\n    timeout-minutes: 10",
                "  policy:\n    runs-on: ubuntu-latest\n    timeout-minutes: 1",
                1,
            ),
            "receipt_command_reordered": workflow.replace(
                "      - run: python -B -m unittest scripts.test_refresh_release_please_prs\n"
                + step,
                step
                + "\n      - run: python -B -m unittest scripts.test_refresh_release_please_prs",
                1,
            ),
            "receipt_command_preceded_by_overwrite": workflow.replace(
                step, "      - run: echo receipt skipped\n" + step, 1
            ),
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
            "bash_no_execute": workflow.replace(
                step, step + "\n        shell: bash -n {0}", 1
            ),
            "echo_no_execute": workflow.replace(
                step, step + "\n        shell: echo {0}", 1
            ),
            "step_env": workflow.replace(
                step, step + "\n        env:\n          PYTHONPATH: scripts", 1
            ),
            "working_directory": workflow.replace(
                step, step + "\n        working-directory: scripts", 1
            ),
            "step_timeout": workflow.replace(
                step, step + "\n        timeout-minutes: 1", 1
            ),
            "preceding_run_overwrite": workflow.replace(
                step, "      - run: echo skipped\n        run: " + command, 1
            ),
            "checkout_repository_input": workflow.replace(
                "  policy:\n    runs-on: ubuntu-latest\n"
                "    timeout-minutes: 10\n    steps:\n"
                "      - uses: actions/checkout@v7\n",
                "  policy:\n    runs-on: ubuntu-latest\n"
                "    timeout-minutes: 10\n    steps:\n"
                "      - uses: actions/checkout@v7\n"
                "        with:\n"
                "          repository: owner/other\n",
                1,
            ),
            "checkout_ref_input": workflow.replace(
                "  policy:\n    runs-on: ubuntu-latest\n"
                "    timeout-minutes: 10\n    steps:\n"
                "      - uses: actions/checkout@v7\n",
                "  policy:\n    runs-on: ubuntu-latest\n"
                "    timeout-minutes: 10\n    steps:\n"
                "      - uses: actions/checkout@v7\n"
                "        with:\n"
                "          ref: refs/heads/other\n",
                1,
            ),
            "toolchain_input_changed": workflow.replace(
                "          toolchain: 1.95.0",
                "          toolchain: stable",
                1,
            ),
            "toolchain_component_changed": workflow.replace(
                "          components: rustfmt",
                "          components: clippy",
                1,
            ),
            "install_tool_input_changed": workflow.replace(
                "          tool: cargo-hakari",
                "          tool: cargo-nextest",
                1,
            ),
            "hakari_block_body_replaced": workflow.replace(
                "      - run: |\n"
                "          cargo hakari generate --diff\n"
                "          cargo hakari manage-deps --dry-run",
                "      - run: |\n"
                "          echo arbitrary replacement\n"
                "          exit 0",
                1,
            ),
            "hakari_block_comment_decoy": workflow.replace(
                "      - run: |\n"
                "          cargo hakari generate --diff\n"
                "          cargo hakari manage-deps --dry-run",
                "      - run: |\n"
                "          # cargo hakari generate --diff\n"
                "          # cargo hakari manage-deps --dry-run\n"
                "          echo skipped",
                1,
            ),
            "hakari_equivalent_later_job_decoy": workflow.replace(
                "      - run: |\n"
                "          cargo hakari generate --diff\n"
                "          cargo hakari manage-deps --dry-run",
                "      - run: |\n"
                "          echo arbitrary replacement\n",
                1,
            ).replace(
                "\n  verify:",
                "\n  hakari-decoy:\n"
                "    runs-on: ubuntu-latest\n"
                "    steps:\n"
                "      - run: |\n"
                "          cargo hakari generate --diff\n"
                "          cargo hakari manage-deps --dry-run\n\n"
                "  verify:",
                1,
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
                audit = READINESS.audit_ci_contract(mutation)
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
