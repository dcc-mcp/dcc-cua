import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("release_integrity.py")
SPEC = importlib.util.spec_from_file_location("release_integrity", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)

SOURCE_SHA = "1" * 40
RUN_ID = 4242
VERSION = "1.6.0"
TAG = f"v{VERSION}"


def _write_complete_release(release_dir: Path) -> None:
    for target, extension in MODULE.RELEASE_TARGETS:
        archive = release_dir / f"dcc-cua-{VERSION}-{target}.{extension}"
        archive.write_bytes(target.encode("utf-8"))
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        (release_dir / (archive.name + ".sha256")).write_text(
            f"{digest}  {archive.name}\n", encoding="utf-8"
        )
        manifest = {
            "schema_version": 1,
            "name": "dcc-cua",
            "version": VERSION,
            "target": target,
            "asset": {
                "name": archive.name,
                "url": "https://github.com/dcc-mcp/dcc-cua/releases/download/"
                f"{TAG}/{archive.name}",
                "sha256": digest,
            },
        }
        (release_dir / f"dcc-cua-install-manifest-{target}.json").write_text(
            json.dumps(manifest), encoding="utf-8"
        )


def _artifact_metadata(name: str, artifact_id: int) -> dict:
    return {
        "id": artifact_id,
        "name": name,
        "expired": False,
        "digest": "sha256:" + (f"{artifact_id:064x}"[-64:]),
        "workflow_run": {"id": RUN_ID, "head_sha": SOURCE_SHA},
    }


def _build_artifacts() -> dict:
    artifacts = [
        _artifact_metadata(name, index)
        for index, name in enumerate(MODULE.EXPECTED_BUILD_ARTIFACTS, start=1)
    ]
    return {"total_count": len(artifacts), "artifacts": artifacts}


class ReleaseIntegrityTests(unittest.TestCase):
    def test_changed_release_versions_yield_only_their_new_immutable_tags(self):
        before = {
            ".": "1.5.6",
            "browser-extension/chrome": "0.2.0",
        }

        self.assertEqual(MODULE.changed_release_tags(before, before), ())
        self.assertEqual(
            MODULE.changed_release_tags(before, {**before, ".": "1.6.0"}),
            ("v1.6.0",),
        )
        self.assertEqual(
            MODULE.changed_release_tags(
                before,
                {**before, "browser-extension/chrome": "0.3.0"},
            ),
            ("dcc-cua-browser-extension-v0.3.0",),
        )
        with self.assertRaisesRegex(ValueError, "release manifest"):
            MODULE.changed_release_tags(before, {**before, "decoy": "9.9.9"})
        with self.assertRaisesRegex(ValueError, "stable semver"):
            MODULE.changed_release_tags(before, {**before, ".": "latest"})
        with self.assertRaisesRegex(ValueError, "increase"):
            MODULE.changed_release_tags(before, {**before, ".": "1.5.5"})

    def test_release_source_requires_exact_head_peeled_tag_and_release_target(self):
        MODULE.verify_release_source(
            head_sha=SOURCE_SHA,
            tag_sha=SOURCE_SHA,
            release_target=SOURCE_SHA,
            expected_sha=SOURCE_SHA,
        )
        for field in ("head_sha", "tag_sha", "release_target"):
            values = {
                "head_sha": SOURCE_SHA,
                "tag_sha": SOURCE_SHA,
                "release_target": SOURCE_SHA,
                "expected_sha": SOURCE_SHA,
            }
            values[field] = "2" * 40
            with (
                self.subTest(field=field),
                self.assertRaisesRegex(ValueError, "exact release source"),
            ):
                MODULE.verify_release_source(**values)

        with self.assertRaisesRegex(ValueError, "40-character"):
            MODULE.verify_release_source(
                head_sha="main",
                tag_sha=SOURCE_SHA,
                release_target=SOURCE_SHA,
                expected_sha=SOURCE_SHA,
            )

    def test_workflow_artifact_identity_digest_run_and_head_are_all_bound(self):
        metadata = _artifact_metadata("dcc-cua-native-release", 99)
        digest = metadata["digest"].removeprefix("sha256:")
        MODULE.verify_workflow_artifact(
            metadata,
            expected_id=99,
            expected_digest=digest,
            expected_name="dcc-cua-native-release",
            expected_run_id=RUN_ID,
            expected_head_sha=SOURCE_SHA,
        )

        mutations = {
            "id": 100,
            "name": "decoy",
            "digest": "sha256:" + "f" * 64,
            "expired": True,
            "workflow_run": {"id": RUN_ID + 1, "head_sha": "2" * 40},
        }
        for field, value in mutations.items():
            changed = json.loads(json.dumps(metadata))
            changed[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                MODULE.verify_workflow_artifact(
                    changed,
                    expected_id=99,
                    expected_digest=digest,
                    expected_name="dcc-cua-native-release",
                    expected_run_id=RUN_ID,
                    expected_head_sha=SOURCE_SHA,
                )

    def test_provenance_records_every_asset_checksum_and_unsigned_fact(self):
        with tempfile.TemporaryDirectory() as directory:
            release_dir = Path(directory)
            _write_complete_release(release_dir)
            provenance = MODULE.build_release_provenance(
                release_dir,
                version=VERSION,
                tag=TAG,
                source_sha=SOURCE_SHA,
                release_target_sha=SOURCE_SHA,
                workflow_run_id=RUN_ID,
                workflow_artifacts=_build_artifacts(),
            )
            self.assertEqual(provenance["signing"]["status"], "not_performed")
            self.assertEqual(provenance["signing"]["verification"], "sha256_only")
            self.assertEqual(len(provenance["assets"]), 4)
            self.assertEqual(len(provenance["build_artifacts"]), 4)
            for asset in provenance["assets"]:
                self.assertEqual(asset["signing"], "not_performed")
                self.assertRegex(asset["sha256"], r"^[0-9a-f]{64}$")

            provenance_path = release_dir / MODULE.PROVENANCE_NAME
            MODULE.write_release_provenance(provenance_path, provenance)
            MODULE.verify_release_provenance(
                release_dir,
                version=VERSION,
                tag=TAG,
                source_sha=SOURCE_SHA,
                release_target_sha=SOURCE_SHA,
                provenance_path=provenance_path,
            )

            document = json.loads(provenance_path.read_text(encoding="utf-8"))
            document["signing"]["status"] = "signed"
            provenance_path.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "provenance"):
                MODULE.verify_release_provenance(
                    release_dir,
                    version=VERSION,
                    tag=TAG,
                    source_sha=SOURCE_SHA,
                    release_target_sha=SOURCE_SHA,
                    provenance_path=provenance_path,
                )

    def test_native_provenance_ignores_a_separate_extension_artifact(self):
        with tempfile.TemporaryDirectory() as directory:
            release_dir = Path(directory)
            _write_complete_release(release_dir)
            metadata = _build_artifacts()
            metadata["artifacts"].append(
                _artifact_metadata("dcc-cua-browser-extension", 99)
            )
            metadata["total_count"] += 1

            provenance = MODULE.build_release_provenance(
                release_dir,
                version=VERSION,
                tag=TAG,
                source_sha=SOURCE_SHA,
                release_target_sha=SOURCE_SHA,
                workflow_run_id=RUN_ID,
                workflow_artifacts=metadata,
            )
            self.assertEqual(
                [entry["name"] for entry in provenance["build_artifacts"]],
                list(MODULE.EXPECTED_BUILD_ARTIFACTS),
            )


if __name__ == "__main__":
    unittest.main()
