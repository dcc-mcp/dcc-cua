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
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Mapping, Sequence
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


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
KNOWN_ITEM_STATES = {
    "published",
    "pending_review",
    "staged",
    "published_to_testers",
    "in_review",
    "in_store",
    "approved",
    "public",
    "unlisted",
    "listed",
}
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
DIGEST_PATTERN = re.compile(r"^sha256:[0-9a-f]{64}$")
REMOTE_ACTION_PATTERN = re.compile(r"^\s*(?:-\s*)?uses:\s*([^\s#]+)", re.MULTILINE)
PINNED_ACTION_PATTERN = re.compile(r"^[^@\s]+@[0-9a-f]{40}$")
FIREFOX_ADDON_ID = "dcc-cua@dcc-mcp.org"


class ReadinessError(RuntimeError):
    """A provider-safe preflight failure."""


def _bool(value: object) -> bool:
    return value is True


def _mapping(value: object) -> Mapping[str, Any]:
    return value if isinstance(value, Mapping) else {}


def _environment_receipt(environment: Mapping[str, Any]) -> dict[str, object]:
    policy = _mapping(environment.get("deployment_branch_policy"))
    protected_only = _bool(policy.get("protected_branches"))
    custom = _bool(policy.get("custom_branch_policies"))
    default_protected = _bool(environment.get("default_branch_protected"))
    valid = environment.get("name") == "browser-stores"
    reason = "eligible"
    if not valid:
        reason = "environment_missing"
    elif protected_only and not custom and not default_protected:
        valid = False
        reason = "default_branch_not_eligible"
    elif custom:
        valid = _bool(environment.get("default_branch_eligible"))
        reason = "eligible" if valid else "default_branch_not_eligible"
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
    if not all(SHA_PATTERN.fullmatch(value) for value in (source, tag_target, release_target, run_head)):
        reasons.append("invalid_source_identity")
    elif len({source, tag_target, release_target, run_head}) != 1:
        reasons.append("source_head_drift")
    repository_id = repository.get("id")
    if not isinstance(repository_id, int) or repository_id < 1:
        reasons.append("invalid_repository_identity")
    if run.get("repository_id") != repository_id or run.get("head_repository_id") != repository_id:
        reasons.append("artifact_repository_mismatch")
    artifact_id = artifact.get("id")
    run_id = run.get("id")
    release_id = github.get("release_id")
    if not all(isinstance(value, int) and value > 0 for value in (artifact_id, run_id, release_id)):
        reasons.append("invalid_numeric_identity")
    if artifact.get("name") != "dcc-cua-browser-extension":
        reasons.append("artifact_name_mismatch")
    digest = str(artifact.get("digest", "")).lower()
    if DIGEST_PATTERN.fullmatch(digest) is None:
        reasons.append("artifact_digest_mismatch")
    if _bool(artifact.get("expired")):
        reasons.append("artifact_expired")
    expires_at = str(artifact.get("expires_at", ""))
    try:
        expires = datetime.fromisoformat(expires_at.replace("Z", "+00:00"))
        if expires <= datetime.now(timezone.utc):
            reasons.append("artifact_expired")
    except ValueError:
        reasons.append("invalid_artifact_expiry")
    reasons = sorted(set(reasons))
    return {
        "valid": not reasons,
        "reasons": reasons,
        "repository_id": repository_id if isinstance(repository_id, int) else None,
        "source_sha": source if SHA_PATTERN.fullmatch(source) else None,
        "tag": str(github.get("tag", "")),
        "release_id": release_id if isinstance(release_id, int) else None,
        "artifact_id": artifact_id if isinstance(artifact_id, int) else None,
        "artifact_digest": digest if DIGEST_PATTERN.fullmatch(digest) else None,
        "run_id": run_id if isinstance(run_id, int) else None,
        "head_sha": run_head if SHA_PATTERN.fullmatch(run_head) else None,
        "expired": _bool(artifact.get("expired")),
        "expires_at": expires_at or None,
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
    result: dict[str, object] = {
        "ready": False,
        "state": "not_ready",
        "reason": "missing_configuration" if missing else "readback_missing",
        "missing_configuration": missing,
        "configuration_present": len(PLATFORM_CONFIGURATION[platform]) - len(missing),
        "configuration_expected": len(PLATFORM_CONFIGURATION[platform]),
        "read_permission": str(observation.get("permission", "not_checked")),
        "item_exists": observation.get("item") == "exists",
        "version": None,
        "item_state": None,
    }
    item_name = ITEM_ID_CONFIGURATION.get(platform)
    if item_name in missing:
        result.update(state="human_action_required", reason="item_onboarding_required")
        return result
    if missing:
        return result
    permission = observation.get("permission")
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
    version = str(observation.get("version", ""))
    state = str(observation.get("state", "")).lower()
    result["version"] = version or None
    result["item_state"] = state if state in KNOWN_ITEM_STATES else None
    if state not in KNOWN_ITEM_STATES:
        result["reason"] = "unknown_item_state"
        return result
    if version != expected_version:
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
    tag = str(_mapping(snapshot.get("github")).get("tag", ""))
    match = re.fullmatch(r"dcc-cua-browser-extension-v(.+)", tag)
    expected_version = match.group(1) if match else ""
    observations = _mapping(snapshot.get("stores"))
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
        "publishing_enabled": False,
        "configuration": {
            "expected_names": list(EXPECTED_CONFIGURATION_NAMES),
            "present_count": sum(
                1 for name in EXPECTED_CONFIGURATION_NAMES if _bool(configuration.get(name))
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
        text = path.read_text(encoding="utf-8")
        for match in REMOTE_ACTION_PATTERN.finditer(text):
            action = match.group(1)
            if action.startswith("./"):
                continue
            if PINNED_ACTION_PATTERN.fullmatch(action) is None:
                unpinned.append(f"{path.name}:{action}")
    return {"valid": not unpinned, "unpinned": sorted(unpinned)}


def _request_json(
    url: str,
    *,
    headers: Mapping[str, str],
    expected: Sequence[int] = (200,),
) -> tuple[int, dict[str, Any]]:
    request = urllib.request.Request(url, headers=dict(headers), method="GET")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
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
    if not isinstance(value, dict):
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
        with urllib.request.urlopen(request, timeout=30) as response:
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
        and str(release.get("tag_name", "")).startswith("dcc-cua-browser-extension-v")
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
    if isinstance(artifact_id, int) and artifact_id > 0:
        artifact_status, exact_artifact = _github_get(
            repository, f"actions/artifacts/{artifact_id}", token
        )
        if artifact_status == 200 and isinstance(exact_artifact, Mapping):
            artifact = dict(exact_artifact)
            listed_run = _mapping(exact_artifact.get("workflow_run"))
            run_id = listed_run.get("id")
            if isinstance(run_id, int) and run_id > 0:
                run_status, exact_run = _github_get(
                    repository, f"actions/runs/{run_id}", token
                )
                if run_status == 200 and isinstance(exact_run, Mapping):
                    artifact["workflow_run"] = {
                        "id": exact_run.get("id"),
                        "head_sha": exact_run.get("head_sha"),
                        "repository_id": _mapping(exact_run.get("repository")).get("id"),
                        "head_repository_id": _mapping(exact_run.get("head_repository")).get("id"),
                    }
    status, environment = _github_get(repository, "environments/browser-stores", token)
    environment = environment if status == 200 and isinstance(environment, Mapping) else {}
    branch_status, _ = _github_get(
        repository,
        f"branches/{urllib.parse.quote(default_branch, safe='')}/protection",
        token,
        expected=(200,),
    )
    default_branch_protected = branch_status == 200
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
        "configuration": {name: name in names for name in EXPECTED_CONFIGURATION_NAMES},
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


def probe_chrome(environ: Mapping[str, str], expected_version: str) -> dict[str, str]:
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
    status, data = _request_json(
        "https://addons.mozilla.org/api/v5/addons/addon/"
        + urllib.parse.quote(FIREFOX_ADDON_ID, safe="")
        + "/",
        headers={"Authorization": f"JWT {_amo_jwt(key, secret)}", "Content-Type": "application/json"},
        expected=(200,),
    )
    if status in (401, 403):
        return {"permission": "denied", "item": "unknown", "version": "", "state": ""}
    if status == 404:
        return {"permission": "granted", "item": "missing", "version": "", "state": ""}
    if status != 200 or str(data.get("guid", "")) != FIREFOX_ADDON_ID:
        return {"permission": "granted", "item": "unknown", "version": "", "state": ""}
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


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--snapshot", type=Path)
    parser.add_argument("--repository")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--probe-stores", action="store_true")
    args = parser.parse_args()
    if args.snapshot:
        snapshot = _load_snapshot(args.snapshot)
    elif args.repository:
        snapshot = collect_github_snapshot(
            args.repository, os.environ.get("GITHUB_TOKEN", "").strip()
        )
    else:
        parser.error("one of --snapshot or --repository is required")
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
    }
    if args.probe_stores:
        tag = str(_mapping(snapshot.get("github")).get("tag", ""))
        match = re.fullmatch(r"dcc-cua-browser-extension-v(.+)", tag)
        version = match.group(1) if match else ""
        snapshot["stores"] = {
            "chrome": probe_chrome(environment, version),
            "edge": probe_edge(environment),
            "firefox": probe_firefox(environment),
        }
    receipt = build_receipt(snapshot)
    serialized = serialize_receipt(receipt)
    if args.output:
        args.output.write_text(serialized, encoding="utf-8", newline="\n")
    print(serialized, end="")
    return 0 if receipt["ready"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
