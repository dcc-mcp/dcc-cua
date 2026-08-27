#!/usr/bin/env python3
"""Write stable per-target dcc-cua release metadata."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath

PACKAGE_FILES = (
    "LICENSE",
    "THIRD_PARTY_LICENSES.md",
    "README.md",
    "README.zh-CN.md",
    ".mcp.json",
)
PACKAGE_DIRECTORIES = (
    "assets",
    "skills",
    "plugins",
    ".claude-plugin",
    ".codex-plugin",
)
SUPPORTED_TARGETS = {
    "x86_64-pc-windows-msvc": "dcc-cua.exe",
    "x86_64-unknown-linux-gnu": "dcc-cua",
    "aarch64-apple-darwin": "dcc-cua",
    "x86_64-apple-darwin": "dcc-cua",
}


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _canonical_relative_path(raw: str) -> str:
    path = PurePosixPath(raw)
    if (
        not raw
        or "\\" in raw
        or path.is_absolute()
        or any(part in {"", ".", ".."} for part in path.parts)
        or path.as_posix() != raw
    ):
        raise ValueError(f"install path is not canonical: {raw!r}")
    return raw


def collect_install_plan(
    source_root: Path, target: str
) -> tuple[dict[str, str], set[str]]:
    if source_root.is_symlink() or not source_root.is_dir():
        raise ValueError("install source root must be a regular directory")
    try:
        binary_name = SUPPORTED_TARGETS[target]
    except KeyError as exc:
        raise ValueError(f"unsupported install target: {target}") from exc
    files: dict[str, str] = {}
    directories: set[str] = set()
    for name in (binary_name, *PACKAGE_FILES):
        path = source_root / name
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"install source is missing a regular file: {name}")
        files[name] = _sha256(path)
    for directory_name in PACKAGE_DIRECTORIES:
        directory = source_root / directory_name
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError(f"install source is missing a directory: {directory_name}")
        directories.add(directory_name)
        for current_root, directory_names, file_names in os.walk(
            directory, followlinks=False
        ):
            current = Path(current_root)
            for name in sorted(directory_names):
                path = current / name
                if path.is_symlink():
                    raise ValueError("install source contains a directory link")
                directories.add(
                    _canonical_relative_path(path.relative_to(source_root).as_posix())
                )
            for name in sorted(file_names):
                path = current / name
                if path.is_symlink() or not path.is_file():
                    raise ValueError("install source contains a non-regular file")
                relative = _canonical_relative_path(
                    path.relative_to(source_root).as_posix()
                )
                files[relative] = _sha256(path)
    return files, directories


def build_manifest(
    archive: Path,
    version: str,
    target: str,
    url: str,
    install_files: dict[str, str],
    install_directories: set[str] | tuple[str, ...] = (),
) -> dict:
    if not url.startswith("https://") or not url.endswith("/" + archive.name):
        raise ValueError("archive URL must be HTTPS and name the exact archive")
    digest = _sha256(archive)
    files = [
        {"path": _canonical_relative_path(path), "sha256": sha256}
        for path, sha256 in sorted(install_files.items())
    ]
    if not files or any(
        len(entry["sha256"]) != 64
        or any(character not in "0123456789abcdef" for character in entry["sha256"])
        for entry in files
    ):
        raise ValueError("install file digests must be lowercase SHA-256 values")
    directories = [
        _canonical_relative_path(path) for path in sorted(install_directories)
    ]
    return {
        "schema_version": 1,
        "name": "dcc-cua",
        "version": version,
        "target": target,
        "asset": {"name": archive.name, "url": url, "sha256": digest},
        "install": {"directories": directories, "files": files},
    }


def build_checksum_sidecar(archive: Path, digest: str) -> str:
    """Return a GNU-compatible checksum line for the published asset name."""
    return f"{digest}  {archive.name}\n"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--url", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--checksum-output", type=Path, required=True)
    parser.add_argument("--source-root", type=Path, required=True)
    args = parser.parse_args()
    install_files, install_directories = collect_install_plan(
        args.source_root, args.target
    )
    document = build_manifest(
        args.archive,
        args.version,
        args.target,
        args.url,
        install_files,
        install_directories,
    )
    args.output.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    args.checksum_output.write_text(
        build_checksum_sidecar(args.archive, document["asset"]["sha256"]),
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
