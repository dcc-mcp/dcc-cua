import stat
import unittest
from types import SimpleNamespace
from pathlib import Path
from unittest import mock

from scripts.path_safety import (
    is_regular_unlinked_directory,
    is_regular_unlinked_file,
)


class PathSafetyTests(unittest.TestCase):
    def test_reparse_file_is_not_regular(self):
        path = mock.Mock(spec=Path)
        path.lstat.return_value = SimpleNamespace(
            st_mode=stat.S_IFREG,
            st_file_attributes=0x400,
        )
        with mock.patch.object(
            stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400, create=True
        ):
            self.assertFalse(is_regular_unlinked_file(path))

    def test_reparse_directory_is_not_regular(self):
        path = mock.Mock(spec=Path)
        path.lstat.return_value = SimpleNamespace(
            st_mode=stat.S_IFDIR,
            st_file_attributes=0x400,
        )
        with mock.patch.object(
            stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400, create=True
        ):
            self.assertFalse(is_regular_unlinked_directory(path))


if __name__ == "__main__":
    unittest.main()
