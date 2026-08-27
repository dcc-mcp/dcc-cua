import hashlib
import importlib.util
import io
import json
import platform
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).with_name("verify_final_archive.py")
SPEC = importlib.util.spec_from_file_location("verify_final_archive", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


def host_target() -> tuple[str, str, str]:
    machine = platform.machine().lower()
    if sys.platform == "win32":
        return "x86_64-pc-windows-msvc", "zip", "dcc-cua.exe"
    if sys.platform == "darwin":
        architecture = "aarch64" if machine in {"arm64", "aarch64"} else "x86_64"
        return f"{architecture}-apple-darwin", "tar.gz", "dcc-cua"
    return "x86_64-unknown-linux-gnu", "tar.gz", "dcc-cua"


class FinalArchiveVerifierTests(unittest.TestCase):
    version = "1.2.3"

    def _source(self, root: Path) -> tuple[Path, str, str, str]:
        target, extension, binary_name = host_target()
        source = root / "source"
        source.mkdir(parents=True)
        (source / binary_name).write_bytes(b"arbitrary but self-consistent bytes")
        for name in MODULE.REQUIRED_FILES:
            (source / name).write_text(name, encoding="utf-8")
        for name in MODULE.REQUIRED_DIRECTORIES:
            path = source / name
            path.mkdir(parents=True)
            (path / "marker.txt").write_text(name, encoding="utf-8")
        return source, target, extension, binary_name

    def _entries(self, source: Path) -> dict[str, bytes]:
        return {
            path.relative_to(source).as_posix(): path.read_bytes()
            for path in sorted(source.rglob("*"))
            if path.is_file()
        }

    def _write_archive(
        self,
        archive: Path,
        extension: str,
        entries: dict[str, bytes],
        directories: tuple[str, ...] = (),
    ) -> None:
        if extension == "zip":
            with zipfile.ZipFile(archive, "w") as bundle:
                for name, content in entries.items():
                    bundle.writestr(name, content)
                for name in directories:
                    bundle.writestr(name.rstrip("/") + "/", b"")
            return
        with tarfile.open(archive, "w:gz") as bundle:
            for name, content in entries.items():
                entry = tarfile.TarInfo(name)
                entry.size = len(content)
                entry.mode = 0o755 if name == "dcc-cua" else 0o644
                bundle.addfile(entry, io.BytesIO(content))
            for name in directories:
                entry = tarfile.TarInfo(name.rstrip("/"))
                entry.type = tarfile.DIRTYPE
                entry.mode = 0o755
                bundle.addfile(entry)

    def _release(self, root: Path, entries: dict[str, bytes] | None = None) -> dict:
        source, target, extension, binary_name = self._source(root)
        archive = root / f"dcc-cua-{self.version}-{target}.{extension}"
        self._write_archive(
            archive, extension, self._entries(source) if entries is None else entries
        )
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        checksum = root / f"{archive.name}.sha256"
        checksum.write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
        manifest = root / f"dcc-cua-install-manifest-{target}.json"
        install_directories = MODULE._expected_directories(
            {
                relative: source / Path(*Path(relative).parts)
                for relative in self._entries(source)
            }
        )
        manifest.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "name": "dcc-cua",
                    "version": self.version,
                    "target": target,
                    "asset": {
                        "name": archive.name,
                        "url": f"https://github.com/dcc-mcp/dcc-cua/releases/download/v{self.version}/{archive.name}",
                        "sha256": digest,
                    },
                    "install": {
                        "directories": sorted(install_directories),
                        "files": [
                            {
                                "path": relative,
                                "sha256": hashlib.sha256(content).hexdigest(),
                            }
                            for relative, content in sorted(
                                self._entries(source).items()
                            )
                        ],
                    },
                }
            ),
            encoding="utf-8",
        )
        return {
            "source_root": source,
            "archive": archive,
            "manifest_path": manifest,
            "checksum_path": checksum,
            "target": target,
            "version": self.version,
            "extract_root": root / "extract",
            "install_root": root / "install",
            "extension": extension,
            "binary_name": binary_name,
        }

    def _verify_args(self, release: dict) -> dict:
        return {
            name: value
            for name, value in release.items()
            if name not in {"extension", "binary_name"}
        }

    def _refresh_metadata(self, release: dict) -> None:
        archive = release["archive"]
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        release["checksum_path"].write_text(
            f"{digest}  {archive.name}\n", encoding="utf-8"
        )
        manifest = json.loads(release["manifest_path"].read_text(encoding="utf-8"))
        manifest["asset"]["sha256"] = digest
        release["manifest_path"].write_text(json.dumps(manifest), encoding="utf-8")

    def test_self_consistent_arbitrary_binary_is_rejected_by_real_cli_smoke(self):
        with tempfile.TemporaryDirectory() as directory:
            release = self._release(Path(directory))
            with self.assertRaisesRegex(ValueError, "CLI smoke command"):
                MODULE.verify_final_archive(**self._verify_args(release))

    def test_missing_bundled_resource_is_rejected_before_smoke(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initial = self._release(root / "initial")
            entries = self._entries(initial["source_root"])
            entries.pop("skills/marker.txt")
            release = self._release(root / "release", entries)
            with self.assertRaisesRegex(ValueError, "archive layout mismatch"):
                MODULE.verify_final_archive(**self._verify_args(release))

    def test_extra_or_renamed_archive_entries_are_rejected(self):
        for mutation in ("extra", "renamed"):
            with (
                self.subTest(mutation=mutation),
                tempfile.TemporaryDirectory() as directory,
            ):
                root = Path(directory)
                initial = self._release(root / "initial")
                entries = self._entries(initial["source_root"])
                if mutation == "extra":
                    entries["unexpected.txt"] = b"unexpected"
                else:
                    entries["renamed-license.txt"] = entries.pop("LICENSE")
                release = self._release(root / "release", entries)
                with self.assertRaisesRegex(ValueError, "archive layout mismatch"):
                    MODULE.verify_final_archive(**self._verify_args(release))

    def test_extra_empty_directory_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            release = self._release(root)
            self._write_archive(
                release["archive"],
                release["extension"],
                self._entries(release["source_root"]),
                directories=("unexpected-empty-directory",),
            )
            self._refresh_metadata(release)
            with self.assertRaisesRegex(ValueError, "archive layout mismatch"):
                MODULE.verify_final_archive(**self._verify_args(release))

    def test_archive_content_drift_is_rejected_even_when_metadata_is_resigned(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initial = self._release(root / "initial")
            entries = self._entries(initial["source_root"])
            entries["README.md"] = b"drift"
            release = self._release(root / "release", entries)
            with self.assertRaisesRegex(ValueError, "archive package digest drift"):
                MODULE.verify_final_archive(**self._verify_args(release))

    def test_archive_mutation_during_verification_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            release = self._release(Path(directory))
            extractor_name = (
                "_extract_zip" if release["extension"] == "zip" else "_extract_tar"
            )
            original_extractor = getattr(MODULE, extractor_name)

            def mutate_after_extraction(archive, destination):
                result = original_extractor(archive, destination)
                with release["archive"].open("ab") as stream:
                    stream.write(b"post-digest mutation")
                return result

            with (
                mock.patch.object(MODULE, extractor_name, mutate_after_extraction),
                mock.patch.object(MODULE, "_smoke_binary"),
                self.assertRaisesRegex(ValueError, "changed during verification"),
            ):
                MODULE.verify_final_archive(**self._verify_args(release))

    def test_noncanonical_raw_member_names_are_rejected_for_zip_and_tar(self):
        aliases = ("./README.md", "skills//marker.txt")
        for extension, alias in (
            ("zip", aliases[0]),
            ("zip", aliases[1]),
            ("tar.gz", aliases[0]),
            ("tar.gz", aliases[1]),
        ):
            with (
                self.subTest(extension=extension, alias=alias),
                tempfile.TemporaryDirectory() as directory,
            ):
                root = Path(directory)
                archive = root / f"alias.{extension}"
                self._write_archive(archive, extension, {alias: b"alias"})
                extractor = (
                    MODULE._extract_zip if extension == "zip" else MODULE._extract_tar
                )
                with self.assertRaisesRegex(ValueError, "non-canonical path"):
                    extractor(archive, root / "extract")

    def test_unsafe_archive_path_is_rejected_without_writing_outside_root(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initial = self._release(root / "initial")
            entries = self._entries(initial["source_root"])
            entries["../escape.txt"] = b"escape"
            release = self._release(root / "release", entries)
            with self.assertRaisesRegex(ValueError, "unsafe path"):
                MODULE.verify_final_archive(**self._verify_args(release))
            self.assertFalse((root / "escape.txt").exists())

    def test_windows_device_alias_is_rejected_on_every_platform(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initial = self._release(root / "initial")
            entries = self._entries(initial["source_root"])
            entries["skills/CON"] = b"device alias"
            release = self._release(root / "release", entries)
            with self.assertRaisesRegex(ValueError, "unsafe path"):
                MODULE.verify_final_archive(**self._verify_args(release))

    def test_case_colliding_archive_paths_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initial = self._release(root / "initial")
            entries = self._entries(initial["source_root"])
            entries["readme.md"] = b"collision"
            release = self._release(root / "release", entries)
            with self.assertRaisesRegex(ValueError, "duplicate path"):
                MODULE.verify_final_archive(**self._verify_args(release))

    def test_unicode_normalization_colliding_paths_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initial = self._release(root / "initial")
            entries = self._entries(initial["source_root"])
            entries["skills/\u00e9.txt"] = b"composed"
            entries["skills/e\u0301.txt"] = b"decomposed"
            release = self._release(root / "release", entries)
            with self.assertRaisesRegex(ValueError, "duplicate path"):
                MODULE.verify_final_archive(**self._verify_args(release))

    def test_corrupt_archive_is_rejected_even_when_metadata_is_resigned(self):
        with tempfile.TemporaryDirectory() as directory:
            release = self._release(Path(directory))
            release["archive"].write_bytes(b"not an archive")
            self._refresh_metadata(release)
            with self.assertRaisesRegex(ValueError, "invalid .* archive"):
                MODULE.verify_final_archive(**self._verify_args(release))

    def test_archive_link_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            release = self._release(root)
            archive = release["archive"]
            if release["extension"] == "zip":
                entries = self._entries(release["source_root"])
                with zipfile.ZipFile(archive, "w") as bundle:
                    for name, content in entries.items():
                        bundle.writestr(name, content)
                    link = zipfile.ZipInfo("skills/link")
                    link.create_system = 3
                    link.external_attr = 0o120777 << 16
                    bundle.writestr(link, "../LICENSE")
            else:
                entries = self._entries(release["source_root"])
                with tarfile.open(archive, "w:gz") as bundle:
                    for name, content in entries.items():
                        entry = tarfile.TarInfo(name)
                        entry.size = len(content)
                        entry.mode = 0o755 if name == "dcc-cua" else 0o644
                        bundle.addfile(entry, io.BytesIO(content))
                    link = tarfile.TarInfo("skills/link")
                    link.type = tarfile.SYMTYPE
                    link.linkname = "../LICENSE"
                    bundle.addfile(link)
            self._refresh_metadata(release)
            with self.assertRaisesRegex(ValueError, "links"):
                MODULE.verify_final_archive(**self._verify_args(release))

    def test_manifest_and_checksum_must_bind_the_exact_archive(self):
        for mutation in ("manifest", "checksum"):
            with (
                self.subTest(mutation=mutation),
                tempfile.TemporaryDirectory() as directory,
            ):
                release = self._release(Path(directory))
                expected_error = "install manifest"
                if mutation == "manifest":
                    document = json.loads(
                        release["manifest_path"].read_text(encoding="utf-8")
                    )
                    document["target"] = "another-target"
                    release["manifest_path"].write_text(
                        json.dumps(document), encoding="utf-8"
                    )
                else:
                    release["checksum_path"].write_text(
                        f"{'0' * 64}  {release['archive'].name}\n",
                        encoding="utf-8",
                    )
                    expected_error = "checksum sidecar"
                with self.assertRaisesRegex(ValueError, expected_error):
                    MODULE.verify_final_archive(**self._verify_args(release))

    def test_generated_manifest_is_the_only_installation_plan(self):
        with tempfile.TemporaryDirectory() as directory:
            release = self._release(Path(directory))
            document = json.loads(release["manifest_path"].read_text(encoding="utf-8"))
            expected_files = self._entries(release["source_root"])
            document["install"]["files"] = [
                {
                    "path": relative,
                    "sha256": hashlib.sha256(content).hexdigest(),
                }
                for relative, content in sorted(expected_files.items())
                if relative != "README.md"
            ]
            release["manifest_path"].write_text(json.dumps(document), encoding="utf-8")
            with (
                mock.patch.object(MODULE, "_expected_manifest", return_value=document),
                mock.patch.object(MODULE, "_smoke_binary"),
                self.assertRaisesRegex(ValueError, "installed layout"),
            ):
                MODULE.verify_final_archive(**self._verify_args(release))


if __name__ == "__main__":
    unittest.main()
