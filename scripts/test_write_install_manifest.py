import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("write-install-manifest.py")
SCHEMA = SCRIPT.parent.parent / "docs" / "schemas" / "install-manifest-v1.schema.json"
SPEC = importlib.util.spec_from_file_location("write_install_manifest", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class InstallManifestTests(unittest.TestCase):
    def _package_source(self, root: Path, target: str) -> Path:
        source = root / "target" / "release"
        source.mkdir(parents=True)
        binary = MODULE.SUPPORTED_TARGETS[target]
        for name in (binary, *MODULE.PACKAGE_FILES):
            (source / name).write_text(name, encoding="utf-8")
        for name in MODULE.PACKAGE_DIRECTORIES:
            directory = source / name
            directory.mkdir()
            (directory / "marker.txt").write_text(name, encoding="utf-8")
        return source

    def test_manifest_has_stable_target_contract_and_archive_digest(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "dcc-cua-1.2.3-target.zip"
            archive.write_bytes(b"archive")
            result = MODULE.build_manifest(
                archive,
                "1.2.3",
                "target",
                "https://example.test/" + archive.name,
                {"dcc-cua": "a" * 64, "README.md": "b" * 64},
            )
        self.assertEqual(result["target"], "target")
        self.assertEqual(result["name"], "dcc-cua")
        self.assertNotIn("product", result)
        self.assertEqual(len(result["asset"]["sha256"]), 64)
        self.assertEqual(
            result["install"]["files"],
            [
                {"path": "README.md", "sha256": "b" * 64},
                {"path": "dcc-cua", "sha256": "a" * 64},
            ],
        )

    def test_manifest_matches_the_published_schema_contract(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "dcc-cua-1.2.3-target.zip"
            archive.write_bytes(b"archive")
            result = MODULE.build_manifest(
                archive,
                "1.2.3",
                "target",
                "https://example.test/" + archive.name,
                {"dcc-cua": "a" * 64},
            )
        schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
        self.assertEqual(set(schema["required"]), set(result))
        self.assertEqual(set(schema["properties"]), set(result))
        self.assertEqual(schema["properties"]["name"]["const"], "dcc-cua")
        self.assertNotIn("product", schema["properties"])
        asset_schema = schema["properties"]["asset"]
        self.assertEqual(set(asset_schema["required"]), set(result["asset"]))
        self.assertEqual(set(asset_schema["properties"]), set(result["asset"]))
        install_schema = schema["properties"]["install"]
        self.assertEqual(set(install_schema["required"]), set(result["install"]))
        self.assertEqual(set(install_schema["properties"]), set(result["install"]))

    def test_checksum_sidecar_names_only_the_archive_basename(self):
        archive = Path("dist") / "dcc-cua-1.2.3-target.zip"
        digest = "a" * 64
        self.assertEqual(
            MODULE.build_checksum_sidecar(archive, digest),
            f"{digest}  {archive.name}\n",
        )

    def test_install_plan_excludes_unpublished_build_outputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = "x86_64-unknown-linux-gnu"
            source = self._package_source(root, target)
            (source / "deps").mkdir()
            (source / "deps" / "unpublished-build-output").write_bytes(b"decoy")
            files, directories = MODULE.collect_install_plan(source, target)
        self.assertNotIn("deps/unpublished-build-output", files)
        self.assertNotIn("deps", directories)
        self.assertIn("dcc-cua", files)
        self.assertIn("skills/marker.txt", files)

    def test_url_must_name_exact_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "dcc-cua.zip"
            archive.write_bytes(b"archive")
            with self.assertRaisesRegex(ValueError, "exact archive"):
                MODULE.build_manifest(
                    archive,
                    "1.0.0",
                    "target",
                    "https://example.test/other.zip",
                    {"dcc-cua": "a" * 64},
                )


if __name__ == "__main__":
    unittest.main()
