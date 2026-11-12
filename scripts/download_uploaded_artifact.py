"""Read one upload receipt with stage diagnostics and identity-proven retries.

An isolated 404 is not proof of transient failure. Retries require the same job
credential to list the exact artifact in this run with matching identity first.
No credential, response body, redirect URL, or raw stderr is logged.
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import time
from pathlib import Path

if __package__:
    from .verify_uploaded_artifact import MAX_BUNDLE_BYTES, validate_artifact_identity
else:
    from verify_uploaded_artifact import MAX_BUNDLE_BYTES, validate_artifact_identity


MAX_ATTEMPTS = 3
MAX_PAGES = 5


def emit(**fields):
    print(
        json.dumps({"type": "artifact_readback", **fields}, sort_keys=True), flush=True
    )


class HttpFailure(RuntimeError):
    def __init__(self, stage, status):
        self.status = status
        super().__init__(f"{stage} failed; HTTP status {status or 'unavailable'}")


class GhApi:
    def __init__(self):
        if not os.environ.get("GH_TOKEN"):
            raise ValueError(
                "explicit job GH_TOKEN is required; no credential fallback"
            )
        emit(
            stage="credential_context",
            credential_source="job_GH_TOKEN",
            api_host="github.com",
        )

    @staticmethod
    def _command(endpoint):
        return ["gh", "api", "--hostname", "github.com", endpoint]

    def json(self, stage, endpoint, attempt=1):
        result = subprocess.run(
            [*self._command(endpoint), "--include"],
            capture_output=True,
            timeout=30,
            check=False,
        )
        headers, separator, body = result.stdout.replace(b"\r\n", b"\n").partition(
            b"\n\n"
        )
        status_match = re.match(rb"HTTP/\S+ (\d{3})\b", headers)
        status = int(status_match[1]) if status_match else None
        request = re.search(
            rb"(?im)^x-github-request-id: ([a-z0-9:_-]{1,128})$", headers
        )
        emit(
            stage=stage,
            attempt=attempt,
            http_status=status,
            gh_exit_code=result.returncode,
            request_id=request[1].decode("ascii") if request else None,
        )
        if result.returncode or status != 200 or not separator:
            raise HttpFailure(stage, status)
        document = json.loads(body)
        if not isinstance(document, dict):
            raise TypeError("GitHub JSON response must be an object")
        return document

    def bundle(self, stage, endpoint, path, attempt=1):
        with path.open("xb") as output:
            result = subprocess.run(
                self._command(endpoint),
                stdout=output,
                stderr=subprocess.PIPE,
                timeout=120,
                check=False,
            )
        # gh follows the download redirect; never log its signed Location URL.
        status_match = re.search(rb"\(HTTP (\d{3})\)", result.stderr)
        status = int(status_match[1]) if status_match else None
        emit(
            stage=stage,
            attempt=attempt,
            http_status=status,
            gh_exit_code=result.returncode,
        )
        if result.returncode:
            raise HttpFailure(stage, status)


def download(api, *, repository, root, identity, sleep=time.sleep):
    if re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository) is None:
        raise ValueError("invalid repository identity")
    if re.fullmatch(r"[0-9a-f]{40}", identity["head_sha"]) is None:
        raise ValueError("expected source must be an immutable full SHA")
    if re.fullmatch(r"[0-9a-f]{64}", identity["artifact_digest"]) is None:
        raise ValueError("expected artifact digest must be SHA-256")
    for key in ("artifact_id", "run_id", "repository_id", "head_repository_id"):
        if type(identity[key]) is not int or not 0 < identity[key] < 2**63:
            raise ValueError("expected server IDs must be exact bounded integers")
    if root.is_symlink() or not root.is_dir() or any(root.iterdir()):
        raise ValueError("download root must be a newly created empty owned directory")
    prefix = f"repos/{repository}"
    repository_document = api.json("repository", prefix)
    if repository_document.get("full_name", "").casefold() != repository.casefold():
        raise ValueError("repository response does not match the requested repository")

    def validate(document):
        try:
            validate_artifact_identity(document, repository_document, **identity)
        except (TypeError, ValueError) as error:
            # This validator emits only locally defined contract messages.
            emit(stage="identity", verdict="rejected", reason=str(error))
            raise
        size = document.get("size_in_bytes")
        if type(size) is not int or not 0 < size <= MAX_BUNDLE_BYTES:
            raise ValueError("artifact size is outside the bounded download contract")

    def confirm_in_run():
        # A repository-level search or a name-only lookup cannot establish this.
        seen = set()
        found = []
        for page in range(1, MAX_PAGES + 1):
            listing = api.json(
                "run_artifacts",
                f"{prefix}/actions/runs/{identity['run_id']}/artifacts?per_page=100&page={page}",
            )
            artifacts = listing.get("artifacts")
            total = listing.get("total_count")
            if (
                not isinstance(artifacts, list)
                or len(artifacts) > 100
                or type(total) is not int
                or not 0 <= total <= MAX_PAGES * 100
            ):
                raise ValueError(
                    "run artifact inventory is invalid or exceeds its bound"
                )
            for artifact in artifacts:
                if not isinstance(artifact, dict):
                    raise TypeError("run artifact entry must be an object")
                server_id = artifact.get("id")
                if (
                    type(server_id) is not int
                    or not 0 < server_id < 2**63
                    or server_id in seen
                ):
                    raise ValueError(
                        "run artifact inventory has invalid or duplicate IDs"
                    )
                seen.add(server_id)
                if server_id == identity["artifact_id"]:
                    validate(artifact)
                    found.append(artifact)
            if len(artifacts) < 100 or len(seen) >= total:
                break
        else:
            raise ValueError("run artifact pagination did not finish within its bound")
        if len(found) != 1:
            raise ValueError(
                "upload ID is not proven in the current run; do not retry an unexplained 404"
            )
        return found[0]

    listed = confirm_in_run()

    def read_proven(stage, operation):
        for attempt in range(1, MAX_ATTEMPTS + 1):
            try:
                return operation(attempt)
            except HttpFailure as error:
                if error.status != 404 or attempt == MAX_ATTEMPTS:
                    raise
                confirm_in_run()  # Recheck expiry, identity and same-token access.
                emit(
                    stage=stage,
                    attempt=attempt,
                    retry_reason="exact_artifact_still_proven_in_current_run",
                    delay_seconds=attempt,
                )
                sleep(attempt)
        raise AssertionError("unreachable retry state")

    endpoint = f"{prefix}/actions/artifacts/{identity['artifact_id']}"
    metadata = read_proven(
        "metadata", lambda attempt: api.json("metadata", endpoint, attempt)
    )
    validate(metadata)
    if metadata["size_in_bytes"] != listed["size_in_bytes"]:
        raise ValueError("artifact metadata changed after run inventory proof")
    (root / "metadata.json").write_text(json.dumps(metadata), encoding="utf-8")
    (root / "repository.json").write_text(
        json.dumps(repository_document), encoding="utf-8"
    )

    def bundle(attempt):
        partial = root / f"artifact.part-{attempt}"
        try:
            api.bundle("bundle", f"{endpoint}/zip", partial, attempt)
            if partial.stat().st_size != metadata["size_in_bytes"]:
                raise ValueError("downloaded artifact size mismatch")
            with partial.open("rb") as stream:
                digest = hashlib.file_digest(stream, "sha256").hexdigest()
            if digest != identity["artifact_digest"]:
                raise ValueError("downloaded artifact digest mismatch")
            partial.rename(root / "artifact.zip")
        finally:
            partial.unlink(missing_ok=True)

    read_proven("bundle", bundle)
    emit(
        stage="complete", artifact_id=identity["artifact_id"], run_id=identity["run_id"]
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--root", required=True, type=Path)
    for name in ("artifact-id", "run-id", "repository-id", "head-repository-id"):
        parser.add_argument(f"--{name}", required=True, type=int)
    for name in ("artifact-name", "artifact-digest", "head-sha"):
        parser.add_argument(f"--{name}", required=True)
    args = parser.parse_args()
    identity = {
        key: value
        for key, value in vars(args).items()
        if key not in {"repository", "root"}
    }
    try:
        download(GhApi(), repository=args.repository, root=args.root, identity=identity)
    except (
        ValueError,
        TypeError,
        OSError,
        RuntimeError,
        subprocess.TimeoutExpired,
    ) as error:
        # These messages are locally defined; never include raw gh output.
        emit(stage="failed", error_type=type(error).__name__)
        raise SystemExit(1) from None


if __name__ == "__main__":
    main()
