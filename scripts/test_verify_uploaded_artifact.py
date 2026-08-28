import hashlib
import json
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

from scripts import verify_uploaded_artifact as verifier


class UploadedArtifactVerifierTests(unittest.TestCase):
    artifact_id = 123456
    repository_id = 123456789
    head_repository_id = 987654321
    artifact_name = "dcc-cua-native-windows-x86_64"
    run_id = 987654
    head_sha = "a" * 40
    archive_name = "dcc-cua-1.2.3-x86_64-pc-windows-msvc.zip"
    manifest_name = "dcc-cua-install-manifest-x86_64-pc-windows-msvc.json"

    def _fixture(self, root, entries=None):
        bundle = root / "artifact.zip"
        entries = entries or {
            self.archive_name: b"native archive",
            f"{self.archive_name}.sha256": b"archive checksum\n",
            self.manifest_name: b"{}",
        }
        with zipfile.ZipFile(bundle, "w", zipfile.ZIP_DEFLATED) as archive:
            for name, data in entries.items():
                archive.writestr(name, data)
        digest = hashlib.sha256(bundle.read_bytes()).hexdigest()
        metadata = root / "metadata.json"
        metadata.write_text(
            json.dumps(
                {
                    "id": self.artifact_id,
                    "name": self.artifact_name,
                    "expired": False,
                    "size_in_bytes": bundle.stat().st_size,
                    "digest": f"sha256:{digest}",
                    "workflow_run": {
                        "id": self.run_id,
                        "head_sha": self.head_sha,
                        "repository_id": self.repository_id,
                        "head_repository_id": self.head_repository_id,
                    },
                }
            ),
            encoding="utf-8",
        )
        (root / "repository.json").write_text(
            json.dumps({"id": self.repository_id}), encoding="utf-8"
        )
        return bundle, metadata, digest

    def _verify(self, root, bundle, metadata, digest, after_snapshot=None):
        with mock.patch.object(
            verifier,
            "verify_final_archive",
            return_value={"type": "final_archive_verified"},
        ) as final:
            receipt = verifier.verify_uploaded_artifact(
                metadata_path=metadata,
                repository_metadata_path=root / "repository.json",
                bundle_path=bundle,
                output_root=root / "downloaded",
                expected_artifact_id=self.artifact_id,
                expected_artifact_name=self.artifact_name,
                expected_artifact_digest=digest,
                expected_run_id=self.run_id,
                expected_head_sha=self.head_sha,
                expected_repository_id=self.repository_id,
                expected_head_repository_id=self.head_repository_id,
                archive_name=self.archive_name,
                manifest_name=self.manifest_name,
                source_root=root / "source",
                target="x86_64-pc-windows-msvc",
                version="1.2.3",
                extract_root=root / "extract",
                install_root=root / "install",
                after_snapshot=after_snapshot,
            )
        return receipt, final

    def test_exact_server_artifact_is_extracted_and_fully_verified(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle, metadata, digest = self._fixture(root)
            receipt, final = self._verify(root, bundle, metadata, digest)

            self.assertEqual(receipt["type"], "uploaded_final_archive_verified")
            self.assertEqual(receipt["artifact_id"], self.artifact_id)
            self.assertEqual(receipt["artifact_digest"], digest)
            kwargs = final.call_args.kwargs
            self.assertEqual(kwargs["archive"].name, self.archive_name)
            self.assertEqual(kwargs["manifest_path"].name, self.manifest_name)
            self.assertEqual(
                kwargs["checksum_path"].name, f"{self.archive_name}.sha256"
            )

    def test_server_digest_mismatch_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle, metadata, _digest = self._fixture(root)
            with self.assertRaisesRegex(ValueError, "artifact digest"):
                self._verify(root, bundle, metadata, "0" * 64)

    def test_post_snapshot_bundle_mutation_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle, metadata, digest = self._fixture(root)

            def mutate():
                with bundle.open("ab") as stream:
                    stream.write(b"mutation")

            with self.assertRaisesRegex(ValueError, "changed during verification"):
                self._verify(root, bundle, metadata, digest, after_snapshot=mutate)

    def test_noncanonical_or_extra_envelope_members_are_rejected(self):
        for bad_name in (f"./{self.archive_name}", "unrelated.txt"):
            with (
                self.subTest(bad_name=bad_name),
                tempfile.TemporaryDirectory() as temporary,
            ):
                root = Path(temporary)
                entries = {
                    self.archive_name: b"native archive",
                    f"{self.archive_name}.sha256": b"archive checksum\n",
                    self.manifest_name: b"{}",
                    bad_name: b"decoy",
                }
                bundle, metadata, digest = self._fixture(root, entries)
                with self.assertRaisesRegex(ValueError, "artifact members"):
                    self._verify(root, bundle, metadata, digest)

    def test_metadata_identity_drift_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle, metadata, digest = self._fixture(root)
            document = json.loads(metadata.read_text(encoding="utf-8"))
            document["workflow_run"]["head_sha"] = "b" * 40
            metadata.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "workflow head"):
                self._verify(root, bundle, metadata, digest)

    def test_every_server_id_requires_an_exact_bounded_positive_integer(self):
        id_paths = (
            ("artifact", ("id",)),
            ("repository", ("id",)),
            ("artifact", ("workflow_run", "id")),
            ("artifact", ("workflow_run", "repository_id")),
            ("artifact", ("workflow_run", "head_repository_id")),
        )
        invalid_values = (True, 1.0, "1", 0, -1, 2**63)
        for source, path in id_paths:
            for invalid in invalid_values:
                with (
                    self.subTest(source=source, path=path, invalid=invalid),
                    tempfile.TemporaryDirectory() as temporary,
                ):
                    root = Path(temporary)
                    bundle, metadata, digest = self._fixture(root)
                    document_path = (
                        metadata if source == "artifact" else root / "repository.json"
                    )
                    document = json.loads(document_path.read_text(encoding="utf-8"))
                    target = document
                    for key in path[:-1]:
                        target = target[key]
                    target[path[-1]] = invalid
                    document_path.write_text(json.dumps(document), encoding="utf-8")
                    with self.assertRaisesRegex(ValueError, "ID"):
                        self._verify(root, bundle, metadata, digest)

    def test_artifact_and_workflow_repository_identities_are_bound(self):
        paths = (
            ("workflow_run", "repository_id"),
            ("workflow_run", "head_repository_id"),
        )
        for path in paths:
            with (
                self.subTest(path=path),
                tempfile.TemporaryDirectory() as temporary,
            ):
                root = Path(temporary)
                bundle, metadata, digest = self._fixture(root)
                document = json.loads(metadata.read_text(encoding="utf-8"))
                target = document
                for key in path[:-1]:
                    target = target[key]
                target[path[-1]] += 1
                metadata.write_text(json.dumps(document), encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "repository"):
                    self._verify(root, bundle, metadata, digest)

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle, metadata, digest = self._fixture(root)
            repository_metadata = root / "repository.json"
            repository_metadata.write_text(
                json.dumps({"id": self.repository_id + 1}), encoding="utf-8"
            )
            with self.assertRaisesRegex(ValueError, "repository"):
                self._verify(root, bundle, metadata, digest)


if __name__ == "__main__":
    unittest.main()
