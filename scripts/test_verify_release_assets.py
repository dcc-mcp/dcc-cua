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

WRITER_SPEC = importlib.util.spec_from_file_location(
    "write_install_manifest", SCRIPT.with_name("write-install-manifest.py")
)
WRITER = importlib.util.module_from_spec(WRITER_SPEC)
assert WRITER_SPEC.loader is not None
WRITER_SPEC.loader.exec_module(WRITER)


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
            "install": {
                "directories": [
                    ".claude-plugin",
                    ".codex-plugin",
                    "assets",
                    "plugins",
                    "skills",
                ],
                "files": [
                    {"path": ".mcp.json", "sha256": "0" * 64},
                    {"path": "LICENSE", "sha256": "1" * 64},
                    {"path": "README.md", "sha256": "2" * 64},
                    {"path": "README.zh-CN.md", "sha256": "3" * 64},
                    {"path": "THIRD_PARTY_LICENSES.md", "sha256": "4" * 64},
                    {
                        "path": "dcc-cua.exe" if "windows" in target else "dcc-cua",
                        "sha256": "5" * 64,
                    },
                    {
                        "path": (
                            "dcc-cua-background.exe"
                            if "windows" in target
                            else "dcc-cua-background"
                        ),
                        "sha256": "6" * 64,
                    },
                ],
            },
        }
        manifest["install"]["files"].sort(key=lambda entry: entry["path"])
        (release_dir / f"dcc-cua-install-manifest-{target}.json").write_text(
            json.dumps(manifest), encoding="utf-8"
        )


class VerifyReleaseAssetsTests(unittest.TestCase):
    def test_writer_generated_manifests_pass_for_every_release_target(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            release_dir = root / "dist"
            release_dir.mkdir()
            for target, extension in MODULE.RELEASE_TARGETS:
                source = root / target
                source.mkdir()
                suffix = ".exe" if "windows" in target else ""
                for name in (
                    "dcc-cua" + suffix,
                    "dcc-cua-background" + suffix,
                    *WRITER.PACKAGE_FILES,
                ):
                    (source / name).write_bytes(name.encode("utf-8"))
                for name in WRITER.PACKAGE_DIRECTORIES:
                    (source / name).mkdir()
                    (source / name / "marker.txt").write_bytes(b"package content")
                archive = release_dir / f"dcc-cua-1.10.0-{target}.{extension}"
                archive.write_bytes(target.encode("utf-8"))
                files, directories = WRITER.collect_install_plan(source, target)
                manifest = WRITER.build_manifest(
                    archive,
                    "1.10.0",
                    target,
                    "https://github.com/dcc-mcp/dcc-cua/releases/download/"
                    f"v1.10.0/{archive.name}",
                    files,
                    directories,
                )
                (release_dir / (archive.name + ".sha256")).write_text(
                    WRITER.build_checksum_sidecar(archive, manifest["asset"]["sha256"]),
                    encoding="utf-8",
                )
                (release_dir / f"dcc-cua-install-manifest-{target}.json").write_text(
                    json.dumps(manifest), encoding="utf-8"
                )
            MODULE.verify_release_assets(release_dir, "1.10.0")

    def test_both_entry_points_are_required_for_every_release_target(self):
        for target, _ in MODULE.RELEASE_TARGETS:
            suffix = ".exe" if "windows" in target else ""
            for binary in ("dcc-cua" + suffix, "dcc-cua-background" + suffix):
                with self.subTest(target=target, missing=binary):
                    with tempfile.TemporaryDirectory() as directory:
                        release_dir = Path(directory)
                        _write_complete_release(release_dir)
                        manifest_path = (
                            release_dir / f"dcc-cua-install-manifest-{target}.json"
                        )
                        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
                        manifest["install"]["files"] = [
                            entry for entry in manifest["install"]["files"]
                            if entry["path"] != binary
                        ]
                        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
                        with self.assertRaisesRegex(ValueError, "complete, unique, and sorted"):
                            MODULE.verify_release_assets(release_dir, "1.6.0")

    def test_unexpected_background_entry_names_fail_closed(self):
        for target, _ in MODULE.RELEASE_TARGETS:
            other_platform = (
                "dcc-cua-background"
                if "windows" in target
                else "dcc-cua-background.exe"
            )
            for unexpected in (other_platform, "dcc-cua-background-extra"):
                with self.subTest(target=target, unexpected=unexpected):
                    with tempfile.TemporaryDirectory() as directory:
                        release_dir = Path(directory)
                        _write_complete_release(release_dir)
                        manifest_path = (
                            release_dir / f"dcc-cua-install-manifest-{target}.json"
                        )
                        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
                        manifest["install"]["files"].append(
                            {"path": unexpected, "sha256": "7" * 64}
                        )
                        manifest["install"]["files"].sort(key=lambda entry: entry["path"])
                        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
                        with self.assertRaisesRegex(ValueError, "invalid package file"):
                            MODULE.verify_release_assets(release_dir, "1.6.0")

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

    def test_install_manifest_paths_and_digests_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            release_dir = Path(directory)
            _write_complete_release(release_dir)
            manifest_path = (
                release_dir / "dcc-cua-install-manifest-x86_64-apple-darwin.json"
            )
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            manifest["install"]["files"][0]["path"] = "../outside"
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "install manifest"):
                MODULE.verify_release_assets(release_dir, "1.6.0")


if __name__ == "__main__":
    unittest.main()
