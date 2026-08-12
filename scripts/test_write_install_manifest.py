import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("write-install-manifest.py")
SPEC = importlib.util.spec_from_file_location("write_install_manifest", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class InstallManifestTests(unittest.TestCase):
    def test_manifest_has_stable_target_contract_and_archive_digest(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "dcc-cua-1.2.3-target.zip"
            archive.write_bytes(b"archive")
            result = MODULE.build_manifest(
                archive, "1.2.3", "target", "https://example.test/" + archive.name
            )
        self.assertEqual(result["target"], "target")
        self.assertEqual(len(result["asset"]["sha256"]), 64)

    def test_url_must_name_exact_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "dcc-cua.zip"
            archive.write_bytes(b"archive")
            with self.assertRaisesRegex(ValueError, "exact archive"):
                MODULE.build_manifest(archive, "1.0.0", "target", "https://example.test/other.zip")


if __name__ == "__main__":
    unittest.main()
