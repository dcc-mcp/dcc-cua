#!/usr/bin/env python3
"""Build a deterministic store artifact from one WXT browser target."""

from __future__ import annotations

import argparse
import hashlib
import json
import zipfile
from pathlib import Path


SUPPORTED_BROWSERS = ("chrome", "edge", "firefox")
FIXED_TIMESTAMP = (1980, 1, 1, 0, 0, 0)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def load_version(root: Path) -> str:
    package = json.loads((root / "package.json").read_text(encoding="utf-8"))
    version = package.get("version")
    if not isinstance(version, str):
        raise ValueError("package.json version must be a string")
    return version


def build_root(root: Path, browser: str) -> Path:
    if browser not in SUPPORTED_BROWSERS:
        raise ValueError(f"unsupported browser target: {browser}")
    output = root / ".output" / f"{browser}-mv3"
    if not output.is_dir():
        raise ValueError(f"WXT output is missing for {browser}: {output}")
    return output


def archive_files(output: Path) -> list[Path]:
    files = sorted(
        (path for path in output.rglob("*") if path.is_file()),
        key=lambda path: path.relative_to(output).as_posix(),
    )
    if not files:
        raise ValueError("WXT output contains no files")
    return files


def validate_manifest(root: Path, output: Path, expected_version: str | None) -> str:
    version = load_version(root)
    if expected_version is not None and version != expected_version:
        raise ValueError(f"extension version {version} does not match {expected_version}")
    manifest = json.loads((output / "manifest.json").read_text(encoding="utf-8"))
    if manifest.get("version") != version:
        raise ValueError("generated manifest version must match package.json")
    if manifest.get("manifest_version") != 3:
        raise ValueError("generated extension must use Manifest V3")
    if "host_permissions" in manifest:
        raise ValueError("generated extension must not request blanket host permissions")
    return version


def build_archive(
    root: Path,
    output: Path,
    browser: str,
    expected_version: str | None = None,
) -> str:
    built = build_root(root, browser)
    validate_manifest(root, built, expected_version)
    files = archive_files(built)
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for source in files:
            name = source.relative_to(built).as_posix()
            info = zipfile.ZipInfo(name, FIXED_TIMESTAMP)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, source.read_bytes())
    digest = sha256_file(output)
    sidecar = output.with_name(f"{output.name}.sha256")
    sidecar.write_text(f"{digest}  {output.name}\n", encoding="utf-8", newline="\n")
    return digest


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--browser", choices=SUPPORTED_BROWSERS, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--version")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    digest = build_archive(root, args.output.resolve(), args.browser, args.version)
    print(
        json.dumps(
            {"archive": str(args.output), "browser": args.browser, "sha256": digest},
            separators=(",", ":"),
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
