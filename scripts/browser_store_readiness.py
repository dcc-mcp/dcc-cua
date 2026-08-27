#!/usr/bin/env python3
"""Build a stable, redacted, read-only browser-store readiness receipt."""

from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import os
import re
import secrets
import stat
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Mapping, Sequence
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

try:
    import yaml
except ImportError:  # Fail closed at audit time with a stable public reason.
    yaml = None


EXPECTED_CONFIGURATION_NAMES = (
    "CHROME_WEBSTORE_EXTENSION_ID",
    "EDGE_ADDONS_CLIENT_ID",
    "EDGE_ADDONS_PRODUCT_ID",
    "EDGE_ADDONS_API_KEY",
    "FIREFOX_AMO_API_KEY",
    "FIREFOX_AMO_API_SECRET",
    "CHROME_WEBSTORE_WORKLOAD_IDENTITY_PROVIDER",
    "CHROME_WEBSTORE_SERVICE_ACCOUNT",
    "CHROME_WEBSTORE_PUBLISHER_ID",
    "DCC_CUA_BROWSER_STORE_PUBLISH_READY",
)
PLATFORM_CONFIGURATION = {
    "chrome": (
        "CHROME_WEBSTORE_WORKLOAD_IDENTITY_PROVIDER",
        "CHROME_WEBSTORE_SERVICE_ACCOUNT",
        "CHROME_WEBSTORE_PUBLISHER_ID",
        "CHROME_WEBSTORE_EXTENSION_ID",
    ),
    "edge": (
        "EDGE_ADDONS_API_KEY",
        "EDGE_ADDONS_CLIENT_ID",
        "EDGE_ADDONS_PRODUCT_ID",
    ),
    "firefox": ("FIREFOX_AMO_API_KEY", "FIREFOX_AMO_API_SECRET"),
}
ITEM_ID_CONFIGURATION = {
    "chrome": "CHROME_WEBSTORE_EXTENSION_ID",
    "edge": "EDGE_ADDONS_PRODUCT_ID",
}
PROVIDER_ITEM_STATE_CLASSIFICATIONS = {
    "chrome": {
        "published": True,
        "pending_review": False,
        "staged": False,
    },
    "edge": {
        "in_store": True,
        "in_review": False,
        "approved": False,
    },
    "firefox": {
        "public": True,
        "unlisted": True,
    },
}
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
DIGEST_PATTERN = re.compile(r"^sha256:[0-9a-f]{64}$")
ACTION_PATH_SEGMENT_PATTERN = re.compile(r"^[A-Za-z0-9_.-]+$")
PINNED_REMOTE_ACTION_PATTERN = re.compile(r"^([^@\s]+)@([0-9a-f]{40})$")
PINNED_DOCKER_ACTION_PATTERN = re.compile(
    r"^docker://[^@\s]+@sha256:[0-9a-f]{64}$"
)
PUBLIC_VERSION_PATTERN = re.compile(
    r"^(?:0|[1-9][0-9]*)(?:\.(?:0|[1-9][0-9]*)){2,3}$"
)
RELEASE_TAG_PATTERN = re.compile(
    rf"^dcc-cua-browser-extension-v({PUBLIC_VERSION_PATTERN.pattern[1:-1]})$"
)
CANONICAL_TIMESTAMP_PATTERN = re.compile(
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"
)
KNOWN_READ_PERMISSIONS = {"granted", "denied", "not_checked", "unverifiable"}
CHROME_REMOVAL_FIELDS = {"takenDown": "taken_down", "warned": "warned"}
EXPECTED_ARTIFACT_RUN = {
    "status": "completed",
    "conclusion": "success",
    "event": "push",
    "path": ".github/workflows/release-please.yml",
    "head_branch": "main",
}
FIREFOX_ADDON_ID = "dcc-cua@dcc-mcp.org"
# GitHub and AMO database identities are accepted only in the positive signed
# 64-bit domain. This excludes JSON booleans and implementation-sized overflow.
MAX_EXTERNAL_ID = (1 << 63) - 1
CI_POLICY_JOB = "policy"
CI_RECEIPT_COMMAND = "python -B -m unittest scripts.test_browser_store_readiness"
CI_YAML_INSTALL_COMMAND = (
    "python -m pip install --disable-pip-version-check --no-deps "
    "-r scripts/requirements-browser-store-readiness.txt"
)
EXPECTED_CI_TOP_LEVEL_KEYS = ("name", "on", "permissions", "concurrency", "jobs")
EXPECTED_CI_TRIGGER = (
    "  push:",
    "    branches: [main]",
    "  pull_request:",
    "  workflow_dispatch: {}",
)
EXPECTED_CI_PERMISSIONS = ("  contents: read",)
EXPECTED_CI_CONCURRENCY = (
    "  group: ci-${{ github.workflow }}-${{ github.ref }}",
    "  cancel-in-progress: true",
)
EXPECTED_POLICY_JOB_FIELDS = {
    "runs-on": "ubuntu-latest",
    "timeout-minutes": "10",
    "steps": "",
}
EXPECTED_HAKARI_COMMAND = (
    "cargo hakari generate --diff\n"
    "cargo hakari manage-deps --dry-run\n"
)
EXPECTED_POLICY_STEPS = (
    {"uses": "actions/checkout@v7"},
    {
        "uses": "dtolnay/rust-toolchain@stable",
        "with": {"toolchain": "1.95.0", "components": "rustfmt"},
    },
    {"uses": "taiki-e/install-action@v2", "with": {"tool": "cargo-hakari"}},
    {"run": CI_YAML_INSTALL_COMMAND},
    {"run": "pwsh -NoProfile -File scripts/check-rust-layout.ps1"},
    {"run": "pwsh -NoProfile -File scripts/check-agent-skills.ps1"},
    {"run": "python -B scripts/test_write_install_manifest.py"},
    {"run": "python -B -m unittest scripts.test_verify_release_assets"},
    {"run": "python -B -m unittest scripts.test_release_integrity"},
    {"run": "python -B -m unittest scripts.test_release_workflow"},
    {"run": "python -B -m unittest scripts.test_refresh_release_please_prs"},
    {"run": CI_RECEIPT_COMMAND},
    {"run": EXPECTED_HAKARI_COMMAND},
    {"run": "cargo fmt --all -- --check"},
)
EXPECTED_POLICY_ACTION_INPUTS = {
    "actions/checkout@v7": {},
    "dtolnay/rust-toolchain@stable": {
        "toolchain": "1.95.0",
        "components": "rustfmt",
    },
    "taiki-e/install-action@v2": {"tool": "cargo-hakari"},
}


class ReadinessError(RuntimeError):
    """A provider-safe preflight failure."""


def _bool(value: object) -> bool:
    return value is True


def _mapping(value: object) -> Mapping[str, Any]:
    return value if isinstance(value, Mapping) else {}


def _external_id(value: object) -> bool:
    return type(value) is int and 0 < value <= MAX_EXTERNAL_ID


def _release_identity(value: object) -> tuple[str | None, str]:
    if not isinstance(value, str):
        return None, ""
    match = RELEASE_TAG_PATTERN.fullmatch(value)
    return (value, match.group(1)) if match else (None, "")


def _canonical_expiry(value: object) -> tuple[str | None, datetime | None]:
    if not isinstance(value, str) or CANONICAL_TIMESTAMP_PATTERN.fullmatch(value) is None:
        return None, None
    try:
        parsed = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(
            tzinfo=timezone.utc
        )
    except ValueError:
        return None, None
    return parsed.strftime("%Y-%m-%dT%H:%M:%SZ"), parsed


def _configuration_present(configuration: Mapping[str, Any], name: str) -> bool:
    value = configuration.get(name)
    if name != "DCC_CUA_BROWSER_STORE_PUBLISH_READY":
        return _bool(value)
    if isinstance(value, Mapping):
        return _bool(value.get("present"))
    if isinstance(value, str):
        return value.lower() in {"true", "false"}
    return _bool(value)


def _publishing_gate(configuration: Mapping[str, Any]) -> tuple[bool, str]:
    if "DCC_CUA_BROWSER_STORE_PUBLISH_READY" not in configuration:
        return False, "missing"
    value = configuration.get("DCC_CUA_BROWSER_STORE_PUBLISH_READY")
    if isinstance(value, Mapping):
        if not _bool(value.get("present")):
            return False, "missing"
        enabled = value.get("enabled")
        return (
            (enabled, "enabled" if enabled else "disabled")
            if isinstance(enabled, bool)
            else (False, "unknown")
        )
    if isinstance(value, str):
        normalized = value.lower()
        if normalized == "true":
            return True, "enabled"
        if normalized == "false":
            return False, "disabled"
    if isinstance(value, bool):
        return value, "enabled" if value else "disabled"
    return False, "unknown"


def _environment_receipt(environment: Mapping[str, Any]) -> dict[str, object]:
    raw_policy = environment.get("deployment_branch_policy")
    policy_present = "deployment_branch_policy" in environment
    policy_valid = (
        isinstance(raw_policy, Mapping)
        and isinstance(raw_policy.get("protected_branches"), bool)
        and isinstance(raw_policy.get("custom_branch_policies"), bool)
    )
    policy = raw_policy if policy_valid else {}
    protected_only = _bool(policy.get("protected_branches"))
    custom = _bool(policy.get("custom_branch_policies"))
    default_protected = _bool(environment.get("default_branch_protected"))
    valid = environment.get("name") == "browser-stores"
    reason = "eligible"
    if not valid:
        reason = "environment_missing"
    elif not policy_present:
        valid = False
        reason = "environment_policy_missing"
    elif not policy_valid:
        valid = False
        reason = "environment_policy_invalid"
    elif protected_only and not custom and not default_protected:
        valid = False
        reason = "default_branch_not_eligible"
    elif custom:
        eligibility = environment.get("default_branch_eligible")
        valid = isinstance(eligibility, bool) and eligibility
        reason = (
            "eligible"
            if valid
            else (
                "default_branch_not_eligible"
                if eligibility is False
                else "default_branch_eligibility_unknown"
            )
        )
    return {
        "name": "browser-stores",
        "valid": valid,
        "reason": reason,
        "protected_branches_only": protected_only,
        "custom_branch_policies": custom,
        "default_branch_protected": default_protected,
    }


def _github_identity_receipt(github: Mapping[str, Any]) -> dict[str, object]:
    repository = _mapping(github.get("repository"))
    artifact = _mapping(github.get("artifact"))
    run = _mapping(artifact.get("workflow_run"))
    source = str(github.get("source_sha", "")).lower()
    tag_target = str(github.get("tag_target_sha", "")).lower()
    release_target = str(github.get("release_target_sha", "")).lower()
    run_head = str(run.get("head_sha", "")).lower()
    reasons: list[str] = []
    release_tag, _ = _release_identity(github.get("tag"))
    if release_tag is None:
        reasons.append("invalid_release_tag")
    if not all(SHA_PATTERN.fullmatch(value) for value in (source, tag_target, release_target, run_head)):
        reasons.append("invalid_source_identity")
    elif len({source, tag_target, release_target, run_head}) != 1:
        reasons.append("source_head_drift")
    repository_id = repository.get("id")
    if not _external_id(repository_id):
        reasons.append("invalid_repository_identity")
    run_repository_id = run.get("repository_id")
    head_repository_id = run.get("head_repository_id")
    if run_repository_id != repository_id or head_repository_id != repository_id:
        reasons.append("artifact_repository_mismatch")
    if any(run.get(field) != expected for field, expected in EXPECTED_ARTIFACT_RUN.items()):
        reasons.append("artifact_producer_mismatch")
    artifact_id = artifact.get("id")
    run_id = run.get("id")
    release_id = github.get("release_id")
    if not all(
        _external_id(value)
        for value in (
            repository_id,
            release_id,
            artifact_id,
            run_id,
            run_repository_id,
            head_repository_id,
        )
    ):
        reasons.append("invalid_numeric_identity")
    if artifact.get("name") != "dcc-cua-browser-extension":
        reasons.append("artifact_name_mismatch")
    digest = str(artifact.get("digest", "")).lower()
    if DIGEST_PATTERN.fullmatch(digest) is None:
        reasons.append("artifact_digest_mismatch")
    expired = artifact.get("expired")
    if expired is not False:
        reasons.append("invalid_artifact_expired")
    expires_at, expires = _canonical_expiry(artifact.get("expires_at"))
    if expires is None:
        reasons.append("invalid_artifact_expiry")
    elif expires <= datetime.now(timezone.utc):
        reasons.append("artifact_expired")
    reasons = sorted(set(reasons))
    return {
        "valid": not reasons,
        "reasons": reasons,
        "repository_id": repository_id if _external_id(repository_id) else None,
        "source_sha": source if SHA_PATTERN.fullmatch(source) else None,
        "tag": release_tag,
        "release_id": release_id if _external_id(release_id) else None,
        "artifact_id": artifact_id if _external_id(artifact_id) else None,
        "artifact_digest": digest if DIGEST_PATTERN.fullmatch(digest) else None,
        "run_id": run_id if _external_id(run_id) else None,
        "head_sha": run_head if SHA_PATTERN.fullmatch(run_head) else None,
        "expired": False if expired is False else None,
        "expires_at": expires_at,
    }


def _platform_receipt(
    platform: str,
    configuration: Mapping[str, Any],
    observation: Mapping[str, Any],
    expected_version: str,
) -> dict[str, object]:
    missing = sorted(
        name for name in PLATFORM_CONFIGURATION[platform] if not _bool(configuration.get(name))
    )
    raw_permission = observation.get("permission", "not_checked")
    permission = (
        raw_permission
        if isinstance(raw_permission, str) and raw_permission in KNOWN_READ_PERMISSIONS
        else "unknown"
    )
    result: dict[str, object] = {
        "ready": False,
        "state": "not_ready",
        "reason": "missing_configuration" if missing else "readback_missing",
        "missing_configuration": missing,
        "configuration_present": len(PLATFORM_CONFIGURATION[platform]) - len(missing),
        "configuration_expected": len(PLATFORM_CONFIGURATION[platform]),
        "read_permission": permission,
        "item_exists": observation.get("item") == "exists",
        "version": None,
        "item_state": None,
    }
    if platform == "chrome":
        result["taken_down"] = (
            observation.get("taken_down")
            if isinstance(observation.get("taken_down"), bool)
            else None
        )
        result["warned"] = (
            observation.get("warned")
            if isinstance(observation.get("warned"), bool)
            else None
        )
    item_name = ITEM_ID_CONFIGURATION.get(platform)
    if item_name in missing:
        result.update(state="human_action_required", reason="item_onboarding_required")
        return result
    if missing:
        return result
    if permission == "denied":
        result["reason"] = "read_permission_denied"
        return result
    if permission != "granted":
        result.update(state="human_action_required", reason="read_permission_unverifiable")
        return result
    item = observation.get("item")
    if item != "exists":
        result.update(
            state="human_action_required" if item == "missing" else "not_ready",
            reason="item_onboarding_required" if item == "missing" else "item_existence_unknown",
        )
        return result
    if platform == "chrome":
        if result["taken_down"] is True:
            result["reason"] = "item_taken_down"
            return result
        if result["warned"] is True:
            result["reason"] = "item_warned"
            return result
        if result["taken_down"] is not False or result["warned"] is not False:
            result["reason"] = "removal_state_unknown"
            return result
    raw_version = observation.get("version")
    version_matches = (
        isinstance(raw_version, str)
        and PUBLIC_VERSION_PATTERN.fullmatch(raw_version) is not None
        and raw_version == expected_version
    )
    classifications = PROVIDER_ITEM_STATE_CLASSIFICATIONS[platform]
    raw_state = observation.get("state")
    state = (
        raw_state
        if isinstance(raw_state, str) and raw_state in classifications
        else None
    )
    result["version"] = expected_version if version_matches else None
    result["item_state"] = state
    if state is None:
        result["reason"] = "unknown_item_state"
        return result
    if classifications[state] is not True:
        result["reason"] = "item_state_not_ready"
        return result
    if not version_matches:
        result["reason"] = "version_mismatch"
        return result
    result.update(ready=True, state="ready", reason="readback_verified")
    return result


def build_receipt(snapshot: Mapping[str, Any]) -> dict[str, Any]:
    configuration = _mapping(snapshot.get("configuration"))
    environment = _environment_receipt(_mapping(snapshot.get("environment")))
    github = _github_identity_receipt(_mapping(snapshot.get("github")))
    raw_pins = _mapping(snapshot.get("action_pins"))
    unpinned = raw_pins.get("unpinned", [])
    unpinned_count = len(unpinned) if isinstance(unpinned, Sequence) else 0
    pins = {"valid": _bool(raw_pins.get("valid")), "unpinned_count": unpinned_count}
    _, expected_version = _release_identity(_mapping(snapshot.get("github")).get("tag"))
    observations = _mapping(snapshot.get("stores"))
    publishing_enabled, publishing_gate_state = _publishing_gate(configuration)
    platforms = {
        platform: _platform_receipt(
            platform,
            configuration,
            _mapping(observations.get(platform)),
            expected_version,
        )
        for platform in ("chrome", "edge", "firefox")
    }
    ready = bool(
        expected_version
        and environment["valid"]
        and github["valid"]
        and pins["valid"]
        and all(platform["ready"] for platform in platforms.values())
    )
    platform_states = {platform["state"] for platform in platforms.values()}
    overall_state = "ready" if ready else (
        "human_action_required"
        if platform_states == {"human_action_required"}
        and environment["valid"]
        and github["valid"]
        and pins["valid"]
        else "not_ready"
    )
    return {
        "schema": "dcc-cua.browser-store-readiness.v1",
        "ready": ready,
        "overall_state": overall_state,
        "publishing_enabled": publishing_enabled,
        "publishing_gate_state": publishing_gate_state,
        "configuration": {
            "expected_names": list(EXPECTED_CONFIGURATION_NAMES),
            "present_count": sum(
                1
                for name in EXPECTED_CONFIGURATION_NAMES
                if _configuration_present(configuration, name)
            ),
            "expected_count": len(EXPECTED_CONFIGURATION_NAMES),
            "values_redacted": True,
        },
        "environment": environment,
        "github_identity": github,
        "action_pins": pins,
        "platforms": platforms,
    }


def serialize_receipt(receipt: Mapping[str, Any]) -> str:
    return json.dumps(receipt, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n"


def audit_action_pins(paths: Sequence[Path]) -> dict[str, object]:
    unpinned: list[str] = []
    for path in paths:
        try:
            repository_root = _workflow_repository_root(path)
            workflow = _load_workflow_yaml(path.read_text(encoding="utf-8"))
            actions = _workflow_uses(workflow)
        except (OSError, UnicodeError, ValueError):
            unpinned.append(f"{path.name}:invalid_workflow_yaml")
            continue
        for action in actions:
            if not _immutable_action_reference(
                action, repository_root=repository_root
            ):
                unpinned.append(f"{path.name}:{action}")
    return {"valid": not unpinned, "unpinned": sorted(unpinned)}


def _load_workflow_yaml(text: str) -> Mapping[str, Any]:
    if yaml is None:
        raise ValueError("YAML parser unavailable")

    try:
        for event in yaml.parse(text, Loader=yaml.BaseLoader):
            if (
                isinstance(event, yaml.events.AliasEvent)
                or getattr(event, "anchor", None) is not None
            ):
                raise ValueError("YAML aliases and anchors are not allowed")

        class UniqueKeyLoader(yaml.BaseLoader):
            def construct_mapping(self, node: Any, deep: bool = False) -> dict[str, Any]:
                if not isinstance(node, yaml.MappingNode):
                    raise ValueError("mapping required")
                mapping: dict[str, Any] = {}
                for key_node, value_node in node.value:
                    key = self.construct_object(key_node, deep=deep)
                    if not isinstance(key, str) or key == "<<" or key in mapping:
                        raise ValueError("unique string mapping keys required")
                    mapping[key] = self.construct_object(value_node, deep=deep)
                return mapping

        value = yaml.load(text, Loader=UniqueKeyLoader)
    except (yaml.YAMLError, ValueError) as error:
        raise ValueError("invalid workflow YAML") from error
    if not isinstance(value, Mapping):
        raise ValueError("workflow root must be a mapping")
    return value


def _workflow_uses(workflow: Mapping[str, Any]) -> list[str]:
    jobs = workflow.get("jobs")
    if not isinstance(jobs, Mapping) or not jobs:
        raise ValueError("workflow jobs must be a non-empty mapping")
    actions: list[str] = []
    for job in jobs.values():
        if not isinstance(job, Mapping):
            raise ValueError("workflow job must be a mapping")
        has_uses = "uses" in job
        has_steps = "steps" in job
        if has_uses == has_steps:
            raise ValueError("workflow job must have exactly one execution mode")
        if has_uses:
            action = job["uses"]
            if not isinstance(action, str) or not action:
                raise ValueError("job uses must be a non-empty scalar")
            actions.append(action)
            continue
        steps = job["steps"]
        if not isinstance(steps, list) or not steps:
            raise ValueError("workflow steps must be a non-empty sequence")
        for step in steps:
            if not isinstance(step, Mapping):
                raise ValueError("workflow step must be a mapping")
            has_uses = "uses" in step
            has_run = "run" in step
            if has_uses == has_run:
                raise ValueError("workflow step must have exactly one execution mode")
            if not has_uses:
                continue
            action = step["uses"]
            if not isinstance(action, str) or not action:
                raise ValueError("step uses must be a non-empty scalar")
            actions.append(action)
    return actions


def _workflow_repository_root(workflow_path: Path) -> Path:
    path = Path(os.path.abspath(workflow_path))
    if path.parent.name != "workflows" or path.parent.parent.name != ".github":
        raise ValueError("workflow is outside the repository workflow directory")
    return path.parent.parent.parent


def _path_identity(path: Path) -> tuple[int, int, int, int]:
    value = os.lstat(path)
    attributes = int(getattr(value, "st_file_attributes", 0))
    reparse_flag = int(getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400))
    if stat.S_ISLNK(value.st_mode) or attributes & reparse_flag:
        raise ValueError("linked or reparse paths are not repository-owned")
    return (int(value.st_dev), int(value.st_ino), int(value.st_mode), attributes)


def _optional_path_identity(path: Path) -> tuple[int, int, int, int] | None:
    try:
        return _path_identity(path)
    except FileNotFoundError:
        return None


def _canonical_path_is_within(root: Path, candidate: Path) -> bool:
    root_value = os.path.normcase(os.path.abspath(root))
    candidate_value = os.path.normcase(os.path.abspath(candidate))
    try:
        return os.path.commonpath((root_value, candidate_value)) == root_value
    except ValueError:
        return False


def _local_action_is_repository_owned(repository_root: Path, relative: str) -> bool:
    if not _repository_action_path_is_bounded(relative, minimum_segments=1):
        return False
    components = [repository_root]
    for segment in relative.split("/"):
        components.append(components[-1] / segment)
    metadata_candidates = (
        components[-1] / "action.yml",
        components[-1] / "action.yaml",
    )
    try:
        before: dict[Path, tuple[int, int, int, int] | None] = {
            path: _path_identity(path) for path in components
        }
        before.update(
            {path: _optional_path_identity(path) for path in metadata_candidates}
        )
        existing_metadata = [
            path for path in metadata_candidates if before[path] is not None
        ]
        if len(existing_metadata) != 1:
            return False
        metadata = existing_metadata[0]
        action_identity = before[components[-1]]
        metadata_identity = before[metadata]
        if (
            action_identity is None
            or metadata_identity is None
            or not stat.S_ISDIR(action_identity[2])
            or not stat.S_ISREG(metadata_identity[2])
        ):
            return False
        resolved_root = repository_root.resolve(strict=True)
        if resolved_root != repository_root or not _canonical_path_is_within(
            resolved_root, components[-1].resolve(strict=True)
        ) or not _canonical_path_is_within(
            resolved_root, metadata.resolve(strict=True)
        ):
            return False
        after = {path: _optional_path_identity(path) for path in before}
        if before != after:
            return False
    except (OSError, RuntimeError, ValueError):
        return False
    return True


def _immutable_action_reference(action: str, *, repository_root: Path) -> bool:
    if PINNED_DOCKER_ACTION_PATTERN.fullmatch(action) is not None:
        return True
    if action.startswith("./"):
        return _local_action_is_repository_owned(repository_root, action[2:])
    match = PINNED_REMOTE_ACTION_PATTERN.fullmatch(action)
    return bool(
        match
        and _repository_action_path_is_bounded(match.group(1), minimum_segments=2)
    )


def _repository_action_path_is_bounded(path: str, *, minimum_segments: int) -> bool:
    segments = path.split("/")
    return len(segments) >= minimum_segments and all(
        segment not in {"", ".", ".."}
        and ACTION_PATH_SEGMENT_PATTERN.fullmatch(segment) is not None
        for segment in segments
    )


def audit_ci_contract(
    workflow: str, *, branch_required_observed: bool | None = None
) -> dict[str, object]:
    """Validate the PR CI receipt step without claiming branch enforcement."""
    lines = workflow.splitlines()
    reasons: list[str] = []
    parsed_policy_steps: object = None
    try:
        parsed_workflow = _load_workflow_yaml(workflow)
        parsed_jobs = parsed_workflow.get("jobs")
        if isinstance(parsed_jobs, Mapping):
            parsed_policy = parsed_jobs.get(CI_POLICY_JOB)
            if isinstance(parsed_policy, Mapping):
                parsed_policy_steps = parsed_policy.get("steps")
    except ValueError:
        reasons.append("workflow_yaml_invalid")
    if parsed_policy_steps != list(EXPECTED_POLICY_STEPS):
        reasons.append("ci_policy_steps_mapping_not_closed")

    def result() -> dict[str, object]:
        return {
            "valid": not reasons,
            "reasons": sorted(set(reasons)),
            "branch_required": branch_required_observed is True,
            "branch_required_evidence": (
                "observed_required"
                if branch_required_observed is True
                else (
                    "observed_not_required"
                    if branch_required_observed is False
                    else "not_observed"
                )
            ),
        }

    if any("\t" in line for line in lines):
        reasons.append("workflow_tabs_not_allowed")
    top_headers: list[tuple[int, str, str]] = []
    for index, line in enumerate(lines):
        if not line.strip() or line.lstrip().startswith("#") or line.startswith((" ", "\t")):
            continue
        match = re.fullmatch(r"([A-Za-z0-9_-]+):(?:\s*(.*))?", line)
        if match:
            top_headers.append((index, match.group(1), (match.group(2) or "").strip()))
        else:
            reasons.append("workflow_top_level_structure_invalid")
    top_keys = tuple(key for _, key, _ in top_headers)
    if top_keys != EXPECTED_CI_TOP_LEVEL_KEYS:
        reasons.append("workflow_top_level_mapping_not_closed")
    top_values = {key: value for _, key, value in top_headers}
    if top_values.get("name") != "CI":
        reasons.append("workflow_name_invalid")

    def top_block(key: str) -> tuple[str, ...]:
        matches = [position for position, (_, name, _) in enumerate(top_headers) if name == key]
        if len(matches) != 1:
            return ()
        position = matches[0]
        start = top_headers[position][0] + 1
        end = top_headers[position + 1][0] if position + 1 < len(top_headers) else len(lines)
        return tuple(
            line.rstrip()
            for line in lines[start:end]
            if line.strip() and not line.lstrip().startswith("#")
        )

    if top_block("on") != EXPECTED_CI_TRIGGER:
        reasons.append("workflow_trigger_invalid")
    if top_block("permissions") != EXPECTED_CI_PERMISSIONS:
        reasons.append("workflow_permissions_invalid")
    if top_block("concurrency") != EXPECTED_CI_CONCURRENCY:
        reasons.append("workflow_concurrency_invalid")
    jobs_headers = [
        index
        for index, line in enumerate(lines)
        if line.split("#", 1)[0].rstrip() == "jobs:" and not line.startswith((" ", "\t"))
    ]
    if len(jobs_headers) != 1:
        reasons.append("jobs_mapping_invalid")
        return result()
    jobs_start = jobs_headers[0] + 1
    jobs_end = next(
        (
            index
            for index in range(jobs_start, len(lines))
            if lines[index].strip()
            and not lines[index].lstrip().startswith("#")
            and not lines[index].startswith((" ", "\t"))
        ),
        len(lines),
    )
    job_headers: list[tuple[int, str]] = []
    for index in range(jobs_start, jobs_end):
        match = re.fullmatch(r"  ([A-Za-z0-9_-]+):(?:\s*#.*)?", lines[index])
        if match:
            job_headers.append((index, match.group(1)))
    matches = [entry for entry in job_headers if entry[1] == CI_POLICY_JOB]
    if len(matches) != 1:
        reasons.append("ci_policy_job_missing")
        return result()
    job_start = matches[0][0]
    job_end = next(
        (index for index, _ in job_headers if index > job_start),
        jobs_end,
    )
    job_fields: dict[str, str] = {}
    job_field_lines: dict[str, int] = {}
    for index in range(job_start + 1, job_end):
        line = lines[index]
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        match = re.fullmatch(r"    ([A-Za-z0-9_-]+):(?:\s*(.*))?", line)
        if match:
            key, value = match.group(1), (match.group(2) or "").strip()
            if key in job_fields:
                reasons.append("ci_policy_job_duplicate_field")
            job_fields[key] = value
            job_field_lines[key] = index
        elif line.startswith("    ") and not line.startswith("      "):
            reasons.append("ci_policy_job_structure_invalid")
    if job_fields != EXPECTED_POLICY_JOB_FIELDS:
        reasons.append("ci_policy_job_mapping_not_closed")
    if job_fields.get("runs-on") != "ubuntu-latest":
        reasons.append("ci_policy_job_runner_invalid")
    if job_fields.get("timeout-minutes") != "10":
        reasons.append("ci_policy_job_timeout_invalid")

    steps: list[dict[str, object]] = []
    current: dict[str, object] | None = None
    steps_start = job_field_lines.get("steps", job_end)
    for index in range(steps_start + 1, job_end):
        line = lines[index]
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        start = re.fullmatch(r"      -(?:\s+([A-Za-z0-9_-]+):(?:\s*(.*))?)?", line)
        if start:
            current = {}
            steps.append(current)
            if start.group(1):
                current[start.group(1)] = (start.group(2) or "").strip()
            continue
        field = re.fullmatch(r"        ([A-Za-z0-9_-]+):(?:\s*(.*))?", line)
        nested = re.fullmatch(r"          ([A-Za-z0-9_-]+):(?:\s*(.*))?", line)
        if field and current is not None:
            key, value = field.group(1), (field.group(2) or "").strip()
            if key in current:
                reasons.append("ci_step_duplicate_field")
            current[key] = {} if key == "with" and value == "" else value
        elif nested and current is not None and isinstance(current.get("with"), dict):
            key, value = nested.group(1), (nested.group(2) or "").strip()
            action_inputs = current["with"]
            if key in action_inputs:
                reasons.append("ci_step_duplicate_input")
            action_inputs[key] = value
        else:
            indentation = len(line) - len(line.lstrip(" "))
            if indentation == 6:
                reasons.append("ci_steps_structure_invalid")
            elif indentation == 8:
                reasons.append("ci_step_structure_invalid")
            elif indentation >= 10 and not (
                current is not None and current.get("run") == "|"
            ):
                reasons.append("ci_step_nested_structure_invalid")
    candidates = [step for step in steps if step.get("run") == CI_RECEIPT_COMMAND]
    if len(candidates) != 1:
        reasons.append("ci_receipt_step_missing")
    else:
        candidate = candidates[0]
        if set(candidate) != {"run"}:
            reasons.append("ci_receipt_step_mapping_not_closed")
    for step in steps:
        execution_keys = [key for key in ("uses", "run") if key in step]
        if len(execution_keys) != 1:
            reasons.append("ci_step_execution_ambiguous")
            continue
        key = execution_keys[0]
        value = step[key]
        if not isinstance(value, str):
            reasons.append("ci_step_execution_ambiguous")
            continue
        allowed_fields = {"uses", "with"} if key == "uses" else {"run"}
        if set(step) - allowed_fields:
            reasons.append("ci_step_execution_modifier_not_allowed")
        if key == "uses":
            expected_inputs = EXPECTED_POLICY_ACTION_INPUTS.get(value)
            actual_inputs = step.get("with", {})
            if expected_inputs is None or actual_inputs != expected_inputs:
                reasons.append("ci_action_inputs_invalid")
    return result()


class _RejectRedirects(urllib.request.HTTPRedirectHandler):
    def redirect_request(
        self,
        request: urllib.request.Request,
        file_pointer: object,
        code: int,
        message: str,
        headers: object,
        new_url: str,
    ) -> None:
        return None


def _open_read_only(request: urllib.request.Request):
    return urllib.request.build_opener(_RejectRedirects()).open(request, timeout=30)


def _request_json(
    url: str,
    *,
    headers: Mapping[str, str],
    expected: Sequence[int] = (200,),
    require_mapping: bool = True,
) -> tuple[int, Any]:
    request = urllib.request.Request(url, headers=dict(headers), method="GET")
    try:
        with _open_read_only(request) as response:
            status = response.status
            body = response.read()
    except urllib.error.HTTPError as error:
        status = error.code
        body = error.read()
    except urllib.error.URLError as error:
        raise ReadinessError("read-only provider request failed") from error
    if status not in expected:
        return status, {}
    try:
        value = json.loads(body)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ReadinessError("read-only provider returned invalid JSON") from error
    if require_mapping and not isinstance(value, dict):
        raise ReadinessError("read-only provider returned an invalid response shape")
    return status, value


def _github_get(
    repository: str,
    path: str,
    token: str,
    *,
    expected: Sequence[int] = (200,),
) -> tuple[int, Any]:
    suffix = path.lstrip("/")
    url = f"https://api.github.com/repos/{repository}"
    if suffix:
        url += f"/{suffix}"
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
        },
        method="GET",
    )
    try:
        with _open_read_only(request) as response:
            status = response.status
            body = response.read()
    except urllib.error.HTTPError as error:
        status = error.code
        body = error.read()
    except urllib.error.URLError as error:
        raise ReadinessError("GitHub read-only recapture failed") from error
    if status not in expected:
        return status, None
    try:
        return status, json.loads(body)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ReadinessError("GitHub returned invalid JSON") from error


def _configuration_names(payload: object, key: str) -> set[str]:
    if not isinstance(payload, Mapping):
        return set()
    values = payload.get(key, [])
    if not isinstance(values, list):
        return set()
    return {
        str(value.get("name"))
        for value in values
        if isinstance(value, Mapping) and isinstance(value.get("name"), str)
    }


def collect_github_snapshot(repository: str, token: str) -> dict[str, Any]:
    if not token:
        raise ReadinessError("GitHub token is required for read-only recapture")
    status, repo = _github_get(repository, "", token)
    if status != 200 or not isinstance(repo, Mapping):
        raise ReadinessError("GitHub repository identity could not be recaptured")
    repository_id = repo.get("id")
    default_branch = str(repo.get("default_branch", ""))
    status, releases = _github_get(repository, "releases?per_page=100", token)
    if status != 200 or not isinstance(releases, list):
        raise ReadinessError("GitHub releases could not be recaptured")
    candidates = [
        release
        for release in releases
        if isinstance(release, Mapping)
        and _release_identity(release.get("tag_name"))[0] is not None
        and release.get("draft") is False
        and release.get("prerelease") is False
    ]
    if not candidates:
        raise ReadinessError("browser extension release identity is missing")
    release = max(candidates, key=lambda item: str(item.get("published_at", "")))
    tag = str(release.get("tag_name", ""))
    release_target = str(release.get("target_commitish", "")).lower()
    status, tag_ref = _github_get(
        repository, f"git/ref/tags/{urllib.parse.quote(tag, safe='')}", token
    )
    tag_object = _mapping(_mapping(tag_ref).get("object"))
    tag_target = str(tag_object.get("sha", "")).lower() if status == 200 else ""
    status, artifacts = _github_get(
        repository,
        "actions/artifacts?name=dcc-cua-browser-extension&per_page=100",
        token,
    )
    artifact_values = _mapping(artifacts).get("artifacts", []) if status == 200 else []
    matching = [
        artifact
        for artifact in artifact_values
        if isinstance(artifact, Mapping)
        and artifact.get("name") == "dcc-cua-browser-extension"
        and str(_mapping(artifact.get("workflow_run")).get("head_sha", "")).lower()
        == release_target
    ] if isinstance(artifact_values, list) else []
    artifact = max(matching, key=lambda item: str(item.get("created_at", ""))) if matching else {}
    artifact_id = artifact.get("id") if isinstance(artifact, Mapping) else None
    if _external_id(artifact_id):
        artifact_status, exact_artifact = _github_get(
            repository, f"actions/artifacts/{artifact_id}", token
        )
        if artifact_status == 200 and isinstance(exact_artifact, Mapping):
            artifact = dict(exact_artifact)
            listed_run = _mapping(exact_artifact.get("workflow_run"))
            run_id = listed_run.get("id")
            if _external_id(run_id):
                run_status, exact_run = _github_get(
                    repository, f"actions/runs/{run_id}", token
                )
                if run_status == 200 and isinstance(exact_run, Mapping):
                    artifact["workflow_run"] = {
                        "id": exact_run.get("id"),
                        "head_sha": exact_run.get("head_sha"),
                        "repository_id": _mapping(exact_run.get("repository")).get("id"),
                        "head_repository_id": _mapping(exact_run.get("head_repository")).get("id"),
                        "status": exact_run.get("status"),
                        "conclusion": exact_run.get("conclusion"),
                        "event": exact_run.get("event"),
                        "path": exact_run.get("path"),
                        "head_branch": exact_run.get("head_branch"),
                    }
    status, environment = _github_get(repository, "environments/browser-stores", token)
    environment = environment if status == 200 and isinstance(environment, Mapping) else {}
    branch_status, branch = _github_get(
        repository,
        f"branches/{urllib.parse.quote(default_branch, safe='')}",
        token,
        expected=(200,),
    )
    default_branch_protected = bool(
        branch_status == 200
        and isinstance(branch, Mapping)
        and branch.get("name") == default_branch
        and branch.get("protected") is True
    )
    default_branch_eligible = False
    policy = _mapping(environment.get("deployment_branch_policy"))
    if _bool(policy.get("custom_branch_policies")):
        policies_status, policies = _github_get(
            repository,
            "environments/browser-stores/deployment-branch-policies?per_page=100",
            token,
        )
        branches = _mapping(policies).get("branch_policies", []) if policies_status == 200 else []
        default_branch_eligible = any(
            isinstance(value, Mapping)
            and value.get("type") == "branch"
            and value.get("name") == default_branch
            for value in branches
        ) if isinstance(branches, list) else False
    names: set[str] = set()
    for path, key in (
        ("environments/browser-stores/variables?per_page=100", "variables"),
        ("environments/browser-stores/secrets?per_page=100", "secrets"),
        ("actions/variables?per_page=100", "variables"),
    ):
        names_status, payload = _github_get(repository, path, token)
        if names_status == 200:
            names.update(_configuration_names(payload, key))
    return {
        "configuration": {
            name: (
                {"present": name in names, "enabled": None}
                if name == "DCC_CUA_BROWSER_STORE_PUBLISH_READY"
                else name in names
            )
            for name in EXPECTED_CONFIGURATION_NAMES
        },
        "environment": {
            "name": environment.get("name"),
            "deployment_branch_policy": environment.get("deployment_branch_policy"),
            "default_branch": default_branch,
            "default_branch_protected": default_branch_protected,
            "default_branch_eligible": default_branch_eligible,
        },
        "github": {
            "repository": {"id": repository_id, "full_name": repo.get("full_name")},
            "source_sha": release_target,
            "tag": tag,
            "tag_target_sha": tag_target,
            "release_id": release.get("id"),
            "release_target_sha": release_target,
            "artifact": artifact,
        },
        "stores": {
            platform: {
                "permission": "not_checked",
                "item": "missing"
                if not names.intersection({ITEM_ID_CONFIGURATION.get(platform, "")})
                and platform in ITEM_ID_CONFIGURATION
                else "unknown",
                "version": "",
                "state": "",
            }
            for platform in ("chrome", "edge", "firefox")
        },
    }


def probe_chrome(environ: Mapping[str, str], expected_version: str) -> dict[str, object]:
    token = environ.get("CHROME_WEBSTORE_ACCESS_TOKEN", "").strip()
    publisher = environ.get("CHROME_WEBSTORE_PUBLISHER_ID", "").strip()
    item = environ.get("CHROME_WEBSTORE_EXTENSION_ID", "").strip()
    if not item:
        return {"permission": "not_checked", "item": "missing", "version": "", "state": ""}
    if not token or not publisher:
        return {"permission": "not_checked", "item": "unknown", "version": "", "state": ""}
    name = f"publishers/{urllib.parse.quote(publisher, safe='')}/items/{urllib.parse.quote(item, safe='')}"
    status, data = _request_json(
        f"https://chromewebstore.googleapis.com/v2/{name}:fetchStatus",
        headers={"Authorization": f"Bearer {token}"},
        expected=(200,),
    )
    if status in (401, 403):
        return {"permission": "denied", "item": "unknown", "version": "", "state": ""}
    if status == 404:
        return {"permission": "granted", "item": "missing", "version": "", "state": ""}
    if status != 200 or str(data.get("itemId", "")) != item:
        return {"permission": "granted", "item": "unknown", "version": "", "state": ""}
    revisions = [
        _mapping(data.get("publishedItemRevisionStatus")),
        _mapping(data.get("submittedItemRevisionStatus")),
    ]
    selected: Mapping[str, Any] = {}
    for revision in revisions:
        channels = revision.get("distributionChannels", [])
        versions = [
            str(_mapping(channel).get("crxVersion", ""))
            for channel in channels
            if isinstance(channel, Mapping)
        ] if isinstance(channels, list) else []
        if expected_version in versions or (not selected and versions):
            selected = {"state": revision.get("state", ""), "version": versions[0] if versions else ""}
        if expected_version in versions:
            selected = {"state": revision.get("state", ""), "version": expected_version}
            break
    return {
        "permission": "granted",
        "item": "exists",
        "version": str(selected.get("version", "")),
        "state": str(selected.get("state", "")).lower(),
        **{
            receipt_name: data.get(provider_name)
            if isinstance(data.get(provider_name), bool)
            else None
            for provider_name, receipt_name in CHROME_REMOVAL_FIELDS.items()
        },
    }


def probe_edge(environ: Mapping[str, str]) -> dict[str, str]:
    if not environ.get("EDGE_ADDONS_PRODUCT_ID", "").strip():
        return {"permission": "not_checked", "item": "missing", "version": "", "state": ""}
    # The documented Edge Update API only offers GET for operation IDs returned by
    # prior mutations. Do not manufacture an upload/publish solely to test access.
    return {"permission": "unverifiable", "item": "unknown", "version": "", "state": ""}


def _base64url(value: bytes) -> str:
    return base64.urlsafe_b64encode(value).rstrip(b"=").decode("ascii")


def _amo_jwt(key: str, secret: str) -> str:
    now = int(time.time())
    header = _base64url(b'{"alg":"HS256","typ":"JWT"}')
    payload = _base64url(
        json.dumps(
            {"iss": key, "jti": secrets.token_hex(16), "iat": now, "exp": now + 60},
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    )
    signing_input = f"{header}.{payload}".encode("ascii")
    signature = _base64url(hmac.new(secret.encode(), signing_input, hashlib.sha256).digest())
    return f"{header}.{payload}.{signature}"


def probe_firefox(environ: Mapping[str, str]) -> dict[str, str]:
    key = environ.get("FIREFOX_AMO_API_KEY", "").strip()
    secret = environ.get("FIREFOX_AMO_API_SECRET", "").strip()
    if not key or not secret:
        return {"permission": "not_checked", "item": "unknown", "version": "", "state": ""}
    profile_status, profile = _request_json(
        "https://addons.mozilla.org/api/v5/accounts/profile/",
        headers={"Authorization": f"JWT {_amo_jwt(key, secret)}"},
        expected=(200,),
    )
    if profile_status in (401, 403):
        return {"permission": "denied", "item": "unknown", "version": "", "state": ""}
    caller_id = profile.get("id") if profile_status == 200 else None
    if not _external_id(caller_id):
        return {"permission": "unverifiable", "item": "unknown", "version": "", "state": ""}
    addon_url = (
        "https://addons.mozilla.org/api/v5/addons/addon/"
        + urllib.parse.quote(FIREFOX_ADDON_ID, safe="")
        + "/"
    )
    status, data = _request_json(
        addon_url,
        headers={"Authorization": f"JWT {_amo_jwt(key, secret)}"},
        expected=(200,),
    )
    if status in (401, 403):
        return {"permission": "denied", "item": "unknown", "version": "", "state": ""}
    if status == 404:
        return {"permission": "granted", "item": "missing", "version": "", "state": ""}
    if status != 200 or str(data.get("guid", "")) != FIREFOX_ADDON_ID:
        return {"permission": "granted", "item": "unknown", "version": "", "state": ""}
    author_status, authors = _request_json(
        addon_url + "authors/",
        headers={"Authorization": f"JWT {_amo_jwt(key, secret)}"},
        expected=(200,),
        require_mapping=False,
    )
    if author_status in (401, 403):
        return {"permission": "denied", "item": "unknown", "version": "", "state": ""}
    if author_status != 200 or not isinstance(authors, list):
        return {"permission": "unverifiable", "item": "unknown", "version": "", "state": ""}
    if not any(
        isinstance(author, Mapping)
        and _external_id(author.get("user_id"))
        and author.get("user_id") == caller_id
        and author.get("role") in {"owner", "developer"}
        for author in authors
    ):
        return {"permission": "unverifiable", "item": "unknown", "version": "", "state": ""}
    version = str(_mapping(data.get("current_version")).get("version", ""))
    return {
        "permission": "granted",
        "item": "exists",
        "version": version,
        "state": str(data.get("status", "")).lower(),
    }


def _load_snapshot(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ReadinessError("snapshot must be a JSON object")
    return value


class _ReceiptArgumentParser(argparse.ArgumentParser):
    def error(self, message: str) -> None:
        raise ReadinessError("invalid command line")


def _failure_receipt() -> dict[str, Any]:
    receipt = build_receipt({})
    receipt["terminal_reason"] = "preflight_failed"
    return receipt


def _requested_output(arguments: Sequence[str]) -> Path | None:
    output: Path | None = None
    index = 0
    while index < len(arguments):
        argument = arguments[index]
        if argument == "--":
            break
        if argument.startswith("--output="):
            output = Path(argument.partition("=")[2])
        elif (
            argument == "--output"
            and index + 1 < len(arguments)
            and not arguments[index + 1].startswith("-")
        ):
            index += 1
            output = Path(arguments[index])
        index += 1
    return output


def _emit_receipt(receipt: Mapping[str, Any], output: Path | None) -> bool:
    serialized = serialize_receipt(receipt)
    file_written = True
    if output:
        try:
            file_written = (
                output.write_text(serialized, encoding="utf-8", newline="\n")
                == len(serialized)
            )
        except (OSError, ValueError):
            file_written = False
    stdout_written = True
    try:
        stdout_written = sys.stdout.write(serialized) == len(serialized)
        sys.stdout.flush()
    except (OSError, ValueError):
        stdout_written = False
    return file_written and stdout_written


def _write_safe_failure_message() -> None:
    try:
        sys.stderr.write("browser store readiness preflight failed\n")
        sys.stderr.flush()
    except (OSError, ValueError):
        pass


def main() -> int:
    output = _requested_output(sys.argv[1:])
    failed = False
    try:
        parser = _ReceiptArgumentParser()
        parser.add_argument("--snapshot", type=Path)
        parser.add_argument("--repository")
        parser.add_argument("--output", type=Path)
        parser.add_argument("--probe-stores", action="store_true")
        args = parser.parse_args()
        output = args.output
        if args.snapshot:
            snapshot = _load_snapshot(args.snapshot)
        elif args.repository:
            snapshot = collect_github_snapshot(
                args.repository, os.environ.get("GITHUB_TOKEN", "").strip()
            )
        else:
            raise ReadinessError("snapshot or repository is required")
        root = Path(__file__).resolve().parent.parent
        snapshot["action_pins"] = audit_action_pins(
            (
                root / ".github" / "workflows" / "browser-store-preflight.yml",
                root / ".github" / "workflows" / "release-please.yml",
            )
        )
        environment = dict(os.environ)
        existing_configuration = _mapping(snapshot.get("configuration"))
        snapshot["configuration"] = {
            name: bool(environment.get(name, "").strip())
            or _bool(existing_configuration.get(name))
            for name in EXPECTED_CONFIGURATION_NAMES
            if name != "DCC_CUA_BROWSER_STORE_PUBLISH_READY"
        }
        publish_name = "DCC_CUA_BROWSER_STORE_PUBLISH_READY"
        publish_value = environment.get(publish_name, "").strip().lower()
        if publish_value:
            snapshot["configuration"][publish_name] = {
                "present": True,
                "enabled": (
                    True
                    if publish_value == "true"
                    else False if publish_value == "false" else None
                ),
            }
        else:
            existing_publish = existing_configuration.get(publish_name)
            snapshot["configuration"][publish_name] = (
                dict(existing_publish)
                if isinstance(existing_publish, Mapping)
                else {"present": _bool(existing_publish), "enabled": None}
            )
        if args.probe_stores:
            _, version = _release_identity(_mapping(snapshot.get("github")).get("tag"))
            snapshot["stores"] = {
                "chrome": probe_chrome(environment, version),
                "edge": probe_edge(environment),
                "firefox": probe_firefox(environment),
            }
        receipt = build_receipt(snapshot)
        exit_code = 0 if receipt["ready"] else 1
    except Exception:
        receipt = _failure_receipt()
        exit_code = 1
        failed = True
    delivered = _emit_receipt(receipt, output)
    if failed or not delivered:
        _write_safe_failure_message()
    return exit_code if delivered else 1


if __name__ == "__main__":
    raise SystemExit(main())
