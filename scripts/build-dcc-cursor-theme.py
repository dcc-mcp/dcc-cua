#!/usr/bin/env python3
"""Retheme CUA's canonical v2 cursor source for dcc-cua."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any, NamedTuple
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

SOURCE_BLUE = [94 / 255, 192 / 255, 232 / 255, 1]
THEME_SPEC = (
    Path(__file__).resolve().parents[1]
    / "crates"
    / "dcc-cua-indicator"
    / "theme"
    / "dcc-cua-theme.json"
)


class ThemeContract(NamedTuple):
    accent_hex: str
    accent_rgba: list[float]
    theme_id: str
    reduced_motion: str


def load_theme_contract(path: Path) -> ThemeContract:
    value = json.loads(path.read_text(encoding="utf-8"))
    if value.get("schema_version") != 1:
        raise ValueError("unsupported DCC CUA theme schema")
    cursor = value["cursor"]
    encoded = cursor["accent"]
    if not isinstance(encoded, str) or len(encoded) != 7 or not encoded.startswith("#"):
        raise ValueError("cursor accent must use #RRGGBB")
    try:
        accent = [int(encoded[index : index + 2], 16) / 255 for index in (1, 3, 5)]
    except ValueError as error:
        raise ValueError("cursor accent must use #RRGGBB") from error
    theme_id = cursor["theme_id"]
    if not isinstance(theme_id, str) or not theme_id.strip():
        raise ValueError("cursor theme_id must be a non-empty string")
    reduced_motion = cursor["reduced_motion"]
    if reduced_motion not in {"auto", "reduce", "animate"}:
        raise ValueError("cursor reduced_motion must be auto, reduce, or animate")
    return ThemeContract(encoded.upper(), [*accent, 1], theme_id, reduced_motion)


def retheme(value: Any, accent: list[float]) -> tuple[Any, int]:
    if isinstance(value, list):
        if len(value) == 4 and all(
            isinstance(item, (int, float))
            and abs(float(item) - expected) < 1e-9
            for item, expected in zip(value, SOURCE_BLUE)
        ):
            return accent, 1
        changed = 0
        output = []
        for item in value:
            item, count = retheme(item, accent)
            output.append(item)
            changed += count
        return output, changed
    if isinstance(value, dict):
        changed = 0
        output = {}
        for key, item in value.items():
            item, count = retheme(item, accent)
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


def count_color(value: Any, color: list[float]) -> int:
    if isinstance(value, list):
        if len(value) == 4 and all(
            isinstance(item, (int, float))
            and abs(float(item) - expected) < 1e-9
            for item, expected in zip(value, color)
        ):
            return 1
        return sum(count_color(item, color) for item in value)
    if isinstance(value, dict):
        return sum(count_color(item, color) for item in value.values())
    return 0


def validate_built_theme(path: Path, contract_path: Path = THEME_SPEC) -> None:
    contract = load_theme_contract(contract_path)
    accent_count = 0
    with ZipFile(path) as archive:
        theme = json.loads(archive.read("cua/theme.json"))
        if theme.get("id") != contract.theme_id:
            raise ValueError("built cursor theme id does not match the shared contract")
        for name in archive.namelist():
            if name.startswith("a/") and name.endswith(".json"):
                accent_count += count_color(
                    json.loads(archive.read(name)), contract.accent_rgba
                )
    if accent_count < 12:
        raise ValueError(
            "built cursor theme accent does not cover all semantic states: "
            f"found {accent_count}"
        )


def build(source: Path, output: Path, contract_path: Path = THEME_SPEC) -> None:
    contract = load_theme_contract(contract_path)
    changed = 0
    entries: list[tuple[str, bytes]] = []
    with ZipFile(source) as archive:
        for name in archive.namelist():
            data = archive.read(name)
            if name.endswith(".json"):
                value = json.loads(data)
                value, count = retheme(value, contract.accent_rgba)
                changed += count
                if name == "manifest.json":
                    value["generator"] = "dcc-cua cursor theme builder"
                elif name == "cua/theme.json":
                    value.update(
                        {
                            "id": contract.theme_id,
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
    validate_built_theme(output, contract_path)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--theme-contract", type=Path, default=THEME_SPEC)
    args = parser.parse_args()
    build(args.source, args.output, args.theme_contract)
    print(args.output.resolve())


if __name__ == "__main__":
    main()
