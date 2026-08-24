import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("verify_release_assets.py")
SPEC = importlib.util.spec_from_file_location("verify_release_assets", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


def _write_complete_release(release_dir: Path, version: str = "1.6.0") -> None:
    for target, extension in MODULE.RELEASE_TARGETS:
        archive = release_dir / f"dcc-cua-{version}-{target}.{extension}"
        archive.write_bytes(target.encode("utf-8"))
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        (release_dir / (archive.name + ".sha256")).write_text(
            f"{digest}  {archive.name}\n", encoding="utf-8"
        )
        manifest = {
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
        (release_dir / f"dcc-cua-install-manifest-{target}.json").write_text(
            json.dumps(manifest), encoding="utf-8"
        )


class VerifyReleaseAssetsTests(unittest.TestCase):
    def test_missing_intel_macos_release_assets_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            release_dir = Path(directory)
            for name in MODULE.expected_asset_names("1.6.0"):
                if "x86_64-apple-darwin" not in name:
                    (release_dir / name).touch()

            with self.assertRaisesRegex(ValueError, "x86_64-apple-darwin"):
                MODULE.verify_release_assets(release_dir, "1.6.0")

    def test_manifest_target_must_match_the_asset_slot(self):
        with tempfile.TemporaryDirectory() as directory:
            release_dir = Path(directory)
            _write_complete_release(release_dir)
            intel_manifest_path = (
                release_dir / "dcc-cua-install-manifest-x86_64-apple-darwin.json"
            )
            intel_manifest = json.loads(intel_manifest_path.read_text(encoding="utf-8"))
            intel_manifest["target"] = "aarch64-apple-darwin"
            intel_manifest_path.write_text(json.dumps(intel_manifest), encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "manifest target"):
                MODULE.verify_release_assets(release_dir, "1.6.0")

            intel_manifest["target"] = "x86_64-apple-darwin"
            intel_manifest_path.write_text(json.dumps(intel_manifest), encoding="utf-8")
            MODULE.verify_release_assets(release_dir, "1.6.0")

    def test_archive_checksum_and_manifest_provenance_must_match(self):
        with tempfile.TemporaryDirectory() as directory:
            release_dir = Path(directory)
            _write_complete_release(release_dir)
            intel_archive = release_dir / "dcc-cua-1.6.0-x86_64-apple-darwin.tar.gz"
            intel_archive.write_bytes(b"tampered")

            with self.assertRaisesRegex(ValueError, "checksum does not match"):
                MODULE.verify_release_assets(release_dir, "1.6.0")

            _write_complete_release(release_dir)
            intel_manifest_path = (
                release_dir / "dcc-cua-install-manifest-x86_64-apple-darwin.json"
            )
            intel_manifest = json.loads(intel_manifest_path.read_text(encoding="utf-8"))
            intel_manifest["asset"]["url"] = "https://example.test/tampered.tar.gz"
            intel_manifest_path.write_text(json.dumps(intel_manifest), encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "manifest does not match"):
                MODULE.verify_release_assets(release_dir, "1.6.0")

    def test_unexpected_native_release_asset_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            release_dir = Path(directory)
            _write_complete_release(release_dir)
            (release_dir / "unexpected-payload.bin").write_bytes(b"unexpected")

            with self.assertRaisesRegex(ValueError, "unexpected release assets"):
                MODULE.verify_release_assets(release_dir, "1.6.0")


if __name__ == "__main__":
    unittest.main()
