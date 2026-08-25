#!/usr/bin/env python3
"""Verify immutable release sources, workflow artifacts, and provenance."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path

try:
    from scripts.verify_release_assets import RELEASE_TARGETS, verify_release_assets
except ModuleNotFoundError:  # Direct execution from the scripts directory.
    from verify_release_assets import RELEASE_TARGETS, verify_release_assets

PROVENANCE_NAME = "dcc-cua-release-provenance-v1.json"
EXPECTED_BUILD_ARTIFACTS = (
    "dcc-cua-native-windows-x86_64",
    "dcc-cua-native-linux-x86_64",
    "dcc-cua-native-macos-aarch64",
    "dcc-cua-native-macos-x86_64",
)

_SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
_DIGEST_PATTERN = re.compile(r"^[0-9a-f]{64}$")
_VERSION_PATTERN = re.compile(r"^\d+\.\d+\.\d+$")
_SIGNING_FACT = {"status": "not_performed", "verification": "sha256_only"}
_RELEASE_COMPONENTS = (
    (".", "v"),
    ("browser-extension/chrome", "dcc-cua-browser-extension-v"),
)


def _require_sha(value: str, name: str) -> None:
    if _SHA_PATTERN.fullmatch(value) is None:
        raise ValueError(f"{name} must be a lowercase 40-character commit SHA")


def _normalize_digest(value: object) -> str:
    if not isinstance(value, str):
        raise TypeError("artifact digest must be a SHA-256 string")
    digest = value.removeprefix("sha256:").lower()
    if _DIGEST_PATTERN.fullmatch(digest) is None:
        raise ValueError("artifact digest must be a SHA-256 string")
    return digest


def changed_release_tags(before: object, after: object) -> tuple[str, ...]:
    """Return newly versioned component tags from one release-manifest change."""
    expected_keys = {key for key, _ in _RELEASE_COMPONENTS}
    if (
        not isinstance(before, dict)
        or not isinstance(after, dict)
        or set(before) != expected_keys
        or set(after) != expected_keys
    ):
        raise ValueError(
            "release manifest must contain exactly the reviewed components"
        )

    tags = []
    for key, prefix in _RELEASE_COMPONENTS:
        old_version = before.get(key)
        new_version = after.get(key)
        if (
            not isinstance(old_version, str)
            or not isinstance(new_version, str)
            or _VERSION_PATTERN.fullmatch(old_version) is None
            or _VERSION_PATTERN.fullmatch(new_version) is None
        ):
            raise ValueError("release manifest versions must use stable semver")
        if old_version == new_version:
            continue
        old_parts = tuple(int(part) for part in old_version.split("."))
        new_parts = tuple(int(part) for part in new_version.split("."))
        if new_parts <= old_parts:
            raise ValueError("release manifest version must increase")
        tags.append(f"{prefix}{new_version}")
    return tuple(tags)


def verify_release_source(
    *, head_sha: str, tag_sha: str, release_target: str, expected_sha: str
) -> None:
    for name, value in (
        ("head SHA", head_sha),
        ("tag SHA", tag_sha),
        ("release target", release_target),
        ("expected SHA", expected_sha),
    ):
        _require_sha(value, name)
    if {head_sha, tag_sha, release_target} != {expected_sha}:
        raise ValueError(
            "HEAD, peeled tag, and release target must bind one exact release source"
        )


def verify_workflow_artifact(
    metadata: object,
    *,
    expected_id: int,
    expected_digest: str,
    expected_name: str,
    expected_run_id: int,
    expected_head_sha: str,
) -> None:
    _require_sha(expected_head_sha, "expected artifact head SHA")
    if not isinstance(metadata, dict):
        raise TypeError("artifact metadata must be an object")
    if not isinstance(expected_id, int) or expected_id <= 0:
        raise ValueError("expected artifact ID must be positive")
    if metadata.get("id") != expected_id:
        raise ValueError("artifact ID does not match the immutable workflow output")
    if metadata.get("name") != expected_name:
        raise ValueError("artifact name does not match the immutable workflow output")
    if metadata.get("expired") is not False:
        raise ValueError("workflow artifact is expired or has unknown expiry state")
    if _normalize_digest(metadata.get("digest")) != _normalize_digest(expected_digest):
        raise ValueError("artifact digest does not match the immutable workflow output")
    workflow_run = metadata.get("workflow_run")
    if not isinstance(workflow_run, dict):
        raise TypeError("artifact metadata has no owning workflow run")
    if workflow_run.get("id") != expected_run_id:
        raise ValueError("artifact belongs to a different workflow run")
    if workflow_run.get("head_sha") != expected_head_sha:
        raise ValueError("artifact belongs to a different source commit")


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _asset_facts(directory: Path, version: str) -> list[dict]:
    facts = []
    for target, extension in RELEASE_TARGETS:
        archive = directory / f"dcc-cua-{version}-{target}.{extension}"
        facts.append(
            {
                "target": target,
                "archive": archive.name,
                "size": archive.stat().st_size,
                "sha256": _sha256(archive),
                "checksum": archive.name + ".sha256",
                "install_manifest": f"dcc-cua-install-manifest-{target}.json",
                "signing": "not_performed",
            }
        )
    return facts


def _build_artifact_facts(
    metadata: object, *, workflow_run_id: int, source_sha: str
) -> list[dict]:
    if not isinstance(metadata, dict) or not isinstance(
        metadata.get("artifacts"), list
    ):
        raise TypeError("workflow artifact listing must contain an artifacts list")
    artifacts = metadata["artifacts"]
    if metadata.get("total_count") != len(artifacts):
        raise ValueError("workflow artifact total does not match its listing")
    by_name = {}
    for artifact in artifacts:
        if not isinstance(artifact, dict) or not isinstance(artifact.get("name"), str):
            raise TypeError("workflow artifact entry is malformed")
        if artifact["name"] in by_name:
            raise ValueError("workflow artifact names must be unique")
        by_name[artifact["name"]] = artifact
    if not set(EXPECTED_BUILD_ARTIFACTS).issubset(by_name):
        raise ValueError(
            "workflow artifact listing does not contain the native build matrix"
        )

    facts = []
    for name in EXPECTED_BUILD_ARTIFACTS:
        artifact = by_name[name]
        artifact_id = artifact.get("id")
        if not isinstance(artifact_id, int) or artifact_id <= 0:
            raise ValueError("workflow artifact ID must be positive")
        digest = _normalize_digest(artifact.get("digest"))
        verify_workflow_artifact(
            artifact,
            expected_id=artifact_id,
            expected_digest=digest,
            expected_name=name,
            expected_run_id=workflow_run_id,
            expected_head_sha=source_sha,
        )
        facts.append(
            {
                "name": name,
                "id": artifact_id,
                "sha256": digest,
                "workflow_run_id": workflow_run_id,
                "head_sha": source_sha,
            }
        )
    return facts


def build_release_provenance(
    directory: Path,
    *,
    version: str,
    tag: str,
    source_sha: str,
    release_target_sha: str,
    workflow_run_id: int,
    workflow_artifacts: object,
) -> dict:
    if _VERSION_PATTERN.fullmatch(version) is None or tag != f"v{version}":
        raise ValueError("release tag and version must identify one stable release")
    verify_release_source(
        head_sha=source_sha,
        tag_sha=source_sha,
        release_target=release_target_sha,
        expected_sha=source_sha,
    )
    if not isinstance(workflow_run_id, int) or workflow_run_id <= 0:
        raise ValueError("workflow run ID must be positive")
    verify_release_assets(directory, version)
    return {
        "schema_version": 1,
        "name": "dcc-cua",
        "version": version,
        "tag": tag,
        "source_commit": source_sha,
        "release_target_commit": release_target_sha,
        "workflow_run_id": workflow_run_id,
        "signing": dict(_SIGNING_FACT),
        "assets": _asset_facts(directory, version),
        "build_artifacts": _build_artifact_facts(
            workflow_artifacts,
            workflow_run_id=workflow_run_id,
            source_sha=source_sha,
        ),
    }


def write_release_provenance(path: Path, document: dict) -> None:
    path.write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def verify_release_provenance(
    directory: Path,
    *,
    version: str,
    tag: str,
    source_sha: str,
    release_target_sha: str,
    provenance_path: Path,
) -> None:
    try:
        document = json.loads(provenance_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, ValueError) as exc:
        raise ValueError("invalid release provenance") from exc
    verify_release_assets(directory, version, allowed_extras=(PROVENANCE_NAME,))
    expected_scalars = {
        "schema_version": 1,
        "name": "dcc-cua",
        "version": version,
        "tag": tag,
        "source_commit": source_sha,
        "release_target_commit": release_target_sha,
        "signing": _SIGNING_FACT,
    }
    if not isinstance(document, dict) or any(
        document.get(key) != value for key, value in expected_scalars.items()
    ):
        raise ValueError("release provenance identity or signing fact does not match")
    verify_release_source(
        head_sha=source_sha,
        tag_sha=source_sha,
        release_target=release_target_sha,
        expected_sha=source_sha,
    )
    if document.get("assets") != _asset_facts(directory, version):
        raise ValueError("release provenance asset facts do not match release contents")
    workflow_run_id = document.get("workflow_run_id")
    build_artifacts = document.get("build_artifacts")
    if not isinstance(workflow_run_id, int) or workflow_run_id <= 0:
        raise ValueError("release provenance workflow run is invalid")
    if not isinstance(build_artifacts, list) or len(build_artifacts) != len(
        EXPECTED_BUILD_ARTIFACTS
    ):
        raise ValueError("release provenance build artifact facts are incomplete")
    names = []
    ids = []
    for artifact in build_artifacts:
        if not isinstance(artifact, dict):
            raise TypeError("release provenance build artifact fact is malformed")
        names.append(artifact.get("name"))
        ids.append(artifact.get("id"))
        if artifact.get("workflow_run_id") != workflow_run_id:
            raise ValueError("release provenance build artifact run does not match")
        if artifact.get("head_sha") != source_sha:
            raise ValueError("release provenance build artifact source does not match")
        if not isinstance(artifact.get("id"), int) or artifact["id"] <= 0:
            raise ValueError("release provenance build artifact ID is invalid")
        _normalize_digest(artifact.get("sha256"))
    if names != list(EXPECTED_BUILD_ARTIFACTS) or len(set(ids)) != len(ids):
        raise ValueError("release provenance build artifact identity does not match")


def _load_json(path: Path) -> object:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, ValueError) as exc:
        raise ValueError(f"invalid JSON input: {path.name}") from exc


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    source = subparsers.add_parser("verify-source")
    source.add_argument("--head-sha", required=True)
    source.add_argument("--tag-sha", required=True)
    source.add_argument("--release-target", required=True)
    source.add_argument("--expected-sha", required=True)

    changed_tags = subparsers.add_parser("changed-tags")
    changed_tags.add_argument("--before", type=Path, required=True)
    changed_tags.add_argument("--after", type=Path, required=True)

    artifact = subparsers.add_parser("verify-artifact")
    artifact.add_argument("--metadata", type=Path, required=True)
    artifact.add_argument("--expected-id", type=int, required=True)
    artifact.add_argument("--expected-digest", required=True)
    artifact.add_argument("--expected-name", required=True)
    artifact.add_argument("--expected-run-id", type=int, required=True)
    artifact.add_argument("--expected-head-sha", required=True)

    provenance = subparsers.add_parser("write-provenance")
    provenance.add_argument("--directory", type=Path, required=True)
    provenance.add_argument("--version", required=True)
    provenance.add_argument("--tag", required=True)
    provenance.add_argument("--source-sha", required=True)
    provenance.add_argument("--release-target-sha", required=True)
    provenance.add_argument("--workflow-run-id", type=int, required=True)
    provenance.add_argument("--workflow-artifacts", type=Path, required=True)
    provenance.add_argument("--output", type=Path, required=True)

    verify = subparsers.add_parser("verify-provenance")
    verify.add_argument("--directory", type=Path, required=True)
    verify.add_argument("--version", required=True)
    verify.add_argument("--tag", required=True)
    verify.add_argument("--source-sha", required=True)
    verify.add_argument("--release-target-sha", required=True)
    verify.add_argument("--provenance", type=Path, required=True)

    args = parser.parse_args()
    if args.command == "changed-tags":
        for tag in changed_release_tags(
            _load_json(args.before), _load_json(args.after)
        ):
            print(tag)
    elif args.command == "verify-source":
        verify_release_source(
            head_sha=args.head_sha,
            tag_sha=args.tag_sha,
            release_target=args.release_target,
            expected_sha=args.expected_sha,
        )
    elif args.command == "verify-artifact":
        verify_workflow_artifact(
            _load_json(args.metadata),
            expected_id=args.expected_id,
            expected_digest=args.expected_digest,
            expected_name=args.expected_name,
            expected_run_id=args.expected_run_id,
            expected_head_sha=args.expected_head_sha,
        )
    elif args.command == "write-provenance":
        document = build_release_provenance(
            args.directory,
            version=args.version,
            tag=args.tag,
            source_sha=args.source_sha,
            release_target_sha=args.release_target_sha,
            workflow_run_id=args.workflow_run_id,
            workflow_artifacts=_load_json(args.workflow_artifacts),
        )
        write_release_provenance(args.output, document)
    else:
        verify_release_provenance(
            args.directory,
            version=args.version,
            tag=args.tag,
            source_sha=args.source_sha,
            release_target_sha=args.release_target_sha,
            provenance_path=args.provenance,
        )


if __name__ == "__main__":
    main()
