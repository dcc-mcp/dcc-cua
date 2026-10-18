"""Admission, transport and closed executable-surface contracts for publication resume."""

import copy
import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from scripts import resume_native_publication as resume
from scripts.test_release_workflow import _RestrictedWorkflowYamlParser
from scripts.test_release_integrity import (
    MODULE, RUN_ID, SOURCE_SHA, TAG, VERSION, _build_artifacts, _write_complete_release,
)
from scripts.test_publish_verified_release import FakeGitHub

ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github/workflows/resume-native-publication.yml"
SURFACE_SHA256 = "fca34997638a5dea169d202908a5d15eef3e32ef0d942c81a3f75fede07a19b3"


def workflow_surface(text):
    parsed = _RestrictedWorkflowYamlParser(text).parse()
    digest = hashlib.sha256(json.dumps(parsed, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    if digest != SURFACE_SHA256:
        raise AssertionError("publication resume executable surface differs from the reviewed contract")
    return parsed


class ResumeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.directory = self.root / "dist"
        self.directory.mkdir()
        _write_complete_release(self.directory)
        self.request = dict(producer_run_id=RUN_ID, producer_head_sha=SOURCE_SHA,
                            artifact_id=991, artifact_digest="sha256:" + "a" * 64,
                            tag=TAG, source_sha=SOURCE_SHA, release_id=42)
        self.repository_id = 123
        self.repository = "dcc-mcp/dcc-cua"
        self.provenance = MODULE.build_release_provenance(
            self.directory, version=VERSION, tag=TAG, source_sha=SOURCE_SHA,
            release_target_sha=SOURCE_SHA, workflow_run_id=RUN_ID, workflow_artifacts=_build_artifacts())
        MODULE.write_release_provenance(self.directory / MODULE.PROVENANCE_NAME, self.provenance)
        self.artifact = dict(id=991, name="dcc-cua-native-release", expired=False,
                             digest=self.request["artifact_digest"], workflow_run=dict(
                                 id=RUN_ID, head_sha=SOURCE_SHA, repository_id=123, head_repository_id=123))
        rows = _build_artifacts()["artifacts"]
        for row in rows:
            row["workflow_run"].update(repository_id=123, head_repository_id=123)
        self.listing = dict(total_count=5, artifacts=[*rows, self.artifact])
        self.run = dict(id=RUN_ID, run_attempt=1, head_sha=SOURCE_SHA, head_branch="main",
                        event="workflow_dispatch", path=".github/workflows/release-please.yml",
                        status="completed", conclusion="failure", repository=dict(id=123, full_name=self.repository),
                        head_repository=dict(id=123, full_name=self.repository))
        self.jobs = dict(total_count=13, jobs=[dict(id=index, name=name, run_id=RUN_ID, run_attempt=1,
            head_sha=SOURCE_SHA, status="completed", conclusion=outcome)
            for index, (name, outcome) in enumerate(resume.JOB_CONCLUSIONS.items(), 100)])

    def validate_bundle(self):
        return resume.validate_bundle(self.directory, self.artifact, self.listing, self.request, 123)

    def validate_producer(self):
        return resume.validate_producer(self.run, self.jobs, self.request, self.repository, 123)

    def test_failed_producer_and_new_execution_are_distinct(self):
        env = {key.upper(): str(value) for key, value in self.request.items()}
        env.update(GITHUB_EVENT_NAME="workflow_dispatch", GITHUB_REF="refs/heads/main",
                   GITHUB_RUN_ATTEMPT="1", GITHUB_SHA="b" * 40, GITHUB_RUN_ID="9999")
        self.assertEqual(resume.request_from_environment(env), self.request)
        for key, value in (("GITHUB_REF", "refs/heads/other"), ("GITHUB_EVENT_NAME", "push"),
                           ("GITHUB_RUN_ATTEMPT", "2"), ("GITHUB_RUN_ID", str(RUN_ID)),
                           ("ARTIFACT_ID", "0991"), ("ARTIFACT_DIGEST", "a" * 64),
                           ("TAG", "v9.0.0/other"), ("SOURCE_SHA", "main")):
            with self.subTest(key=key), self.assertRaises(ValueError):
                resume.request_from_environment({**env, key: value})

    def test_complete_original_native_job_graph_is_required(self):
        self.assertEqual(len(self.validate_producer()), 13)
        original_run, original_jobs = copy.deepcopy(self.run), copy.deepcopy(self.jobs)
        for key, value in (("conclusion", "success"), ("event", "pull_request"),
                           ("head_branch", "other"), ("path", "other.yml"),
                           ("head_sha", "b" * 40), ("run_attempt", 2),
                           ("head_repository", dict(id=999, full_name="other/repo"))):
            self.run = {**original_run, key: value}
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.validate_producer()
        self.run = original_run
        for change in (lambda rows: rows.pop(), lambda rows: rows.append(copy.deepcopy(rows[0])),
                       lambda rows: rows[0].update(conclusion="skipped"),
                       lambda rows: rows[0].update(head_sha="b" * 40),
                       lambda rows: rows[0].update(run_attempt=2)):
            self.jobs = copy.deepcopy(original_jobs)
            change(self.jobs["jobs"])
            with self.assertRaises(ValueError):
                self.validate_producer()

    def test_aggregate_is_separate_from_exact_four_original_builds(self):
        self.assertEqual(self.validate_bundle(), self.provenance)
        self.assertEqual(self.provenance["workflow_run_id"], RUN_ID)
        original = copy.deepcopy(self.listing)
        for key, value in (("id", 777), ("digest", "sha256:" + "b" * 64), ("expired", True),
                           ("workflow_run", dict(id=999, head_sha=SOURCE_SHA))):
            self.listing = copy.deepcopy(original)
            self.listing["artifacts"][0][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.validate_bundle()
        self.listing = original
        self.provenance["workflow_run_id"] = RUN_ID + 1
        for fact in self.provenance["build_artifacts"]:
            fact["workflow_run_id"] = RUN_ID + 1
        MODULE.write_release_provenance(self.directory / MODULE.PROVENANCE_NAME, self.provenance)
        with self.assertRaisesRegex(ValueError, "original producer run"):
            self.validate_bundle()

    def receipt(self):
        fact, asset = self.provenance["build_artifacts"][0], self.provenance["assets"][0]
        return dict(schema_version=1, type="uploaded_final_archive_verified", artifact_id=fact["id"],
            artifact_name=fact["name"], artifact_digest=fact["sha256"], repository_id=123, head_repository_id=123,
            workflow_run_id=RUN_ID, workflow_head_sha=SOURCE_SHA,
            final_archive=dict(schema_version=1, type="final_archive_verified", version=VERSION,
                target=asset["target"], archive=dict(name=asset["archive"], sha256=asset["sha256"]),
                smoke_commands=["--version", "version", "help", "manifest", "doctor"], file_count=18,
                source_tree_sha256="c" * 64, archive_tree_sha256="c" * 64, install_tree_sha256="c" * 64))

    def test_actual_unique_archive_receipt_is_bound_to_old_artifact_and_content(self):
        good = self.receipt()
        def check(value, count=1):
            log = ("2026-10-18T12:15:54.1768820Z " + json.dumps(value) + "\n") * count
            return resume.validate_archive_receipt(log, self.provenance["build_artifacts"][0],
                                                  self.provenance["assets"][0], self.request, 123)
        self.assertEqual(check(good), good)
        for count in (0, 2):
            with self.assertRaises(ValueError):
                check(good, count)
        for key, value in (("workflow_run_id", 999), ("workflow_head_sha", "b" * 40),
                           ("artifact_id", 999), ("head_repository_id", 999)):
            with self.subTest(key=key), self.assertRaises(ValueError):
                check({**good, key: value})
        for key, value in (("install_tree_sha256", "d" * 64), ("file_count", 0),
                           ("smoke_commands", ["--version"]), ("version", "9.9.9"),
                           ("archive", dict(name="other", sha256="d" * 64))):
            bad = copy.deepcopy(good)
            bad["final_archive"][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                check(bad)

    def test_transport_digest_and_safe_extraction_fail_before_bundle_admission(self):
        archive = self.root / "artifact.zip"
        with zipfile.ZipFile(archive, "w") as bundle:
            bundle.writestr("../escape", "bad")
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        for expected in ("0" * 64, digest):
            output = self.root / expected
            with self.assertRaises(ValueError):
                MODULE.verify_and_extract_artifact(archive, expected, output)
            self.assertFalse(output.exists())

    def test_existing_asset_gateway_never_uploads_and_rejects_disappearance_or_replacement(self):
        gateway = resume.ExistingAssetsGitHub(self.repository, 42)
        good = dict(id=42, assets=[dict(id=1, name="asset", size=1, digest="sha256:" + "a" * 64)])
        with patch.object(resume.GitHub, "release", return_value=good):
            self.assertEqual(gateway.release(TAG), good)
        for value in (dict(id=42, assets=[]), {**good, "id": 99},
                      dict(id=42, assets=[{**good["assets"][0], "id": 2}])):
            with patch.object(resume.GitHub, "release", return_value=value), self.assertRaises(ValueError):
                gateway.release(TAG)
        with self.assertRaisesRegex(ValueError, "cannot upload"):
            gateway.upload(TAG, [])

    def test_official_entrypoint_reuses_original_transport_and_publishes_all_thirteen(self):
        # Separate source S, producer W, and current executing tool H throughout.
        producer_head = "b" * 40
        self.request["producer_head_sha"] = producer_head
        self.run["head_sha"] = producer_head
        for row in self.jobs["jobs"]:
            row["head_sha"] = producer_head
        self.provenance["workflow_commit"] = producer_head
        for fact in self.provenance["build_artifacts"]:
            fact["head_sha"] = producer_head
        for row in self.listing["artifacts"]:
            row["workflow_run"]["head_sha"] = producer_head
        MODULE.write_release_provenance(self.directory / MODULE.PROVENANCE_NAME, self.provenance)
        archive = self.root / "original.zip"
        with zipfile.ZipFile(archive, "w") as bundle:
            for path in self.directory.iterdir():
                bundle.write(path, path.name)
        body = archive.read_bytes()
        self.request["artifact_digest"] = "sha256:" + hashlib.sha256(body).hexdigest()
        self.artifact["digest"] = self.request["artifact_digest"]
        fake = FakeGitHub(self.directory, TAG)
        fake.draft_urls = True
        fake.upload(TAG, [])
        fake.events.clear()
        transport = {f"actions/runs/{RUN_ID}": self.run,
            f"actions/runs/{RUN_ID}/attempts/1/jobs?per_page=100": self.jobs,
            "actions/artifacts/991": self.artifact,
            f"actions/runs/{RUN_ID}/artifacts?per_page=100": self.listing}
        receipts = {}
        for name, fact, asset in zip(resume.BUILD_JOBS, self.provenance["build_artifacts"], self.provenance["assets"]):
            job = next(row for row in self.jobs["jobs"] if row["name"] == name)
            receipt = self.receipt()
            receipt.update(artifact_id=fact["id"], artifact_name=fact["name"], artifact_digest=fact["sha256"],
                           workflow_head_sha=producer_head)
            receipt["final_archive"].update(target=asset["target"],
                archive=dict(name=asset["archive"], sha256=asset["sha256"]))
            receipts[f"repos/{self.repository}/actions/jobs/{job['id']}/logs"] = (
                "2026-10-18T12:15:54.1768820Z " + json.dumps(receipt))
        env = {key.upper(): str(value) for key, value in self.request.items()}
        env.update(GITHUB_EVENT_NAME="workflow_dispatch", GITHUB_REF="refs/heads/main", GITHUB_RUN_ATTEMPT="1",
                   GITHUB_SHA="c" * 40, GITHUB_RUN_ID="9999", GITHUB_REPOSITORY=self.repository,
                   GITHUB_REPOSITORY_ID="123", RUNNER_TEMP=str(self.root))
        def command(*args):
            if args[:3] == ("git", "rev-parse", "HEAD"):
                return env["GITHUB_SHA"]
            if args[:2] == ("git", "merge-base"):
                return ""
            if args[:2] == ("git", "show"):
                return VERSION
            if args[:2] == ("gh", "api"):
                return receipts[args[2]]
            raise AssertionError(args)
        def download(args, stdout, check):
            self.assertEqual(args, ["gh", "api", f"repos/{self.repository}/actions/artifacts/991/zip"])
            stdout.write(body)
        with patch.dict(os.environ, env), patch.object(resume, "command", side_effect=command), \
                patch.object(resume.subprocess, "run", side_effect=download) as downloaded, \
                patch.object(resume.GitHub, "api", side_effect=lambda path: copy.deepcopy(transport[path])), \
                patch.object(resume.GitHub, "release", side_effect=fake.release), \
                patch.object(resume.GitHub, "tag_sha", side_effect=fake.tag_sha), \
                patch.object(resume.GitHub, "latest", side_effect=fake.latest), \
                patch.object(resume.GitHub, "patch", side_effect=fake.patch), \
                patch.object(resume.GitHub, "public_digest", side_effect=fake.public_digest):
            resume.main()
        self.assertEqual(downloaded.call_count, 1)
        self.assertNotIn("upload", fake.events)
        self.assertEqual(fake.events.count("public_get"), 13)
        records = [json.loads(line) for line in (self.root / "native-publication-resume-9999/publication.jsonl").read_text().splitlines()]
        self.assertEqual(records[0]["producer"]["producer_run_id"], RUN_ID)
        self.assertEqual(records[0]["execution"]["workflow_head_sha"], "c" * 40)
        self.assertEqual(records[-1]["phase"], "published_and_verified")
        self.assertEqual((self.root / "native-publication-resume-9999/dist" / MODULE.PROVENANCE_NAME).read_bytes(),
                         (self.directory / MODULE.PROVENANCE_NAME).read_bytes())


class ResumeWorkflowTests(unittest.TestCase):
    def test_closed_main_only_workflow_and_pinned_executable_surface(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        for text in (workflow, workflow.replace("\n", "\r\n")):
            parsed = workflow_surface(text)
            self.assertEqual(set(parsed["jobs"]), {"publish"})
        ci = (ROOT / ".github/workflows/ci-checks.yml").read_text()
        self.assertIn("scripts.test_resume_native_publication", ci)

    def test_permission_trigger_source_transport_and_evidence_changes_are_rejected(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        changes = (("contents: write", "contents: read"), ("actions: read", "actions: write"),
                   ("persist-credentials: false", "persist-credentials: true"),
                   ("ref: ${{ github.sha }}", "ref: main"),
                   ("cancel-in-progress: false", "cancel-in-progress: true"),
                   ("workflow_dispatch:", "push:"),
                   ("if: github.ref", "if: false && github.ref"),
                   ("if: always()", "if: success()"),
                   ("python -B scripts/resume_native_publication.py",
                    "python -B scripts/resume_native_publication.py --upload"),
                   ("fetch-depth: 0", "fetch-depth: 1"),
                   ("GH_TOKEN: ${{ github.token }}", "GH_TOKEN: ${{ secrets.PAT }}"))
        for newline in ("\n", "\r\n"):
            for before, after in changes:
                with self.subTest(before=before, newline=newline), self.assertRaises(AssertionError):
                    workflow_surface(workflow.replace(before, after).replace("\n", newline))
            with self.assertRaises(AssertionError):
                workflow_surface((workflow + "\n  extra:\n    run: echo bad\n").replace("\n", newline))


if __name__ == "__main__":
    unittest.main()
