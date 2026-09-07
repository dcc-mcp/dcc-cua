"""Filesystem predicates used by release and packaging validators."""

from __future__ import annotations

import stat
from pathlib import Path


def is_reparse_point(path: Path) -> bool:
    """Return whether *path* is a symlink or Windows reparse point."""
    status = path.lstat()
    return _status_is_reparse(status)


def _status_is_reparse(status: object) -> bool:
    reparse_flag = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
    return stat.S_ISLNK(getattr(status, "st_mode", 0)) or bool(
        getattr(status, "st_file_attributes", 0) & reparse_flag
    )


def is_regular_unlinked_file(path: Path) -> bool:
    """Return whether *path* is a regular file with no link semantics."""
    try:
        status = path.lstat()
    except OSError:
        return False
    return stat.S_ISREG(status.st_mode) and not _status_is_reparse(status)


def is_regular_unlinked_directory(path: Path) -> bool:
    """Return whether *path* is a regular directory with no link semantics."""
    try:
        status = path.lstat()
    except OSError:
        return False
    return stat.S_ISDIR(status.st_mode) and not _status_is_reparse(status)
