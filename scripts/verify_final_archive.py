#!/usr/bin/env python3
"""Verify and smoke the exact native archive before artifact upload."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import unicodedata
import zipfile
from pathlib import Path, PurePosixPath

REQUIRED_FILES = (
    "LICENSE",
    "THIRD_PARTY_LICENSES.md",
    "README.md",
    "README.zh-CN.md",
    ".mcp.json",
)
REQUIRED_DIRECTORIES = (
    "assets",
    "skills",
    "plugins",
    ".claude-plugin",
    ".codex-plugin",
)
SUPPORTED_TARGETS = {
    "x86_64-pc-windows-msvc": ("zip", "dcc-cua.exe", "windows"),
    "x86_64-unknown-linux-gnu": ("tar.gz", "dcc-cua", "linux"),
    "aarch64-apple-darwin": ("tar.gz", "dcc-cua", "macos"),
    "x86_64-apple-darwin": ("tar.gz", "dcc-cua", "macos"),
}
MAX_ARCHIVE_ENTRIES = 20_000
MAX_FILE_BYTES = 256 * 1024 * 1024
MAX_TOTAL_BYTES = 1024 * 1024 * 1024
MAX_COMPRESSION_RATIO = 1_000
MAX_PATH_BYTES = 1024
MAX_PATH_DEPTH = 64
MAX_MANIFEST_BYTES = 64 * 1024
MAX_CHECKSUM_BYTES = 1024
VERSION_PATTERN = re.compile(r"^\d+\.\d+\.\d+$")
WINDOWS_RESERVED_NAMES = {
    "CON",
    "PRN",
    "AUX",
    "NUL",
    *(f"COM{index}" for index in range(1, 10)),
    *(f"LPT{index}" for index in range(1, 10)),
}


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _safe_relative_path(
    raw_name: str, *, directory_suffix: bool = False
) -> PurePosixPath:
    if not raw_name or "\x00" in raw_name or "\\" in raw_name:
        raise ValueError("archive contains an unsafe path")
    spelling = (
        raw_name[:-1] if directory_suffix and raw_name.endswith("/") else raw_name
    )
    path = PurePosixPath(spelling)
    canonical = path.as_posix() + ("/" if directory_suffix else "")
    if raw_name != canonical:
        raise ValueError(f"archive contains a non-canonical path: {raw_name!r}")
    unsafe_component = any(
        len(part.encode("utf-8")) > 255
        or part.endswith((" ", "."))
        or part.split(".", 1)[0].upper() in WINDOWS_RESERVED_NAMES
        for part in path.parts
    )
    if (
        not path.parts
        or path.is_absolute()
        or len(raw_name.encode("utf-8")) > MAX_PATH_BYTES
        or len(path.parts) > MAX_PATH_DEPTH
        or unsafe_component
        or any(part in {"", ".", ".."} or ":" in part for part in path.parts)
    ):
        raise ValueError(f"archive contains an unsafe path: {raw_name!r}")
    return path


def _register_path(
    raw_name: str,
    seen: set[str],
    casefolded: set[str],
    *,
    directory_suffix: bool = False,
) -> PurePosixPath:
    path = _safe_relative_path(raw_name, directory_suffix=directory_suffix)
    normalized = path.as_posix()
    folded = unicodedata.normalize("NFC", normalized).casefold()
    if normalized in seen or folded in casefolded:
        raise ValueError(f"archive contains a duplicate path: {normalized}")
    seen.add(normalized)
    casefolded.add(folded)
    return path


def _bounded_size(size: int, total: int) -> int:
    if size < 0 or size > MAX_FILE_BYTES:
        raise ValueError("archive member exceeds the per-file size limit")
    total += size
    if total > MAX_TOTAL_BYTES:
        raise ValueError("archive exceeds the total extracted size limit")
    return total


def _write_member(stream, destination: Path, expected_size: int) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    written = 0
    with destination.open("xb") as output:
        while True:
            chunk = stream.read(min(1024 * 1024, expected_size - written + 1))
            if not chunk:
                break
            written += len(chunk)
            if written > expected_size:
                raise ValueError("archive member expanded beyond its declared size")
            output.write(chunk)
    if written != expected_size:
        raise ValueError("archive member size does not match its declaration")


def _parent_directories(path: PurePosixPath) -> set[str]:
    return {
        PurePosixPath(*path.parts[:index]).as_posix()
        for index in range(1, len(path.parts))
    }


def _extract_zip(archive: Path, destination: Path) -> tuple[set[str], set[str]]:
    files: set[str] = set()
    directories: set[str] = set()
    seen: set[str] = set()
    casefolded: set[str] = set()
    total = 0
    try:
        with zipfile.ZipFile(archive) as bundle:
            entries = bundle.infolist()
            if not entries or len(entries) > MAX_ARCHIVE_ENTRIES:
                raise ValueError("archive entry count is outside the allowed range")
            for entry in entries:
                path = _register_path(
                    entry.filename,
                    seen,
                    casefolded,
                    directory_suffix=entry.is_dir(),
                )
                mode = (entry.external_attr >> 16) & 0xFFFF
                file_type = stat.S_IFMT(mode) if mode else 0
                if file_type == stat.S_IFLNK:
                    raise ValueError("archive links are not allowed")
                if entry.flag_bits & 0x1:
                    raise ValueError("encrypted archive members are not allowed")
                if entry.is_dir():
                    if file_type not in {0, stat.S_IFDIR}:
                        raise ValueError("archive contains a non-directory entry")
                    (destination / Path(*path.parts)).mkdir(parents=True, exist_ok=True)
                    directories.add(path.as_posix())
                    directories.update(_parent_directories(path))
                    continue
                if file_type not in {0, stat.S_IFREG}:
                    raise ValueError("archive contains a non-regular file")
                total = _bounded_size(entry.file_size, total)
                compressed_too_much = entry.file_size > 0 and (
                    entry.compress_size == 0
                    or entry.file_size > entry.compress_size * MAX_COMPRESSION_RATIO
                )
                if compressed_too_much:
                    raise ValueError(
                        "archive member exceeds the compression ratio limit"
                    )
                with bundle.open(entry, "r") as stream:
                    _write_member(
                        stream, destination / Path(*path.parts), entry.file_size
                    )
                files.add(path.as_posix())
                directories.update(_parent_directories(path))
    except (OSError, zipfile.BadZipFile, RuntimeError) as exc:
        raise ValueError("invalid zip archive") from exc
    return files, directories


def _extract_tar(archive: Path, destination: Path) -> tuple[set[str], set[str]]:
    files: set[str] = set()
    directories: set[str] = set()
    seen: set[str] = set()
    casefolded: set[str] = set()
    total = 0
    try:
        with tarfile.open(archive, "r:gz") as bundle:
            entries = bundle.getmembers()
            if not entries or len(entries) > MAX_ARCHIVE_ENTRIES:
                raise ValueError("archive entry count is outside the allowed range")
            for entry in entries:
                path = _register_path(entry.name, seen, casefolded)
                destination_path = destination / Path(*path.parts)
                if entry.isdir():
                    destination_path.mkdir(parents=True, exist_ok=True)
                    directories.add(path.as_posix())
                    directories.update(_parent_directories(path))
                    continue
                if not entry.isfile():
                    raise ValueError("archive links and special files are not allowed")
                total = _bounded_size(entry.size, total)
                stream = bundle.extractfile(entry)
                if stream is None:
                    raise ValueError("archive member cannot be read")
                with stream:
                    _write_member(stream, destination_path, entry.size)
                os.chmod(destination_path, entry.mode & 0o777)
                files.add(path.as_posix())
                directories.update(_parent_directories(path))
    except (OSError, tarfile.TarError) as exc:
        raise ValueError("invalid tar.gz archive") from exc
    return files, directories


def _package_files(source_root: Path, binary_name: str) -> dict[str, Path]:
    entries: dict[str, Path] = {}
    total = 0
    for name in (binary_name, *REQUIRED_FILES):
        path = source_root / name
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"source package is missing a regular file: {name}")
        total = _bounded_size(path.stat().st_size, total)
        entries[name] = path
    for directory_name in REQUIRED_DIRECTORIES:
        directory = source_root / directory_name
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError(f"source package is missing a directory: {directory_name}")
        for current_root, directory_names, file_names in os.walk(
            directory, followlinks=False
        ):
            current = Path(current_root)
            for name in directory_names:
                if (current / name).is_symlink():
                    raise ValueError("source package links are not allowed")
            for name in file_names:
                path = current / name
                if path.is_symlink() or not path.is_file():
                    raise ValueError("source package contains a non-regular file")
                total = _bounded_size(path.stat().st_size, total)
                relative = path.relative_to(source_root).as_posix()
                _safe_relative_path(relative)
                entries[relative] = path
    return entries


def _expected_directories(expected_files: dict[str, Path]) -> set[str]:
    directories = set(REQUIRED_DIRECTORIES)
    for relative in expected_files:
        directories.update(_parent_directories(PurePosixPath(relative)))
    return directories


def _install_plan(
    expected_files: dict[str, Path], expected_directories: set[str]
) -> dict:
    return {
        "directories": sorted(expected_directories),
        "files": [
            {"path": relative, "sha256": _sha256(path)}
            for relative, path in sorted(expected_files.items())
        ],
    }


def _expected_manifest(
    archive: Path,
    target: str,
    version: str,
    digest: str,
    expected_files: dict[str, Path],
    expected_directories: set[str],
) -> dict:
    return {
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
        "install": _install_plan(expected_files, expected_directories),
    }


def _read_bounded_text(path: Path, maximum: int, label: str) -> str:
    try:
        content = path.read_bytes()
    except OSError as exc:
        raise ValueError(f"{label} cannot be read") from exc
    if len(content) > maximum:
        raise ValueError(f"{label} exceeds its size limit")
    try:
        text = content.decode("utf-8")
    except UnicodeError as exc:
        raise ValueError(f"{label} is not valid UTF-8") from exc
    return text.replace("\r\n", "\n").replace("\r", "\n")


def _load_manifest(path: Path) -> dict:
    try:
        document = json.loads(
            _read_bounded_text(path, MAX_MANIFEST_BYTES, "install manifest")
        )
    except ValueError as exc:
        raise ValueError("install manifest is not valid UTF-8 JSON") from exc
    if not isinstance(document, dict):
        raise TypeError("install manifest must be an object")
    return document


def _verify_file_digests(expected: dict[str, Path], root: Path, label: str) -> None:
    for relative, source in expected.items():
        candidate = root / Path(*PurePosixPath(relative).parts)
        if candidate.is_symlink() or not candidate.is_file():
            raise ValueError(f"{label} package is missing a regular file: {relative}")
        if _sha256(candidate) != _sha256(source):
            raise ValueError(f"{label} package digest drift: {relative}")


def _tree_digest(files: dict[str, Path]) -> str:
    digest = hashlib.sha256()
    for relative, path in sorted(files.items()):
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(bytes.fromhex(_sha256(path)))
    return digest.hexdigest()


def _tree_at_root(relative_paths: set[str], root: Path) -> dict[str, Path]:
    return {
        relative: root / Path(*PurePosixPath(relative).parts)
        for relative in relative_paths
    }


def _run(binary: Path, arguments: list[str], cwd: Path) -> subprocess.CompletedProcess:
    environment = os.environ.copy()
    environment["CI"] = "true"
    environment["DCC_CUA_NO_UPDATE_CHECK"] = "true"
    try:
        return subprocess.run(
            [str(binary), *arguments],
            cwd=cwd,
            env=environment,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=30,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        command = " ".join(arguments)
        raise ValueError(f"CLI smoke command could not execute: {command}") from exc


def _json_output(result: subprocess.CompletedProcess, command: str) -> dict:
    try:
        document = json.loads(result.stdout)
    except (TypeError, ValueError) as exc:
        raise ValueError(f"CLI smoke command returned invalid JSON: {command}") from exc
    if not isinstance(document, dict):
        raise TypeError(f"CLI smoke command returned a non-object: {command}")
    return document


def _smoke_binary(root: Path, binary_name: str, target: str, version: str) -> None:
    binary = (root / binary_name).resolve()
    expected_version = f"dcc-cua {version}"
    for arguments in (["--version"], ["version"]):
        result = _run(binary, arguments, root)
        if result.returncode != 0 or result.stdout.strip() != expected_version:
            raise ValueError(f"CLI smoke command failed: {' '.join(arguments)}")

    help_result = _run(binary, ["help"], root)
    if help_result.returncode != 0 or not all(
        token in help_result.stdout for token in ("dcc-cua", "manifest", "doctor")
    ):
        raise ValueError("CLI smoke command failed: help")

    manifest_result = _run(binary, ["manifest"], root)
    if manifest_result.returncode != 0:
        raise ValueError("CLI smoke command failed: manifest")
    runtime_manifest = _json_output(manifest_result, "manifest")
    expected_os = SUPPORTED_TARGETS[target][2]
    if (
        runtime_manifest.get("name") != "dcc-cua"
        or runtime_manifest.get("version") != version
        or runtime_manifest.get("target", {}).get("os") != expected_os
    ):
        raise ValueError("CLI manifest does not match the packaged target")

    doctor_result = _run(binary, ["doctor"], root)
    diagnostics = _json_output(doctor_result, "doctor")
    if doctor_result.returncode not in {0, 1} or (
        diagnostics.get("type") != "diagnostics"
        or diagnostics.get("schema_version") != 1
        or not isinstance(diagnostics.get("ready"), bool)
        or not isinstance(diagnostics.get("routes"), dict)
        or not isinstance(diagnostics.get("checks"), dict)
    ):
        raise ValueError("CLI doctor did not return bounded diagnostics semantics")


def _install_from_manifest(
    manifest: dict,
    extraction: Path,
    install: Path,
) -> None:
    plan = manifest.get("install")
    if not isinstance(plan, dict) or set(plan) != {"directories", "files"}:
        raise ValueError("install manifest does not contain an exact installation plan")
    raw_directories = plan["directories"]
    raw_files = plan["files"]
    if not isinstance(raw_directories, list) or not isinstance(raw_files, list):
        raise TypeError("install manifest plan must contain arrays")

    directories: list[str] = []
    for raw in raw_directories:
        if not isinstance(raw, str):
            raise TypeError("install manifest directory path must be a string")
        directories.append(_safe_relative_path(raw).as_posix())
    if directories != sorted(set(directories)):
        raise ValueError("install manifest directories must be unique and sorted")

    files: list[tuple[str, str]] = []
    for entry in raw_files:
        if not isinstance(entry, dict) or set(entry) != {"path", "sha256"}:
            raise ValueError("install manifest file entry is invalid")
        raw_path = entry["path"]
        digest = entry["sha256"]
        if not isinstance(raw_path, str) or not isinstance(digest, str):
            raise TypeError("install manifest file entry types are invalid")
        relative = _safe_relative_path(raw_path).as_posix()
        if re.fullmatch(r"[0-9a-f]{64}", digest) is None:
            raise ValueError("install manifest file digest is invalid")
        files.append((relative, digest))
    if not files or [relative for relative, _ in files] != sorted(
        {relative for relative, _ in files}
    ):
        raise ValueError("install manifest files must be unique and sorted")

    install.mkdir(parents=False)
    for relative in directories:
        (install / Path(*PurePosixPath(relative).parts)).mkdir(
            parents=True, exist_ok=True
        )
    for relative, expected_digest in files:
        source = extraction / Path(*PurePosixPath(relative).parts)
        if source.is_symlink() or not source.is_file():
            raise ValueError(f"install manifest source is missing: {relative}")
        if _sha256(source) != expected_digest:
            raise ValueError(f"install manifest source digest drift: {relative}")
        destination = install / Path(*PurePosixPath(relative).parts)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        if _sha256(destination) != expected_digest:
            raise ValueError(f"installed manifest digest drift: {relative}")


def _archive_identity(path: Path) -> tuple[int, int, int, int]:
    metadata = path.stat()
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_size,
        metadata.st_mtime_ns,
    )


def _verify_snapshot(
    *,
    archive: Path,
    extension: str,
    binary_name: str,
    expected_files: dict[str, Path],
    manifest: dict,
    target: str,
    version: str,
    digest: str,
    extract_root: Path,
    install_root: Path,
) -> dict:
    extract_root.mkdir(parents=False)
    archive_files, archive_directories = (
        _extract_zip(archive, extract_root)
        if extension == "zip"
        else _extract_tar(archive, extract_root)
    )
    expected_names = set(expected_files)
    expected_directories = _expected_directories(expected_files)
    if archive_files != expected_names or archive_directories != expected_directories:
        missing = sorted(expected_names - archive_files)
        extra = sorted(archive_files - expected_names)
        missing_directories = sorted(expected_directories - archive_directories)
        extra_directories = sorted(archive_directories - expected_directories)
        raise ValueError(
            "archive layout mismatch; "
            f"missing={missing}; extra={extra}; "
            f"missing_directories={missing_directories}; "
            f"extra_directories={extra_directories}"
        )
    _verify_file_digests(expected_files, extract_root, "archive")
    _smoke_binary(extract_root, binary_name, target, version)

    _install_from_manifest(manifest, extract_root, install_root)
    installed_names = {
        path.relative_to(install_root).as_posix()
        for path in install_root.rglob("*")
        if path.is_file()
    }
    if installed_names != expected_names:
        raise ValueError("installed layout does not match the archive")
    installed_directories = {
        path.relative_to(install_root).as_posix()
        for path in install_root.rglob("*")
        if path.is_dir()
    }
    if installed_directories != expected_directories:
        raise ValueError("installed directory topology does not match the archive")
    _verify_file_digests(expected_files, install_root, "installed")
    _smoke_binary(install_root, binary_name, target, version)
    source_tree_digest = _tree_digest(expected_files)
    archive_tree_digest = _tree_digest(_tree_at_root(expected_names, extract_root))
    install_tree_digest = _tree_digest(_tree_at_root(expected_names, install_root))
    if len({source_tree_digest, archive_tree_digest, install_tree_digest}) != 1:
        raise ValueError("source, archive, and installed tree digests differ")
    return {
        "schema_version": 1,
        "type": "final_archive_verified",
        "target": target,
        "version": version,
        "archive": {"name": archive.name, "sha256": digest},
        "file_count": len(expected_names),
        "source_tree_sha256": source_tree_digest,
        "archive_tree_sha256": archive_tree_digest,
        "install_tree_sha256": install_tree_digest,
        "smoke_commands": ["--version", "version", "help", "manifest", "doctor"],
    }


def verify_final_archive(
    *,
    source_root: Path,
    archive: Path,
    manifest_path: Path,
    checksum_path: Path,
    target: str,
    version: str,
    extract_root: Path,
    install_root: Path,
) -> dict:
    if target not in SUPPORTED_TARGETS:
        raise ValueError(f"unsupported release target: {target}")
    if VERSION_PATTERN.fullmatch(version) is None:
        raise ValueError("release version must be stable semver")
    extension, binary_name, _ = SUPPORTED_TARGETS[target]
    expected_archive_name = f"dcc-cua-{version}-{target}.{extension}"
    if archive.name != expected_archive_name or not archive.is_file():
        raise ValueError("archive identity does not match the target and version")
    if extract_root.exists() or install_root.exists():
        raise ValueError("verification roots must be fresh and absent")

    expected_files = _package_files(source_root, binary_name)
    expected_directories = _expected_directories(expected_files)
    initial_identity = _archive_identity(archive)
    if initial_identity[2] > MAX_TOTAL_BYTES:
        raise ValueError("archive exceeds the compressed size limit")
    digest = _sha256(archive)
    manifest = _load_manifest(manifest_path)
    if manifest != _expected_manifest(
        archive,
        target,
        version,
        digest,
        expected_files,
        expected_directories,
    ):
        raise ValueError("install manifest does not match the exact archive")
    checksum = _read_bounded_text(checksum_path, MAX_CHECKSUM_BYTES, "checksum sidecar")
    if checksum != f"{digest}  {archive.name}\n":
        raise ValueError("checksum sidecar does not match the exact archive")
    with tempfile.TemporaryDirectory(
        prefix=".dcc-cua-final-archive-snapshot-", dir=archive.parent
    ) as snapshot_directory:
        snapshot = Path(snapshot_directory) / archive.name
        shutil.copyfile(archive, snapshot)
        if (
            _archive_identity(archive) != initial_identity
            or _sha256(archive) != digest
            or _sha256(snapshot) != digest
        ):
            raise ValueError("archive changed during verification snapshot creation")
        receipt = _verify_snapshot(
            archive=snapshot,
            extension=extension,
            binary_name=binary_name,
            expected_files=expected_files,
            manifest=manifest,
            target=target,
            version=version,
            digest=digest,
            extract_root=extract_root,
            install_root=install_root,
        )
        try:
            unchanged = (
                _archive_identity(archive) == initial_identity
                and _sha256(archive) == digest
            )
        except OSError:
            unchanged = False
        if not unchanged:
            raise ValueError("archive changed during verification")
        return receipt


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--checksum", type=Path, required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--extract-root", type=Path, required=True)
    parser.add_argument("--install-root", type=Path, required=True)
    args = parser.parse_args()
    receipt = verify_final_archive(
        source_root=args.source_root,
        archive=args.archive,
        manifest_path=args.manifest,
        checksum_path=args.checksum,
        target=args.target,
        version=args.version,
        extract_root=args.extract_root,
        install_root=args.install_root,
    )
    print(json.dumps(receipt, sort_keys=True))


if __name__ == "__main__":
    main()
