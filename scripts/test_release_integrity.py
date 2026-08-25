import hashlib
import importlib.util
import json
import stat
import tempfile
import unittest
import zipfile
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


def _published_release(directory: Path, tag: str = TAG) -> dict:
    return {
        "tagName": tag,
        "targetCommitish": SOURCE_SHA,
        "assets": [
            {
                "name": path.name,
                "size": path.stat().st_size,
                "digest": "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest(),
                "state": "uploaded",
            }
            for path in sorted(directory.iterdir())
        ],
    }


def _write_extension_archive(path: Path, members=None) -> None:
    entries = (
        (("manifest.json", b'{"manifest_version": 3}'),) if members is None else members
    )
    with zipfile.ZipFile(path, "w") as archive:
        for name, content in entries:
            if isinstance(name, zipfile.ZipInfo):
                archive.writestr(name, content)
            else:
                archive.writestr(name, content)


def _write_extension_assets(directory: Path, version: str) -> None:
    for name in MODULE.extension_asset_names(version):
        _write_extension_archive(directory / name)


def _directory_snapshot(directory: Path) -> dict:
    return {
        path.relative_to(directory).as_posix(): (
            "directory"
            if path.is_dir()
            else hashlib.sha256(path.read_bytes()).hexdigest()
        )
        for path in sorted(directory.rglob("*"))
    }


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

    def test_native_build_artifact_set_rejects_extra_identity_and_source_mutations(
        self,
    ):
        metadata = _build_artifacts()
        metadata["artifacts"].append(
            _artifact_metadata("dcc-cua-browser-extension", 99)
        )
        metadata["total_count"] += 1
        facts = MODULE.native_build_artifact_facts(
            metadata, workflow_run_id=RUN_ID, source_sha=SOURCE_SHA
        )
        self.assertEqual(
            [fact["name"] for fact in facts], list(MODULE.EXPECTED_BUILD_ARTIFACTS)
        )

        unexpected = json.loads(json.dumps(metadata))
        unexpected["artifacts"].append(_artifact_metadata("dcc-cua-native-decoy", 100))
        unexpected["total_count"] += 1
        with self.assertRaisesRegex(ValueError, "native build artifact set"):
            MODULE.native_build_artifact_facts(
                unexpected, workflow_run_id=RUN_ID, source_sha=SOURCE_SHA
            )

        for field, value in (
            ("id", 0),
            ("digest", "sha256:not-a-digest"),
            ("workflow_run", {"id": RUN_ID + 1, "head_sha": "2" * 40}),
        ):
            changed = json.loads(json.dumps(metadata))
            changed["artifacts"][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                MODULE.native_build_artifact_facts(
                    changed, workflow_run_id=RUN_ID, source_sha=SOURCE_SHA
                )

    def test_artifact_transport_digest_is_verified_before_any_extraction(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "artifact.zip"
            with zipfile.ZipFile(archive, "w") as bundle:
                bundle.writestr("payload/file.txt", b"reviewed bytes")
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            output = root / "output"
            MODULE.verify_and_extract_artifact(archive, digest, output)
            self.assertEqual(
                (output / "payload" / "file.txt").read_bytes(), b"reviewed bytes"
            )

            sentinel = output / "sentinel.txt"
            sentinel.write_text("unchanged", encoding="utf-8")
            corrupt = root / "corrupt.zip"
            corrupt.write_bytes(archive.read_bytes() + b"corruption")
            before = {
                path.relative_to(output).as_posix(): path.read_bytes()
                for path in output.rglob("*")
                if path.is_file()
            }
            with self.assertRaisesRegex(ValueError, "transport digest"):
                MODULE.verify_and_extract_artifact(corrupt, digest, output)
            after = {
                path.relative_to(output).as_posix(): path.read_bytes()
                for path in output.rglob("*")
                if path.is_file()
            }
            self.assertEqual(after, before)

            empty = root / "empty.zip"
            with zipfile.ZipFile(empty, "w"):
                pass
            empty_digest = hashlib.sha256(empty.read_bytes()).hexdigest()
            empty_output = root / "empty-output"
            with self.assertRaisesRegex(ValueError, "regular file"):
                MODULE.verify_and_extract_artifact(empty, empty_digest, empty_output)
            self.assertFalse(empty_output.exists())

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

    def test_published_native_release_matches_local_provenance_and_latest(self):
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
            provenance_path = release_dir / MODULE.PROVENANCE_NAME
            MODULE.write_release_provenance(provenance_path, provenance)
            metadata = _published_release(release_dir)

            MODULE.verify_published_native_release(
                metadata,
                release_dir,
                version=VERSION,
                tag=TAG,
                source_sha=SOURCE_SHA,
                actual_latest_tag=TAG,
            )
            mutations = {
                "target": {**metadata, "targetCommitish": "2" * 40},
                "extra": {
                    **metadata,
                    "assets": [
                        *metadata["assets"],
                        {
                            "name": "decoy",
                            "size": 1,
                            "digest": "sha256:" + "0" * 64,
                            "state": "uploaded",
                        },
                    ],
                },
                "size": {
                    **metadata,
                    "assets": [
                        {**metadata["assets"][0], "size": 0},
                        *metadata["assets"][1:],
                    ],
                },
                "digest": {
                    **metadata,
                    "assets": [
                        {
                            **metadata["assets"][0],
                            "digest": "sha256:" + "0" * 64,
                        },
                        *metadata["assets"][1:],
                    ],
                },
            }
            for name, changed in mutations.items():
                with self.subTest(name=name), self.assertRaises(ValueError):
                    MODULE.verify_published_native_release(
                        changed,
                        release_dir,
                        version=VERSION,
                        tag=TAG,
                        source_sha=SOURCE_SHA,
                        actual_latest_tag=TAG,
                    )
            with self.assertRaisesRegex(ValueError, "Latest"):
                MODULE.verify_published_native_release(
                    metadata,
                    release_dir,
                    version=VERSION,
                    tag=TAG,
                    source_sha=SOURCE_SHA,
                    actual_latest_tag="v9.9.9",
                )

    def test_published_extension_release_is_exact_and_does_not_replace_latest(self):
        with tempfile.TemporaryDirectory() as directory:
            release_dir = Path(directory)
            extension_version = "0.3.0"
            extension_tag = f"dcc-cua-browser-extension-v{extension_version}"
            _write_extension_assets(release_dir, extension_version)
            metadata = _published_release(release_dir, extension_tag)
            MODULE.verify_published_extension_release(
                metadata,
                release_dir,
                version=extension_version,
                tag=extension_tag,
                source_sha=SOURCE_SHA,
                expected_latest_tag=TAG,
                actual_latest_tag=TAG,
            )
            extra = release_dir / "decoy.zip"
            extra.write_bytes(b"decoy")
            with self.assertRaisesRegex(ValueError, "asset set"):
                MODULE.verify_published_extension_release(
                    metadata,
                    release_dir,
                    version=extension_version,
                    tag=extension_tag,
                    source_sha=SOURCE_SHA,
                    expected_latest_tag=TAG,
                    actual_latest_tag=TAG,
                )
            extra.unlink()
            with self.assertRaisesRegex(ValueError, "Latest"):
                MODULE.verify_published_extension_release(
                    metadata,
                    release_dir,
                    version=extension_version,
                    tag=extension_tag,
                    source_sha=SOURCE_SHA,
                    expected_latest_tag=TAG,
                    actual_latest_tag=extension_tag,
                )

    def test_extension_asset_set_is_exact_valid_and_read_only(self):
        with tempfile.TemporaryDirectory() as directory:
            release_dir = Path(directory)
            extension_version = "0.3.0"
            _write_extension_assets(release_dir, extension_version)
            before = _directory_snapshot(release_dir)

            MODULE.verify_extension_asset_set(release_dir, extension_version)

            self.assertEqual(_directory_snapshot(release_dir), before)

    def test_digest_valid_artifact_extra_is_rejected_before_publish_boundary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            source.mkdir()
            extension_version = "0.3.0"
            _write_extension_assets(source, extension_version)
            (source / "unexpected-review-note.txt").write_text(
                "review note", encoding="utf-8"
            )
            transport = root / "extension-artifact.zip"
            with zipfile.ZipFile(transport, "w") as archive:
                for path in source.iterdir():
                    archive.write(path, path.name)
            digest = hashlib.sha256(transport.read_bytes()).hexdigest()
            extracted = root / "extracted"

            MODULE.verify_and_extract_artifact(transport, digest, extracted)
            before = _directory_snapshot(extracted)
            with self.assertRaisesRegex(ValueError, "missing or extra"):
                MODULE.verify_extension_asset_set(extracted, extension_version)

            self.assertEqual(_directory_snapshot(extracted), before)

    def test_extension_asset_set_rejects_unsafe_content_without_mutation(self):
        extension_version = "0.3.0"
        cases = {
            "extra": lambda directory, target: (
                directory / "unexpected-review-note.txt"
            ).write_text("review note", encoding="utf-8"),
            "empty": lambda _directory, target: _write_extension_archive(
                target, members=()
            ),
            "corrupt": lambda _directory, target: target.write_bytes(
                b"not a zip archive"
            ),
            "zip-slip": lambda _directory, target: _write_extension_archive(
                target, members=(("../escape.txt", b"escape"),)
            ),
            "case-duplicate": lambda _directory, target: _write_extension_archive(
                target,
                members=(("manifest.json", b"one"), ("MANIFEST.JSON", b"two")),
            ),
            "symlink": lambda _directory, target: _write_extension_archive(
                target,
                members=((self._symlink_zip_info(), b"manifest.json"),),
            ),
        }
        for name, mutate in cases.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                release_dir = Path(directory)
                _write_extension_assets(release_dir, extension_version)
                target = (
                    release_dir / MODULE.extension_asset_names(extension_version)[0]
                )
                mutate(release_dir, target)
                before = _directory_snapshot(release_dir)

                with self.assertRaises(ValueError):
                    MODULE.verify_extension_asset_set(release_dir, extension_version)

                self.assertEqual(_directory_snapshot(release_dir), before)
                self.assertFalse((release_dir.parent / "escape.txt").exists())

    @staticmethod
    def _symlink_zip_info() -> zipfile.ZipInfo:
        info = zipfile.ZipInfo("linked-manifest.json")
        info.create_system = 3
        info.external_attr = (stat.S_IFLNK | 0o777) << 16
        return info


if __name__ == "__main__":
    unittest.main()
