#!/usr/bin/env python3
"""Fail closed unless a CI worktree still contains the selected commit bytes."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import subprocess
from pathlib import Path

SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")


def _git(repository: Path, *arguments: str, text: bool = True):
    return subprocess.run(
        ["git", "-C", str(repository), *arguments],
        check=False,
        capture_output=True,
        text=text,
    )


def _selected_tree(repository: Path, expected: str) -> dict[str, tuple[str, str]]:
    result = _git(repository, "ls-tree", "-rz", "--full-tree", expected, text=False)
    if result.returncode != 0:
        raise ValueError("selected source tree cannot be read")
    entries: dict[str, tuple[str, str]] = {}
    for record in result.stdout.split(b"\0"):
        if not record:
            continue
        metadata, separator, raw_path = record.partition(b"\t")
        fields = metadata.split()
        if not separator or len(fields) != 3 or fields[1] != b"blob":
            raise ValueError("selected source tree contains an unsupported entry")
        try:
            path = raw_path.decode("utf-8")
        except UnicodeError as exc:
            raise ValueError("selected source path is not valid UTF-8") from exc
        mode = fields[0].decode("ascii")
        object_id = fields[2].decode("ascii")
        if path in entries:
            raise ValueError("selected source tree contains a duplicate path")
        entries[path] = (mode, object_id)
    if not entries:
        raise ValueError("selected source tree is empty")
    return entries


def _index_tree(repository: Path) -> dict[str, tuple[str, str]]:
    result = _git(repository, "ls-files", "--stage", "-z", text=False)
    if result.returncode != 0:
        raise ValueError("checkout index cannot be read")
    entries: dict[str, tuple[str, str]] = {}
    for record in result.stdout.split(b"\0"):
        if not record:
            continue
        metadata, separator, raw_path = record.partition(b"\t")
        fields = metadata.split()
        if not separator or len(fields) != 3 or fields[2] != b"0":
            raise ValueError("checkout index contains an unmerged or invalid entry")
        try:
            path = raw_path.decode("utf-8")
        except UnicodeError as exc:
            raise ValueError("checkout index path is not valid UTF-8") from exc
        entries[path] = (fields[0].decode("ascii"), fields[1].decode("ascii"))
    return entries


def _symlink_blob_id(path: Path, relative: str) -> str:
    if not path.is_symlink():
        raise ValueError(f"checkout source type drift: {relative}")
    content = os.fsencode(os.readlink(path))
    digest = hashlib.sha1(usedforsecurity=False)
    digest.update(f"blob {len(content)}\0".encode("ascii"))
    digest.update(content)
    return digest.hexdigest()


def _worktree_tree(
    repository: Path, selected: dict[str, tuple[str, str]]
) -> dict[str, tuple[str, str]]:
    entries: dict[str, tuple[str, str]] = {}
    regular_paths: list[str] = []
    regular_modes: dict[str, str] = {}
    for relative, (mode, _) in selected.items():
        if any(character in relative for character in "\0\r\n"):
            raise ValueError("selected source path cannot be batch hashed safely")
        path = repository / Path(*relative.split("/"))
        metadata = path.lstat()
        if mode == "120000":
            entries[relative] = (mode, _symlink_blob_id(path, relative))
            continue
        if path.is_symlink() or not stat.S_ISREG(metadata.st_mode):
            raise ValueError(f"checkout source type drift: {relative}")
        if os.name != "nt" and bool(metadata.st_mode & 0o111) != (mode == "100755"):
            raise ValueError(f"checkout source mode drift: {relative}")
        regular_paths.append(relative)
        regular_modes[relative] = mode

    result = subprocess.run(
        [
            "git",
            "-C",
            str(repository),
            "hash-object",
            "--stdin-paths",
            "--filters",
        ],
        input="\n".join(regular_paths),
        check=False,
        capture_output=True,
        text=True,
    )
    object_ids = result.stdout.splitlines()
    if result.returncode != 0 or len(object_ids) != len(regular_paths):
        raise ValueError("checkout source bytes cannot be batch hashed")
    for relative, object_id in zip(regular_paths, object_ids, strict=True):
        entries[relative] = (regular_modes[relative], object_id)
    return entries


def verify_source_integrity(repository: Path, expected: str) -> dict:
    if SHA_PATTERN.fullmatch(expected) is None:
        raise ValueError("expected source identity must be a lowercase full commit SHA")
    repository = repository.resolve(strict=True)
    head = _git(repository, "rev-parse", "--verify", "HEAD^{commit}")
    if head.returncode != 0 or head.stdout.strip() != expected:
        raise ValueError("checkout source identity mismatch")

    selected = _selected_tree(repository, expected)
    if _index_tree(repository) != selected:
        raise ValueError("checkout index differs from the selected source")
    try:
        worktree = _worktree_tree(repository, selected)
    except OSError as exc:
        raise ValueError("checkout worktree cannot be read exactly") from exc
    if worktree != selected:
        raise ValueError("checkout worktree bytes differ from the selected source")

    untracked = _git(
        repository,
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
        text=False,
    )
    if untracked.returncode != 0:
        raise ValueError("checkout untracked-file inventory could not be read")
    unexpected = [entry for entry in untracked.stdout.split(b"\0") if entry]
    if unexpected:
        raise ValueError("checkout contains unexpected untracked source files")

    return {"type": "ci_source_integrity", "head_sha": expected, "clean": True}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", type=Path, required=True)
    parser.add_argument("--expected", required=True)
    args = parser.parse_args()
    print(
        json.dumps(
            verify_source_integrity(args.repository, args.expected), sort_keys=True
        )
    )


if __name__ == "__main__":
    main()
