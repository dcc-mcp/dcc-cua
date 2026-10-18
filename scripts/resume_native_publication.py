#!/usr/bin/env python3
"""Resume visibility of a complete native draft using its original workflow artifact.

Never rebuild, upload, overwrite, or rewrite provenance. A failed producer and
the new publication execution retain separate identities in the evidence.
"""

from __future__ import annotations

from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import subprocess

try:
    from scripts import release_integrity as integrity
    from scripts.publish_verified_release import GitHub, publish
except ModuleNotFoundError:
    import release_integrity as integrity
    from publish_verified_release import GitHub, publish


BUILD_JOBS = (
    "build (windows-latest, windows-x86_64, x86_64-pc-windows-msvc, zip)",
    "build (ubuntu-latest, linux-x86_64, x86_64-unknown-linux-gnu, tar.gz)",
    "build (macos-26, macos-aarch64, aarch64-apple-darwin, tar.gz, arm64)",
    "build (macos-26-intel, macos-x86_64, x86_64-apple-darwin, tar.gz, x86_64)",
)
JOB_CONCLUSIONS = {
    **dict.fromkeys(("validate", "release-please", "consolidate-native", *BUILD_JOBS), "success"),
    "attach-assets": "failure",
    **dict.fromkeys(("package-browser-extension", "attach-browser-extension-assets",
                     "publish-edge-addons", "publish-chrome-web-store", "publish-firefox-addons"), "skipped"),
}


def positive_id(value):
    if type(value) is not int or value <= 0:
        raise ValueError("identity must be a positive integer")
    return value


def request_from_environment(env):
    if (env.get("GITHUB_EVENT_NAME") != "workflow_dispatch"
            or env.get("GITHUB_REF") != "refs/heads/main"
            or env.get("GITHUB_RUN_ATTEMPT") != "1"):
        raise ValueError("publication resume requires a new main-only dispatch")
    request = {key: env[key.upper()] for key in (
        "producer_run_id", "producer_head_sha", "artifact_id", "artifact_digest",
        "tag", "source_sha", "release_id",
    )}
    for key in ("producer_run_id", "artifact_id", "release_id"):
        if not re.fullmatch(r"[1-9][0-9]*", request[key]):
            raise ValueError("dispatch identities must be canonical positive integers")
        request[key] = positive_id(int(request[key]))
    for key in ("producer_head_sha", "source_sha"):
        integrity._require_sha(request[key], key)
    integrity._require_sha(env["GITHUB_SHA"], "executing workflow head")
    if not re.fullmatch(r"v\d+\.\d+\.\d+", request["tag"]):
        raise ValueError("resume requires an exact stable native tag")
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", request["artifact_digest"]):
        raise ValueError("resume requires the immutable transport digest")
    if int(env["GITHUB_RUN_ID"]) == request["producer_run_id"]:
        raise ValueError("publication execution must differ from the failed producer")
    return request


def validate_producer(run, jobs, request, repository, repository_id):
    expected = dict(id=request["producer_run_id"], run_attempt=1,
                    head_sha=request["producer_head_sha"], head_branch="main",
                    path=".github/workflows/release-please.yml", event="workflow_dispatch",
                    status="completed", conclusion="failure")
    if any(run.get(key) != value for key, value in expected.items()):
        raise ValueError("producer is not the exact failed official native recovery")
    for key in ("repository", "head_repository"):
        if (run.get(key, {}).get("id") != repository_id
                or run.get(key, {}).get("full_name") != repository):
            raise ValueError("producer repository identity mismatch")
    rows = jobs.get("jobs", [])
    if jobs.get("total_count") != len(rows) or len(rows) != len(JOB_CONCLUSIONS):
        raise ValueError("producer job listing is incomplete")
    by_name = {row["name"]: row for row in rows}
    if len(by_name) != len(rows) or set(by_name) != set(JOB_CONCLUSIONS):
        raise ValueError("producer native job graph differs from the reviewed graph")
    for name, conclusion in JOB_CONCLUSIONS.items():
        row = by_name[name]
        positive_id(row.get("id"))
        if (row.get("run_id") != request["producer_run_id"] or row.get("run_attempt") != 1
                or row.get("head_sha") != request["producer_head_sha"]
                or row.get("status") != "completed" or row.get("conclusion") != conclusion):
            raise ValueError("producer job was not executed at the required identity/outcome")
    if len({row["id"] for row in rows}) != len(rows):
        raise ValueError("producer job identities are not unique")
    return by_name


def validate_bundle(directory, artifact, listing, request, repository_id):
    integrity.verify_workflow_artifact(
        artifact, expected_id=request["artifact_id"], expected_name="dcc-cua-native-release",
        expected_digest=request["artifact_digest"], expected_run_id=request["producer_run_id"],
        expected_head_sha=request["producer_head_sha"],
    )
    for key in ("repository_id", "head_repository_id"):
        if artifact["workflow_run"].get(key) != repository_id:
            raise ValueError("aggregate artifact repository identity mismatch")
    integrity.verify_release_provenance(
        directory, version=request["tag"][1:], tag=request["tag"],
        source_sha=request["source_sha"], release_target_sha=request["source_sha"],
        workflow_head_sha=request["producer_head_sha"],
        provenance_path=directory / integrity.PROVENANCE_NAME,
    )
    provenance = json.loads((directory / integrity.PROVENANCE_NAME).read_text())
    if provenance["workflow_run_id"] != request["producer_run_id"]:
        raise ValueError("provenance must retain the original producer run")
    rows = listing.get("artifacts", [])
    if (listing.get("total_count") != len(rows)
            or len({row["id"] for row in rows}) != len(rows)
            or len({row["name"] for row in rows}) != len(rows)):
        raise ValueError("producer artifact listing is incomplete or ambiguous")
    if [row for row in rows if row["id"] == request["artifact_id"]] != [artifact]:
        raise ValueError("aggregate artifact differs from its owning run listing")
    # The aggregate shares the native prefix; select only the four original builds.
    selected = [row for row in rows if row["name"] in integrity.EXPECTED_BUILD_ARTIFACTS]
    for row in selected:
        for key in ("repository_id", "head_repository_id"):
            if row.get("workflow_run", {}).get(key) != repository_id:
                raise ValueError("build artifact repository identity mismatch")
    facts = integrity.native_build_artifact_facts(
        dict(total_count=len(selected), artifacts=selected),
        workflow_run_id=request["producer_run_id"], source_sha=request["producer_head_sha"],
    )
    if facts != provenance["build_artifacts"]:
        raise ValueError("original build artifacts differ from immutable provenance")
    return provenance


def validate_archive_receipt(log, fact, asset, request, repository_id):
    receipts = []
    for line in log.splitlines():
        # GitHub raw job logs prefix each emitted JSON line with an ISO timestamp.
        match = re.fullmatch(r"\d{4}-\d\d-\d\dT\S+Z (\{.*\})", line)
        if match:
            try:
                document = json.loads(match.group(1))
            except ValueError:
                continue
            if document.get("type") == "uploaded_final_archive_verified":
                receipts.append(document)
    if len(receipts) != 1:
        raise ValueError("native build must have one actual immutable archive receipt")
    receipt = receipts[0]
    expected = dict(schema_version=1, type="uploaded_final_archive_verified",
                    artifact_id=fact["id"], artifact_name=fact["name"], artifact_digest=fact["sha256"],
                    repository_id=repository_id, head_repository_id=repository_id,
                    workflow_run_id=request["producer_run_id"], workflow_head_sha=request["producer_head_sha"])
    if any(receipt.get(key) != value for key, value in expected.items()):
        raise ValueError("native archive receipt identity differs from original artifact")
    final = receipt.get("final_archive", {})
    expected = dict(schema_version=1, type="final_archive_verified", version=request["tag"][1:],
                    target=asset["target"], archive=dict(name=asset["archive"], sha256=asset["sha256"]),
                    smoke_commands=["--version", "version", "help", "manifest", "doctor"])
    if any(final.get(key) != value for key, value in expected.items()):
        raise ValueError("native archive receipt contents differ from original release")
    digest = final.get("source_tree_sha256", "")
    if (not re.fullmatch(r"[0-9a-f]{64}", digest)
            or final.get("archive_tree_sha256") != digest or final.get("install_tree_sha256") != digest
            or type(final.get("file_count")) is not int or final["file_count"] <= 0):
        raise ValueError("native archive did not preserve the full source/install tree")
    return receipt


class ExistingAssetsGitHub(GitHub):
    def __init__(self, repository, release_id):
        super().__init__(repository)
        self.release_id = release_id
        self.asset_identities = None

    def release(self, tag):
        document = super().release(tag)
        if document.get("id") != self.release_id or not document.get("assets"):
            raise ValueError("resume requires the exact existing complete release")
        identities = sorted((asset["id"], asset["name"], asset["size"], asset["digest"])
                            for asset in document["assets"])
        if self.asset_identities is None:
            self.asset_identities = identities
        elif identities != self.asset_identities:
            raise ValueError("original uploaded asset identities changed during resume")
        return document

    def upload(self, tag, paths):
        raise ValueError("publication-only resume cannot upload assets")


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def main():
    request = request_from_environment(os.environ)
    repository = os.environ["GITHUB_REPOSITORY"]
    repository_id = positive_id(int(os.environ["GITHUB_REPOSITORY_ID"]))
    gateway = ExistingAssetsGitHub(repository, request["release_id"])
    root = Path(os.environ["RUNNER_TEMP"]) / f"native-publication-resume-{os.environ['GITHUB_RUN_ID']}"
    root.mkdir()  # Existing evidence or an uncertain earlier attempt cannot be reused.
    with (root / "publication.jsonl").open("x", encoding="utf-8") as stream:
        def record(event):
            stream.write(json.dumps(dict(at=datetime.now(timezone.utc).isoformat(), **event), sort_keys=True) + "\n")
            stream.flush()
        record(dict(phase="resume_requested", producer=request, execution=dict(
            run_id=os.environ["GITHUB_RUN_ID"], workflow_head_sha=os.environ["GITHUB_SHA"], attempt=1)))
        try:
            if command("git", "rev-parse", "HEAD") != os.environ["GITHUB_SHA"]:
                raise ValueError("executing tools differ from the reviewed main dispatch")
            for older, newer in ((request["source_sha"], request["producer_head_sha"]),
                                 (request["producer_head_sha"], os.environ["GITHUB_SHA"])):
                command("git", "merge-base", "--is-ancestor", older, newer)
            if command("git", "show", f"{request['source_sha']}:version.txt") != request["tag"][1:]:
                raise ValueError("tagged source version differs from the requested release")
            def metadata(path, name):
                document = gateway.api(path)
                (root / name).write_text(json.dumps(document, sort_keys=True) + "\n")
                return document
            run = metadata(f"actions/runs/{request['producer_run_id']}", "producer-run.json")
            jobs = metadata(f"actions/runs/{request['producer_run_id']}/attempts/1/jobs?per_page=100", "producer-jobs.json")
            by_name = validate_producer(run, jobs, request, repository, repository_id)
            artifact = metadata(f"actions/artifacts/{request['artifact_id']}", "artifact.json")
            # Authenticate identity before downloading or extracting any transport.
            integrity.verify_workflow_artifact(artifact, expected_id=request["artifact_id"],
                expected_name="dcc-cua-native-release", expected_digest=request["artifact_digest"],
                expected_run_id=request["producer_run_id"], expected_head_sha=request["producer_head_sha"])
            listing = metadata(f"actions/runs/{request['producer_run_id']}/artifacts?per_page=100", "producer-artifacts.json")
            bundle = root / "artifact.zip"
            with bundle.open("xb") as output:
                subprocess.run(["gh", "api", f"repos/{repository}/actions/artifacts/{request['artifact_id']}/zip"],
                               stdout=output, check=True)
            if bundle.stat().st_size > integrity.MAX_TOTAL_BYTES:
                raise ValueError("aggregate artifact transport exceeds the bounded limit")
            directory = root / "dist"
            integrity.verify_and_extract_artifact(bundle, request["artifact_digest"], directory)
            provenance = validate_bundle(directory, artifact, listing, request, repository_id)
            for name, fact, asset in zip(BUILD_JOBS, provenance["build_artifacts"], provenance["assets"], strict=True):
                job_id = by_name[name]["id"]
                log = command("gh", "api", f"repos/{repository}/actions/jobs/{job_id}/logs")
                (root / f"build-{job_id}.log").write_text(log, encoding="utf-8")
                record(dict(phase="original_archive_verified", receipt=validate_archive_receipt(
                    log, fact, asset, request, repository_id)))
            initial = gateway.release(request["tag"])
            if initial.get("isDraft") is not True or initial.get("isPrerelease") is not False:
                raise ValueError("publication-only admission requires the complete original draft")
            integrity.verify_uploaded_native_release(initial, directory, version=request["tag"][1:],
                tag=request["tag"], source_sha=request["source_sha"], workflow_head_sha=request["producer_head_sha"])
            (root / "release-before.json").write_text(json.dumps(initial, sort_keys=True) + "\n")
            record(dict(phase="original_artifact_admitted", producer=request,
                        asset_ids=sorted(asset["id"] for asset in initial["assets"])))
            publish(gateway, directory, component="native", version=request["tag"][1:],
                    tag=request["tag"], native_tag=request["tag"], source_sha=request["source_sha"],
                    workflow_head_sha=request["producer_head_sha"], record=record)
        except Exception as error:
            record(dict(phase="failed", error_type=type(error).__name__, message=str(error)))
            raise


if __name__ == "__main__":
    main()
