import contextlib
import copy
import hashlib
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts import download_uploaded_artifact as reader


class FakeApi:
    payload = b"fixture uploaded bytes, never executed"

    def __init__(self):
        self.metadata = {
            "id": 123,
            "name": "native-fixture",
            "expired": False,
            "size_in_bytes": len(self.payload),
            "digest": "sha256:" + hashlib.sha256(self.payload).hexdigest(),
            "workflow_run": {
                "id": 42,
                "head_sha": "a" * 40,
                "repository_id": 7,
                "head_repository_id": 8,
            },
        }
        self.listings = [{"total_count": 1, "artifacts": [self.metadata]}]
        self.metadata_reads = [self.metadata]
        self.bundles = [self.payload]
        self.calls = []

    def json(self, stage, endpoint, attempt=1):
        self.calls.append((stage, endpoint, attempt))
        if stage == "repository":
            return {"id": 7, "full_name": "owner/repo"}
        queue = self.listings if stage == "run_artifacts" else self.metadata_reads
        value = queue.pop(0) if len(queue) > 1 else queue[0]
        if isinstance(value, Exception):
            raise value
        return copy.deepcopy(value)

    def bundle(self, stage, endpoint, path, attempt=1):
        self.calls.append((stage, endpoint, attempt))
        value = self.bundles.pop(0) if len(self.bundles) > 1 else self.bundles[0]
        path.write_bytes(b"partial" if isinstance(value, Exception) else value)
        if isinstance(value, Exception):
            raise value


class UploadedArtifactDownloadTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.api = FakeApi()
        self.identity = {
            "artifact_id": 123,
            "artifact_name": "native-fixture",
            "artifact_digest": hashlib.sha256(self.api.payload).hexdigest(),
            "run_id": 42,
            "head_sha": "a" * 40,
            "repository_id": 7,
            "head_repository_id": 8,
        }
        self.delays = []
        self.output = io.StringIO()

    def run_download(self):
        with contextlib.redirect_stdout(self.output):
            reader.download(
                self.api,
                repository="owner/repo",
                root=self.root,
                identity=self.identity,
                sleep=self.delays.append,
            )

    def test_exact_upload_is_downloaded_after_same_run_proof(self):
        self.run_download()
        self.assertEqual((self.root / "artifact.zip").read_bytes(), self.api.payload)
        self.assertEqual(
            json.loads((self.root / "metadata.json").read_text())["id"], 123
        )
        self.assertEqual(self.delays, [])
        self.assertEqual(
            [c[0] for c in self.api.calls],
            ["repository", "run_artifacts", "metadata", "bundle"],
        )

    def test_proven_metadata_404_is_rechecked_then_retried_with_a_bound(self):
        self.api.metadata_reads = [
            reader.HttpFailure("metadata", 404),
            self.api.metadata,
        ]
        self.run_download()
        self.assertEqual(self.delays, [1])
        self.assertEqual([c[0] for c in self.api.calls].count("run_artifacts"), 2)
        self.assertIn(
            "exact_artifact_still_proven_in_current_run", self.output.getvalue()
        )

    def test_proven_bundle_404_removes_partial_before_retry(self):
        self.api.bundles = [reader.HttpFailure("bundle", 404), self.api.payload]
        self.run_download()
        self.assertEqual(self.delays, [1])
        self.assertEqual(list(self.root.glob("*.part-*")), [])
        self.assertEqual((self.root / "artifact.zip").read_bytes(), self.api.payload)

    def test_persistent_404_never_becomes_success(self):
        self.api.metadata_reads = [reader.HttpFailure("metadata", 404)]
        with self.assertRaises(reader.HttpFailure):
            self.run_download()
        self.assertEqual(self.delays, [1, 2])
        self.assertEqual([c[0] for c in self.api.calls].count("metadata"), 3)
        self.assertFalse((self.root / "artifact.zip").exists())
        self.assertNotIn('"stage": "complete"', self.output.getvalue())

    def test_unproven_id_or_inventory_denial_is_not_retried(self):
        for listing in (
            {"total_count": 0, "artifacts": []},
            reader.HttpFailure("run_artifacts", 404),
            reader.HttpFailure("run_artifacts", 403),
        ):
            with self.subTest(listing=type(listing).__name__):
                self.api = FakeApi()
                self.api.listings = [listing]
                with self.assertRaises((ValueError, reader.HttpFailure)):
                    self.run_download()
                self.assertEqual(self.delays, [])
                self.assertFalse(any(c[0] == "metadata" for c in self.api.calls))

    def test_identity_expiry_and_digest_mismatches_fail_before_download(self):
        mutations = [
            ("id", 999),
            ("name", "wrong"),
            ("expired", True),
            ("digest", "sha256:" + "b" * 64),
            ("size_in_bytes", True),
        ]
        for field, value in mutations:
            with self.subTest(field=field):
                self.api = FakeApi()
                self.api.metadata[field] = value
                with self.assertRaises(ValueError):
                    self.run_download()
                self.assertEqual(self.delays, [])
                self.assertFalse(any(c[0] == "bundle" for c in self.api.calls))

    def test_wrong_run_head_or_repository_is_not_retried(self):
        for field, value in (
            ("id", 43),
            ("head_sha", "b" * 40),
            ("repository_id", 9),
            ("head_repository_id", 9),
        ):
            with self.subTest(field=field):
                self.api = FakeApi()
                self.api.metadata["workflow_run"][field] = value
                with self.assertRaises(ValueError):
                    self.run_download()
                self.assertEqual(self.delays, [])

    def test_direct_metadata_cannot_replace_the_proven_upload(self):
        wrong = copy.deepcopy(self.api.metadata)
        wrong["workflow_run"]["id"] = 999
        self.api.metadata_reads = [wrong]
        with self.assertRaisesRegex(ValueError, "workflow run"):
            self.run_download()
        self.assertEqual(self.delays, [])
        self.assertFalse(any(c[0] == "bundle" for c in self.api.calls))
        self.assertIn('"stage": "identity"', self.output.getvalue())

    def test_denial_expiry_and_other_http_errors_are_not_retryable(self):
        for stage in ("metadata", "bundle"):
            for status in (401, 403, 410, 429, 500, None):
                with (
                    self.subTest(stage=stage, status=status),
                    tempfile.TemporaryDirectory() as tmp,
                ):
                    self.root = Path(tmp)
                    self.api = FakeApi()
                    failure = reader.HttpFailure(stage, status)
                    if stage == "metadata":
                        self.api.metadata_reads = [failure]
                    else:
                        self.api.bundles = [failure]
                    with self.assertRaises(reader.HttpFailure):
                        self.run_download()
                    self.assertEqual(self.delays, [])
                    self.assertFalse((self.root / "artifact.zip").exists())
                    self.assertEqual(list(self.root.glob("*.part-*")), [])

    def test_permission_or_identity_loss_during_404_recheck_aborts_retry(self):
        self.api.metadata_reads = [reader.HttpFailure("metadata", 404)]
        self.api.listings = [
            copy.deepcopy(self.api.listings[0]),
            reader.HttpFailure("run_artifacts", 403),
        ]
        with self.assertRaises(reader.HttpFailure):
            self.run_download()
        self.assertEqual(self.delays, [])
        self.assertEqual([c[0] for c in self.api.calls].count("metadata"), 1)

    def test_expiry_during_404_recheck_aborts_retry(self):
        self.api.metadata_reads = [reader.HttpFailure("metadata", 404)]
        expired = copy.deepcopy(self.api.listings[0])
        expired["artifacts"][0]["expired"] = True
        self.api.listings = [copy.deepcopy(self.api.listings[0]), expired]
        with self.assertRaises(ValueError):
            self.run_download()
        self.assertEqual(self.delays, [])

    def test_wrong_bytes_do_not_get_retry_or_reach_verifier(self):
        self.api.bundles = [b"x" * len(self.api.payload)]
        with self.assertRaisesRegex(ValueError, "digest mismatch"):
            self.run_download()
        self.assertEqual(self.delays, [])
        self.assertFalse((self.root / "artifact.zip").exists())

    def test_short_download_is_not_retried_or_promoted(self):
        self.api.bundles = [b"short"]
        with self.assertRaisesRegex(ValueError, "size mismatch"):
            self.run_download()
        self.assertEqual(self.delays, [])
        self.assertFalse((self.root / "artifact.zip").exists())

    def test_timeout_removes_only_its_partial_without_retry(self):
        self.api.bundles = [subprocess.TimeoutExpired("fixture read", 120)]
        with self.assertRaises(subprocess.TimeoutExpired):
            self.run_download()
        self.assertEqual(self.delays, [])
        self.assertEqual(list(self.root.glob("*.part-*")), [])

    def test_exact_id_on_second_run_inventory_page_is_supported(self):
        self.api.listings = [
            {"total_count": 101, "artifacts": [{"id": i} for i in range(1000, 1100)]},
            {"total_count": 101, "artifacts": [self.api.metadata]},
        ]
        self.run_download()
        self.assertEqual([c[0] for c in self.api.calls].count("run_artifacts"), 2)
        self.assertTrue((self.root / "artifact.zip").is_file())

    def test_run_inventory_is_bounded_and_rejects_duplicates(self):
        for listing in (
            {"total_count": 501, "artifacts": []},
            {"total_count": 2, "artifacts": [self.api.metadata, self.api.metadata]},
        ):
            self.api.listings = [listing]
            with self.assertRaises(ValueError):
                self.run_download()
            self.assertEqual(self.delays, [])

    def test_http_diagnostics_never_print_credentials_body_or_signed_redirect(self):
        stdout = b'HTTP/2.0 404 Not Found\r\nX-GitHub-Request-Id: A:B\r\nLocation: signed-secret\r\n\r\n{"message":"body-secret"}'
        response = subprocess.CompletedProcess([], 1, stdout, b"token-secret")
        with (
            mock.patch.dict("os.environ", {"GH_TOKEN": "token-secret"}),
            mock.patch.object(reader.subprocess, "run", return_value=response) as run,
            contextlib.redirect_stdout(self.output),
            self.assertRaises(reader.HttpFailure),
        ):
            reader.GhApi().json("metadata", "repos/owner/repo/actions/artifacts/123")
        diagnostic = self.output.getvalue()
        self.assertIn('"http_status": 404', diagnostic)
        self.assertIn('"request_id": "A:B"', diagnostic)
        for secret in ("token-secret", "signed-secret", "body-secret"):
            self.assertNotIn(secret, diagnostic)
        self.assertNotIn("token-secret", repr(run.call_args))

    def test_job_token_is_required_without_auth_config_fallback(self):
        with (
            mock.patch.dict("os.environ", {}, clear=True),
            self.assertRaisesRegex(ValueError, "GH_TOKEN"),
        ):
            reader.GhApi()


if __name__ == "__main__":
    unittest.main()
