#!/usr/bin/env python3
"""Retheme CUA's canonical v2 cursor source for dcc-cua."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

SOURCE_BLUE = [94 / 255, 192 / 255, 232 / 255, 1]
DCC_PURPLE = [166 / 255, 99 / 255, 255 / 255, 1]
THEME_ID = "com.dcc-mcp.cursor"


def retheme(value: Any) -> tuple[Any, int]:
    if isinstance(value, list):
        if len(value) == 4 and all(
            isinstance(item, (int, float))
            and abs(float(item) - expected) < 1e-9
            for item, expected in zip(value, SOURCE_BLUE)
        ):
            return DCC_PURPLE, 1
        changed = 0
        output = []
        for item in value:
            item, count = retheme(item)
            output.append(item)
            changed += count
        return output, changed
    if isinstance(value, dict):
        changed = 0
        output = {}
        for key, item in value.items():
            item, count = retheme(item)
            output[key] = item
            changed += count
        return output, changed
    return value, 0


def encoded_json(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def write_entry(archive: ZipFile, name: str, data: bytes) -> None:
    info = ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
    info.compress_type = ZIP_DEFLATED
    info.external_attr = 0o100644 << 16
    archive.writestr(info, data, compresslevel=9)


def build(source: Path, output: Path) -> None:
    changed = 0
    entries: list[tuple[str, bytes]] = []
    with ZipFile(source) as archive:
        for name in archive.namelist():
            data = archive.read(name)
            if name.endswith(".json"):
                value = json.loads(data)
                value, count = retheme(value)
                changed += count
                if name == "manifest.json":
                    value["generator"] = "dcc-cua cursor theme builder"
                elif name == "cua/theme.json":
                    value.update(
                        {
                            "id": THEME_ID,
                            "name": "DCC CUA Purple",
                            "version": "1.0.0",
                            "author": "DCC MCP",
                            "license": "MIT",
                        }
                    )
                data = encoded_json(value)
            entries.append((name, data))
    if changed < 12:
        raise ValueError(f"expected CUA blue in all semantic states, changed {changed}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with ZipFile(output, "w") as archive:
        for name, data in entries:
            write_entry(archive, name, data)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    build(args.source, args.output)
    print(args.output.resolve())


if __name__ == "__main__":
    main()
