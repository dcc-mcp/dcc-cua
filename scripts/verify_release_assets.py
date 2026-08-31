#!/usr/bin/env python3
"""Fail closed unless every supported native release target is complete."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path, PurePosixPath

RELEASE_TARGETS: tuple[tuple[str, str], ...] = (
    ("x86_64-pc-windows-msvc", "zip"),
    ("x86_64-unknown-linux-gnu", "tar.gz"),
    ("aarch64-apple-darwin", "tar.gz"),
    ("x86_64-apple-darwin", "tar.gz"),
)
_VERSION_PATTERN = re.compile(r"^\d+\.\d+\.\d+$")
_SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
_PACKAGE_FILES = (
    "LICENSE",
    "THIRD_PARTY_LICENSES.md",
    "README.md",
    "README.zh-CN.md",
    ".mcp.json",
)
_PACKAGE_DIRECTORIES = (
    "assets",
    "skills",
    "plugins",
    ".claude-plugin",
    ".codex-plugin",
)


def _canonical_install_path(raw: object) -> str:
    if not isinstance(raw, str):
        raise ValueError("install manifest path must be a string")
    path = PurePosixPath(raw)
    if (
        not raw
        or "\\" in raw
        or path.is_absolute()
        or any(part in {"", ".", ".."} for part in path.parts)
        or path.as_posix() != raw
    ):
        raise ValueError("install manifest path is not canonical")
    return raw


def _verify_install_manifest(install: object, target: str) -> None:
    if not isinstance(install, dict) or set(install) != {"directories", "files"}:
        raise ValueError("install manifest has an invalid install contract")
    directories = install["directories"]
    files = install["files"]
    if not isinstance(directories, list) or not isinstance(files, list) or not files:
        raise ValueError("install manifest has an invalid file plan")

    canonical_directories = [_canonical_install_path(path) for path in directories]
    if canonical_directories != sorted(set(canonical_directories)):
        raise ValueError("install manifest directories must be unique and sorted")
    directory_roots = {PurePosixPath(path).parts[0] for path in canonical_directories}
    if not set(_PACKAGE_DIRECTORIES).issubset(directory_roots) or any(
        root not in _PACKAGE_DIRECTORIES for root in directory_roots
    ):
        raise ValueError("install manifest contains an invalid package directory")

    expected_binary = "dcc-cua.exe" if "windows" in target else "dcc-cua"
    required_files = {expected_binary, *_PACKAGE_FILES}
    file_paths: list[str] = []
    for entry in files:
        if not isinstance(entry, dict) or set(entry) != {"path", "sha256"}:
            raise ValueError("install manifest has an invalid file entry")
        path = _canonical_install_path(entry["path"])
        digest = entry["sha256"]
        if not isinstance(digest, str) or _SHA256_PATTERN.fullmatch(digest) is None:
            raise ValueError("install manifest has an invalid file digest")
        parts = PurePosixPath(path).parts
        if path not in required_files and parts[0] not in _PACKAGE_DIRECTORIES:
            raise ValueError("install manifest contains an invalid package file")
        if len(parts) > 1 and "/".join(parts[:-1]) not in canonical_directories:
            raise ValueError("install manifest file parent is not declared")
        file_paths.append(path)
    if file_paths != sorted(set(file_paths)) or not required_files.issubset(file_paths):
        raise ValueError("install manifest files must be complete, unique, and sorted")


def expected_asset_names(version: str) -> tuple[str, ...]:
    if _VERSION_PATTERN.fullmatch(version) is None:
        raise ValueError("release version must be stable semver")
    names = []
    for target, extension in RELEASE_TARGETS:
        archive = f"dcc-cua-{version}-{target}.{extension}"
        names.extend(
            (
                archive,
                archive + ".sha256",
                f"dcc-cua-install-manifest-{target}.json",
            )
        )
    return tuple(names)


def verify_release_assets(
    directory: Path, version: str, allowed_extras: tuple[str, ...] = ()
) -> None:
    expected_names = expected_asset_names(version)
    allowed_names = set(expected_names) | set(allowed_extras)
    missing: list[str] = []
    for name in expected_names:
        if not (directory / name).is_file():
            missing.append(name)
    if missing:
        raise ValueError("missing release assets: {}".format(", ".join(missing)))
    unexpected = sorted(
        path.name for path in directory.iterdir() if path.name not in allowed_names
    )
    if unexpected:
        raise ValueError("unexpected release assets: {}".format(", ".join(unexpected)))
    for target, extension in RELEASE_TARGETS:
        archive = directory / f"dcc-cua-{version}-{target}.{extension}"
        digest_builder = hashlib.sha256()
        with archive.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest_builder.update(chunk)
        digest = digest_builder.hexdigest()

        checksum = directory / (archive.name + ".sha256")
        expected_checksum = f"{digest}  {archive.name}\n"
        if checksum.read_text(encoding="utf-8") != expected_checksum:
            raise ValueError(f"checksum does not match archive: {archive.name}")

        manifest_path = directory / f"dcc-cua-install-manifest-{target}.json"
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, ValueError) as exc:
            raise ValueError(f"invalid release manifest: {manifest_path.name}") from exc
        if not isinstance(manifest, dict) or manifest.get("target") != target:
            raise ValueError(f"manifest target does not match asset slot: {target}")
        _verify_install_manifest(manifest.get("install"), target)
        expected_manifest = {
            "schema_version": 1,
            "name": "dcc-cua",
            "version": version,
            "target": target,
            "asset": {
                "name": archive.name,
                "url": "https://github.com/dcc-mcp/dcc-cua/releases/download/"
                f"v{version}/{archive.name}",
                "sha256": digest,
            },
            "install": manifest["install"],
        }
        if manifest != expected_manifest:
            raise ValueError(f"manifest does not match archive: {manifest_path.name}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    verify_release_assets(args.directory, args.version)


if __name__ == "__main__":
    main()
