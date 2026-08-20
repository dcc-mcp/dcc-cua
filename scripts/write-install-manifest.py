#!/usr/bin/env python3
"""Write stable per-target dcc-cua release metadata."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def build_manifest(archive: Path, version: str, target: str, url: str) -> dict:
    if not url.startswith("https://") or not url.endswith("/" + archive.name):
        raise ValueError("archive URL must be HTTPS and name the exact archive")
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    return {
        "schema_version": 1,
        "name": "dcc-cua",
        "version": version,
        "target": target,
        "asset": {"name": archive.name, "url": url, "sha256": digest},
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
    args = parser.parse_args()
    document = build_manifest(args.archive, args.version, args.target, args.url)
    args.output.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    args.checksum_output.write_text(
        build_checksum_sidecar(args.archive, document["asset"]["sha256"]),
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
