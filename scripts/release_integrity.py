#!/usr/bin/env python3
"""Verify immutable release sources, workflow artifacts, and provenance."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import stat
import tempfile
import zipfile
from pathlib import Path, PurePosixPath

try:
    from scripts.verify_release_assets import (
        RELEASE_TARGETS,
        expected_asset_names,
        verify_release_assets,
    )
except ModuleNotFoundError:  # Direct execution from the scripts directory.
    from verify_release_assets import (
        RELEASE_TARGETS,
        expected_asset_names,
        verify_release_assets,
    )

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


def native_build_artifact_facts(
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
    native_names = {name for name in by_name if name.startswith("dcc-cua-native-")}
    if native_names != set(EXPECTED_BUILD_ARTIFACTS):
        raise ValueError("native build artifact set does not match the release matrix")

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


def verify_and_extract_artifact(
    archive: Path, expected_digest: str, output_directory: Path
) -> None:
    """Verify exact artifact ZIP transport bytes before extracting any content."""
    normalized_digest = _normalize_digest(expected_digest)
    if _sha256(archive) != normalized_digest:
        raise ValueError(
            "artifact transport digest does not match the reviewed identity"
        )

    try:
        with zipfile.ZipFile(archive) as bundle:
            members = _validated_zip_members(bundle, "artifact archive")

            output_directory.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.TemporaryDirectory(
                prefix="dcc-cua-artifact-", dir=output_directory.parent
            ) as temporary:
                staged = Path(temporary)
                for member in members:
                    relative = PurePosixPath(member.filename.replace("\\", "/"))
                    destination = staged.joinpath(*relative.parts)
                    if member.is_dir():
                        destination.mkdir(parents=True, exist_ok=True)
                        continue
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    with (
                        bundle.open(member) as source,
                        destination.open("wb") as target,
                    ):
                        shutil.copyfileobj(source, target)

                staged_paths = sorted(
                    staged.rglob("*"),
                    key=lambda path: (not path.is_dir(), len(path.parts), str(path)),
                )
                for staged_path in staged_paths:
                    relative = staged_path.relative_to(staged)
                    destination = output_directory / relative
                    if staged_path.is_dir():
                        if destination.exists() and not destination.is_dir():
                            raise ValueError("artifact extraction would replace a file")
                    elif destination.exists():
                        raise ValueError(
                            "artifact extraction would replace existing content"
                        )

                output_directory.mkdir(parents=True, exist_ok=True)
                for staged_path in staged_paths:
                    relative = staged_path.relative_to(staged)
                    destination = output_directory / relative
                    if staged_path.is_dir():
                        destination.mkdir(parents=True, exist_ok=True)
                    else:
                        destination.parent.mkdir(parents=True, exist_ok=True)
                        shutil.move(str(staged_path), destination)
    except (OSError, zipfile.BadZipFile) as exc:
        raise ValueError("invalid artifact transport archive") from exc


def _validated_zip_members(
    archive: zipfile.ZipFile, description: str
) -> list[zipfile.ZipInfo]:
    members = archive.infolist()
    if not any(not member.is_dir() for member in members):
        raise ValueError(f"{description} must contain at least one regular file")
    seen = set()
    for member in members:
        raw_name = member.filename.replace("\\", "/")
        relative = PurePosixPath(raw_name)
        if (
            not raw_name
            or relative.is_absolute()
            or any(part in ("", ".", "..") for part in relative.parts)
        ):
            raise ValueError(f"{description} contains an unsafe path")
        identity = relative.as_posix().casefold()
        if identity in seen:
            raise ValueError(f"{description} contains duplicate paths")
        seen.add(identity)
        unix_mode = member.external_attr >> 16
        if stat.S_IFMT(unix_mode) == stat.S_IFLNK:
            raise ValueError(f"{description} contains a symbolic link")
    return members


def extension_asset_names(version: str) -> tuple[str, ...]:
    if _VERSION_PATTERN.fullmatch(version) is None:
        raise ValueError("browser extension version must use stable semver")
    return (
        f"dcc-cua-browser-extension-{version}-chrome.zip",
        f"dcc-cua-browser-extension-{version}-edge.zip",
        f"dcc-cua-browser-extension-{version}-firefox.zip",
        f"dcc-cua-browser-extension-{version}-firefox-sources.zip",
    )


def _is_regular_unlinked_file(path: Path) -> bool:
    status = path.lstat()
    file_attributes = getattr(status, "st_file_attributes", 0)
    reparse_flag = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
    return stat.S_ISREG(status.st_mode) and not (file_attributes & reparse_flag)


def verify_extension_asset_set(directory: Path, version: str) -> None:
    """Validate the exact browser asset set and inner ZIP bytes without writes."""
    expected_names = extension_asset_names(version)
    try:
        entries = list(directory.iterdir())
        if {path.name for path in entries} != set(expected_names) or any(
            not _is_regular_unlinked_file(path) for path in entries
        ):
            raise ValueError("browser extension asset set has missing or extra entries")
        for name in expected_names:
            path = directory / name
            with zipfile.ZipFile(path) as archive:
                members = _validated_zip_members(
                    archive, f"browser extension asset {name}"
                )
                for member in members:
                    if member.is_dir():
                        continue
                    with archive.open(member) as source:
                        while source.read(1024 * 1024):
                            pass
    except ValueError:
        raise
    except (OSError, RuntimeError, zipfile.BadZipFile) as exc:
        raise ValueError("browser extension asset set contains an invalid ZIP") from exc


def _verify_published_assets(
    metadata: object,
    directory: Path,
    *,
    expected_names: tuple[str, ...],
    expected_tag: str,
    expected_target_sha: str,
) -> None:
    _require_sha(expected_target_sha, "expected published release target")
    local_entries = list(directory.iterdir())
    local_names = {path.name for path in local_entries if path.is_file()}
    if local_names != set(expected_names) or any(
        not path.is_file() for path in local_entries
    ):
        raise ValueError("published release local asset set has missing or extra files")
    if not isinstance(metadata, dict):
        raise TypeError("published release metadata must be an object")
    if metadata.get("tagName") != expected_tag:
        raise ValueError("published release tag does not match")
    if metadata.get("targetCommitish") != expected_target_sha:
        raise ValueError("published release target does not match the source commit")
    assets = metadata.get("assets")
    if not isinstance(assets, list):
        raise TypeError("published release assets must be a list")
    by_name = {}
    for asset in assets:
        if not isinstance(asset, dict) or not isinstance(asset.get("name"), str):
            raise TypeError("published release asset metadata is malformed")
        if asset["name"] in by_name:
            raise ValueError("published release asset names must be unique")
        by_name[asset["name"]] = asset
    if set(by_name) != set(expected_names):
        raise ValueError("published release asset set has missing or extra files")
    for name in expected_names:
        path = directory / name
        if not path.is_file():
            raise ValueError("published release local asset set is incomplete")
        asset = by_name[name]
        if asset.get("state") != "uploaded":
            raise ValueError("published release asset is not terminally uploaded")
        if asset.get("size") != path.stat().st_size:
            raise ValueError("published release asset size does not match")
        if _normalize_digest(asset.get("digest")) != _sha256(path):
            raise ValueError("published release asset digest does not match")


def verify_published_native_release(
    metadata: object,
    directory: Path,
    *,
    version: str,
    tag: str,
    source_sha: str,
    actual_latest_tag: str,
) -> None:
    verify_release_provenance(
        directory,
        version=version,
        tag=tag,
        source_sha=source_sha,
        release_target_sha=source_sha,
        provenance_path=directory / PROVENANCE_NAME,
    )
    _verify_published_assets(
        metadata,
        directory,
        expected_names=(*expected_asset_names(version), PROVENANCE_NAME),
        expected_tag=tag,
        expected_target_sha=source_sha,
    )
    if actual_latest_tag != tag:
        raise ValueError("native GitHub Release must remain Latest")


def verify_published_extension_release(
    metadata: object,
    directory: Path,
    *,
    version: str,
    tag: str,
    source_sha: str,
    expected_latest_tag: str,
    actual_latest_tag: str,
) -> None:
    verify_extension_asset_set(directory, version)
    _verify_published_assets(
        metadata,
        directory,
        expected_names=extension_asset_names(version),
        expected_tag=tag,
        expected_target_sha=source_sha,
    )
    if actual_latest_tag != expected_latest_tag or actual_latest_tag == tag:
        raise ValueError("browser extension release must not replace native Latest")


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
        "build_artifacts": native_build_artifact_facts(
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

    extract = subparsers.add_parser("verify-extract")
    extract.add_argument("--archive", type=Path, required=True)
    extract.add_argument("--expected-digest", required=True)
    extract.add_argument("--output", type=Path, required=True)

    extension_assets = subparsers.add_parser("verify-extension-assets")
    extension_assets.add_argument("--directory", type=Path, required=True)
    extension_assets.add_argument("--version", required=True)

    native_plan = subparsers.add_parser("write-native-plan")
    native_plan.add_argument("--metadata", type=Path, required=True)
    native_plan.add_argument("--expected-run-id", type=int, required=True)
    native_plan.add_argument("--expected-head-sha", required=True)
    native_plan.add_argument("--output", type=Path, required=True)

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

    published_native = subparsers.add_parser("verify-published-native")
    published_native.add_argument("--metadata", type=Path, required=True)
    published_native.add_argument("--directory", type=Path, required=True)
    published_native.add_argument("--version", required=True)
    published_native.add_argument("--tag", required=True)
    published_native.add_argument("--source-sha", required=True)
    published_native.add_argument("--actual-latest-tag", required=True)

    published_extension = subparsers.add_parser("verify-published-extension")
    published_extension.add_argument("--metadata", type=Path, required=True)
    published_extension.add_argument("--directory", type=Path, required=True)
    published_extension.add_argument("--version", required=True)
    published_extension.add_argument("--tag", required=True)
    published_extension.add_argument("--source-sha", required=True)
    published_extension.add_argument("--expected-latest-tag", required=True)
    published_extension.add_argument("--actual-latest-tag", required=True)

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
    elif args.command == "verify-extract":
        verify_and_extract_artifact(args.archive, args.expected_digest, args.output)
    elif args.command == "verify-extension-assets":
        verify_extension_asset_set(args.directory, args.version)
    elif args.command == "write-native-plan":
        facts = native_build_artifact_facts(
            _load_json(args.metadata),
            workflow_run_id=args.expected_run_id,
            source_sha=args.expected_head_sha,
        )
        args.output.write_text(
            json.dumps(facts, indent=2, sort_keys=True) + "\n", encoding="utf-8"
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
    elif args.command == "verify-provenance":
        verify_release_provenance(
            args.directory,
            version=args.version,
            tag=args.tag,
            source_sha=args.source_sha,
            release_target_sha=args.release_target_sha,
            provenance_path=args.provenance,
        )
    elif args.command == "verify-published-native":
        verify_published_native_release(
            _load_json(args.metadata),
            args.directory,
            version=args.version,
            tag=args.tag,
            source_sha=args.source_sha,
            actual_latest_tag=args.actual_latest_tag,
        )
    else:
        verify_published_extension_release(
            _load_json(args.metadata),
            args.directory,
            version=args.version,
            tag=args.tag,
            source_sha=args.source_sha,
            expected_latest_tag=args.expected_latest_tag,
            actual_latest_tag=args.actual_latest_tag,
        )


if __name__ == "__main__":
    main()
