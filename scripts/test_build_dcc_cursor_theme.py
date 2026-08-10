from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "build-dcc-cursor-theme.py"
SPEC = importlib.util.spec_from_file_location("build_dcc_cursor_theme", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
builder = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(builder)


class CursorThemeContractTests(unittest.TestCase):
    def test_canonical_contract_is_packaged_with_the_indicator_crate(self) -> None:
        expected = (
            ROOT
            / "crates"
            / "dcc-cua-indicator"
            / "theme"
            / "dcc-cua-theme.json"
        )
        self.assertEqual(builder.THEME_SPEC, expected)
        contract = builder.load_theme_contract(expected)
        self.assertEqual(contract.theme_id, "com.dcc-mcp.cursor")
        self.assertEqual(contract.accent_hex, "#A663FF")
        self.assertEqual(contract.reduced_motion, "auto")

    def test_contract_rejects_unknown_reduced_motion_policy(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "theme.json"
            path.write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "cursor": {
                            "theme_id": "com.example.cursor",
                            "accent": "#A663FF",
                            "reduced_motion": "sometimes",
                        },
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "reduced_motion"):
                builder.load_theme_contract(path)

    def test_build_uses_the_supplied_contract_for_id_and_accent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            contract_path = root / "theme.json"
            contract_path.write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "cursor": {
                            "theme_id": "com.example.cursor",
                            "accent": "#123456",
                            "reduced_motion": "reduce",
                        },
                    }
                ),
                encoding="utf-8",
            )
            source = root / "source.lottie"
            with ZipFile(source, "w", ZIP_DEFLATED) as archive:
                archive.writestr("manifest.json", json.dumps({"animations": []}))
                archive.writestr("cua/theme.json", json.dumps({"id": "source"}))
                for index in range(12):
                    archive.writestr(
                        f"a/action_{index}.json",
                        json.dumps({"color": builder.SOURCE_BLUE}),
                    )
            output = root / "output.lottie"

            builder.build(source, output, contract_path)

            with ZipFile(output) as archive:
                theme = json.loads(archive.read("cua/theme.json"))
                action = json.loads(archive.read("a/action_0.json"))
            self.assertEqual(theme["id"], "com.example.cursor")
            self.assertEqual(action["color"], [18 / 255, 52 / 255, 86 / 255, 1])

    def test_checked_in_cursor_source_matches_the_shared_contract(self) -> None:
        builder.validate_built_theme(
            ROOT / "assets" / "cursor-theme" / "dcc-cua.lottie"
        )


if __name__ == "__main__":
    unittest.main()
