import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import tempfile
import zipfile
from collections.abc import Callable
from pathlib import Path, PurePosixPath

if __package__:
    from .verify_final_archive import verify_final_archive
else:
    from verify_final_archive import verify_final_archive


SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")
MAX_BUNDLE_BYTES = 512 * 1024 * 1024
MAX_MEMBER_BYTES = 256 * 1024 * 1024
MAX_COMPRESSION_RATIO = 200
MAX_SERVER_ID = 2**63 - 1


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _canonical_root_name(name: str) -> bool:
    if "\\" in name or name.startswith("/"):
        return False
    parsed = PurePosixPath(name)
    return (
        len(parsed.parts) == 1
        and parsed.parts[0] not in ("", ".", "..")
        and parsed.as_posix() == name
    )


def _require_server_id(value: object, field: str) -> int:
    if type(value) is not int or not 1 <= value <= MAX_SERVER_ID:
        raise ValueError(f"{field} ID must be an exact bounded positive integer")
    return value


def _validate_metadata(
    document: dict,
    repository_document: dict,
    *,
    artifact_id: int,
    artifact_name: str,
    artifact_digest: str,
    run_id: int,
    head_sha: str,
    repository_id: int,
    head_repository_id: int,
    bundle_size: int,
) -> None:
    _require_server_id(repository_document.get("id"), "artifact repository")
    if repository_document.get("id") != repository_id:
        raise ValueError("artifact repository identity does not match the workflow")
    _require_server_id(document.get("id"), "artifact metadata")
    if document.get("id") != artifact_id:
        raise ValueError("artifact metadata ID does not match the upload output")
    if document.get("name") != artifact_name:
        raise ValueError("artifact metadata name does not match the upload contract")
    if document.get("expired") is not False:
        raise ValueError("artifact metadata reports an expired artifact")
    if document.get("size_in_bytes") != bundle_size:
        raise ValueError("artifact metadata size does not match the downloaded bundle")
    if document.get("digest") != f"sha256:{artifact_digest}":
        raise ValueError("artifact digest does not match server metadata")
    workflow_run = document.get("workflow_run")
    if not isinstance(workflow_run, dict):
        raise TypeError("artifact workflow run must be a JSON object")
    _require_server_id(workflow_run.get("id"), "artifact workflow run")
    if workflow_run.get("id") != run_id:
        raise ValueError("artifact workflow run does not match the current run")
    if workflow_run.get("head_sha") != head_sha:
        raise ValueError("artifact workflow head does not match the current run")
    _require_server_id(workflow_run.get("repository_id"), "workflow repository")
    if workflow_run.get("repository_id") != repository_id:
        raise ValueError("workflow repository identity does not match the workflow")
    _require_server_id(
        workflow_run.get("head_repository_id"), "workflow head repository"
    )
    if workflow_run.get("head_repository_id") != head_repository_id:
        raise ValueError(
            "workflow head repository identity does not match the workflow"
        )


def _extract_exact_bundle(snapshot: Path, output_root: Path, names: set[str]) -> None:
    with zipfile.ZipFile(snapshot) as archive:
        members = archive.infolist()
        raw_names = [member.filename for member in members]
        if (
            len(raw_names) != len(set(raw_names))
            or set(raw_names) != names
            or any(not _canonical_root_name(name) for name in raw_names)
        ):
            raise ValueError("artifact members do not match the exact canonical bundle")
        total_size = 0
        for member in members:
            mode = (member.external_attr >> 16) & 0o170000
            if member.is_dir() or stat.S_ISLNK(mode) or member.flag_bits & 0x1:
                raise ValueError("artifact members must be regular unencrypted files")
            if member.file_size > MAX_MEMBER_BYTES:
                raise ValueError("artifact member exceeds the size limit")
            if member.file_size and member.compress_size == 0:
                raise ValueError("artifact member has an invalid compressed size")
            if (
                member.compress_size
                and member.file_size > member.compress_size * MAX_COMPRESSION_RATIO
            ):
                raise ValueError("artifact member exceeds the compression ratio limit")
            total_size += member.file_size
            if total_size > MAX_BUNDLE_BYTES:
                raise ValueError("artifact contents exceed the total size limit")

        output_root.mkdir(parents=True, exist_ok=False)
        for member in members:
            destination = output_root / member.filename
            with archive.open(member) as source, destination.open("xb") as target:
                shutil.copyfileobj(source, target, length=1024 * 1024)


def verify_uploaded_artifact(
    *,
    metadata_path: Path,
    repository_metadata_path: Path,
    bundle_path: Path,
    output_root: Path,
    expected_artifact_id: int,
    expected_artifact_name: str,
    expected_artifact_digest: str,
    expected_run_id: int,
    expected_head_sha: str,
    expected_repository_id: int,
    expected_head_repository_id: int,
    archive_name: str,
    manifest_name: str,
    source_root: Path,
    target: str,
    version: str,
    extract_root: Path,
    install_root: Path,
    after_snapshot: Callable[[], None] | None = None,
) -> dict:
    _require_server_id(expected_artifact_id, "expected artifact")
    _require_server_id(expected_run_id, "expected workflow run")
    _require_server_id(expected_repository_id, "expected repository")
    _require_server_id(expected_head_repository_id, "expected head repository")
    if SHA256_PATTERN.fullmatch(expected_artifact_digest) is None:
        raise ValueError("artifact digest must be a lowercase SHA-256 value")
    if re.fullmatch(r"[0-9a-f]{40}", expected_head_sha) is None:
        raise ValueError("workflow head must be a full lowercase commit SHA")
    expected_names = {archive_name, f"{archive_name}.sha256", manifest_name}
    if any(not _canonical_root_name(name) for name in expected_names):
        raise ValueError("expected artifact member names must be canonical basenames")
    if output_root.exists() or extract_root.exists() or install_root.exists():
        raise ValueError("artifact verification roots must be fresh and absent")
    if (
        not metadata_path.is_file()
        or not repository_metadata_path.is_file()
        or not bundle_path.is_file()
    ):
        raise ValueError(
            "artifact metadata, repository metadata, and downloaded bundle are required"
        )

    metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    if not isinstance(metadata, dict):
        raise TypeError("artifact metadata must be a JSON object")
    repository_metadata = json.loads(
        repository_metadata_path.read_text(encoding="utf-8")
    )
    if not isinstance(repository_metadata, dict):
        raise TypeError("repository metadata must be a JSON object")
    initial_stat = bundle_path.stat()
    if initial_stat.st_size > MAX_BUNDLE_BYTES:
        raise ValueError("downloaded artifact bundle exceeds the size limit")

    output_root.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=".dcc-cua-uploaded-artifact-snapshot-", dir=output_root.parent
    ) as snapshot_directory:
        snapshot = Path(snapshot_directory) / "artifact.zip"
        with bundle_path.open("rb") as source, snapshot.open("xb") as target_stream:
            shutil.copyfileobj(source, target_stream, length=1024 * 1024)
            target_stream.flush()
            os.fsync(target_stream.fileno())
        if after_snapshot is not None:
            after_snapshot()

        snapshot_digest = _sha256(snapshot)
        if snapshot_digest != expected_artifact_digest:
            raise ValueError("artifact digest does not match the upload output")
        _validate_metadata(
            metadata,
            repository_metadata,
            artifact_id=expected_artifact_id,
            artifact_name=expected_artifact_name,
            artifact_digest=expected_artifact_digest,
            run_id=expected_run_id,
            head_sha=expected_head_sha,
            repository_id=expected_repository_id,
            head_repository_id=expected_head_repository_id,
            bundle_size=snapshot.stat().st_size,
        )
        _extract_exact_bundle(snapshot, output_root, expected_names)
        final_receipt = verify_final_archive(
            source_root=source_root,
            archive=output_root / archive_name,
            manifest_path=output_root / manifest_name,
            checksum_path=output_root / f"{archive_name}.sha256",
            target=target,
            version=version,
            extract_root=extract_root,
            install_root=install_root,
        )
        try:
            unchanged = (
                bundle_path.stat().st_size == initial_stat.st_size
                and bundle_path.stat().st_mtime_ns == initial_stat.st_mtime_ns
                and _sha256(bundle_path) == expected_artifact_digest
                and _sha256(snapshot) == expected_artifact_digest
            )
        except OSError:
            unchanged = False
        if not unchanged:
            raise ValueError("downloaded artifact changed during verification")

    return {
        "schema_version": 1,
        "type": "uploaded_final_archive_verified",
        "artifact_id": expected_artifact_id,
        "artifact_name": expected_artifact_name,
        "artifact_digest": expected_artifact_digest,
        "workflow_run_id": expected_run_id,
        "workflow_head_sha": expected_head_sha,
        "repository_id": expected_repository_id,
        "head_repository_id": expected_head_repository_id,
        "final_archive": final_receipt,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--metadata", type=Path, required=True)
    parser.add_argument("--repository-metadata", type=Path, required=True)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    parser.add_argument("--artifact-id", type=int, required=True)
    parser.add_argument("--artifact-name", required=True)
    parser.add_argument("--artifact-digest", required=True)
    parser.add_argument("--run-id", type=int, required=True)
    parser.add_argument("--head-sha", required=True)
    parser.add_argument("--repository-id", type=int, required=True)
    parser.add_argument("--head-repository-id", type=int, required=True)
    parser.add_argument("--archive-name", required=True)
    parser.add_argument("--manifest-name", required=True)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--extract-root", type=Path, required=True)
    parser.add_argument("--install-root", type=Path, required=True)
    args = parser.parse_args()
    receipt = verify_uploaded_artifact(
        metadata_path=args.metadata,
        repository_metadata_path=args.repository_metadata,
        bundle_path=args.bundle,
        output_root=args.output_root,
        expected_artifact_id=args.artifact_id,
        expected_artifact_name=args.artifact_name,
        expected_artifact_digest=args.artifact_digest,
        expected_run_id=args.run_id,
        expected_head_sha=args.head_sha,
        expected_repository_id=args.repository_id,
        expected_head_repository_id=args.head_repository_id,
        archive_name=args.archive_name,
        manifest_name=args.manifest_name,
        source_root=args.source_root,
        target=args.target,
        version=args.version,
        extract_root=args.extract_root,
        install_root=args.install_root,
    )
    print(json.dumps(receipt, sort_keys=True))


if __name__ == "__main__":
    main()
