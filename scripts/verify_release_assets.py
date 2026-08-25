#!/usr/bin/env python3
"""Fail closed unless every supported native release target is complete."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path

RELEASE_TARGETS: tuple[tuple[str, str], ...] = (
    ("x86_64-pc-windows-msvc", "zip"),
    ("x86_64-unknown-linux-gnu", "tar.gz"),
    ("aarch64-apple-darwin", "tar.gz"),
    ("x86_64-apple-darwin", "tar.gz"),
)
_VERSION_PATTERN = re.compile(r"^\d+\.\d+\.\d+$")


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
