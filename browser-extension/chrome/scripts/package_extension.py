#!/usr/bin/env python3
"""Build a deterministic review artifact for the DCC-CUA Chrome extension."""

from __future__ import annotations

import argparse
import hashlib
import json
import zipfile
from pathlib import Path


ARCHIVE_FILES = (
    "CHANGELOG.md",
    "component-manifest.json",
    "LICENSE",
    "README.md",
    "content-script.js",
    "manifest.json",
    "native-host-manifest.template.json",
    "protocol-v1.schema.json",
    "protocol.js",
    "service-worker.js",
)
FIXED_TIMESTAMP = (1980, 1, 1, 0, 0, 0)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def load_version(root: Path) -> str:
    package = json.loads((root / "package.json").read_text(encoding="utf-8"))
    manifest = json.loads((root / "manifest.json").read_text(encoding="utf-8"))
    package_version = package.get("version")
    manifest_version = manifest.get("version")
    if not isinstance(package_version, str) or package_version != manifest_version:
        raise ValueError("package.json and manifest.json versions must match")
    return package_version


def build_archive(root: Path, output: Path, expected_version: str | None = None) -> str:
    version = load_version(root)
    if expected_version is not None and version != expected_version:
        raise ValueError(f"extension version {version} does not match {expected_version}")
    sources = {
        name: (root.parents[1] / "LICENSE" if name == "LICENSE" else root / name)
        for name in ARCHIVE_FILES
    }
    missing = [name for name, source in sources.items() if not source.is_file()]
    if missing:
        raise ValueError(f"extension package files are missing: {', '.join(missing)}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name in ARCHIVE_FILES:
            info = zipfile.ZipInfo(name, FIXED_TIMESTAMP)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, sources[name].read_bytes())
    digest = sha256_file(output)
    sidecar = output.with_name(f"{output.name}.sha256")
    sidecar.write_text(f"{digest}  {output.name}\n", encoding="utf-8", newline="\n")
    return digest


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--version")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    digest = build_archive(root, args.output.resolve(), args.version)
    print(json.dumps({"archive": str(args.output), "sha256": digest}, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
