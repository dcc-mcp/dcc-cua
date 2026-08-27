import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from shutil import copyfile

SCRIPT = Path(__file__).with_name("verify_ci_source_integrity.py")


def run(*args: str, cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run([*args], cwd=cwd, text=True, capture_output=True, check=False)


class CiSourceIntegrityTests(unittest.TestCase):
    def _repository(self, root: Path) -> tuple[Path, str, str]:
        repository = root / "repository"
        repository.mkdir()
        self.assertEqual(run("git", "init", cwd=repository).returncode, 0)
        self.assertEqual(
            run("git", "config", "user.name", "CI Fixture", cwd=repository).returncode,
            0,
        )
        self.assertEqual(
            run(
                "git", "config", "user.email", "ci@example.invalid", cwd=repository
            ).returncode,
            0,
        )
        (repository / "tracked.txt").write_text("expected\n", encoding="utf-8")
        committed_script = repository / "scripts" / SCRIPT.name
        committed_script.parent.mkdir()
        copyfile(SCRIPT, committed_script)
        self.assertEqual(run("git", "add", ".", cwd=repository).returncode, 0)
        self.assertEqual(
            run("git", "commit", "-m", "expected", cwd=repository).returncode, 0
        )
        expected = run("git", "rev-parse", "HEAD", cwd=repository).stdout.strip()
        (repository / "tracked.txt").write_text("attacker\n", encoding="utf-8")
        (repository / "added.txt").write_text("attacker\n", encoding="utf-8")
        self.assertEqual(run("git", "add", ".", cwd=repository).returncode, 0)
        self.assertEqual(
            run("git", "commit", "-m", "attacker", cwd=repository).returncode, 0
        )
        attacker = run("git", "rev-parse", "HEAD", cwd=repository).stdout.strip()
        self.assertEqual(
            run("git", "checkout", "--detach", expected, cwd=repository).returncode, 0
        )
        return repository, expected, attacker

    def _verify(
        self, repository: Path, expected: str
    ) -> subprocess.CompletedProcess[str]:
        return run(
            sys.executable,
            "-B",
            str(SCRIPT),
            "--repository",
            str(repository),
            "--expected",
            expected,
            cwd=repository,
        )

    def _verify_from_selected_commit(
        self, repository: Path, expected: str
    ) -> subprocess.CompletedProcess[bytes]:
        selected_script = subprocess.run(
            [
                "git",
                "-C",
                str(repository),
                "show",
                f"{expected}:scripts/{SCRIPT.name}",
            ],
            check=False,
            capture_output=True,
        )
        self.assertEqual(selected_script.returncode, 0, selected_script.stderr)
        return subprocess.run(
            [
                sys.executable,
                "-B",
                "-",
                "--repository",
                str(repository),
                "--expected",
                expected,
            ],
            input=selected_script.stdout,
            check=False,
            capture_output=True,
        )

    def test_clean_exact_checkout_is_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            repository, expected, _ = self._repository(Path(directory))
            self.assertEqual(self._verify(repository, expected).returncode, 0)

    def test_selected_commit_verifier_rejects_a_mutable_worktree_decoy(self):
        with tempfile.TemporaryDirectory() as directory:
            repository, expected, _ = self._repository(Path(directory))
            (repository / "scripts" / SCRIPT.name).write_text(
                "print('{}')\n", encoding="utf-8"
            )
            self.assertNotEqual(
                self._verify_from_selected_commit(repository, expected).returncode,
                0,
            )

    def test_post_verification_index_and_worktree_replacements_are_rejected(self):
        for mutation in ("read-tree", "restore", "archive-extract"):
            with (
                self.subTest(mutation=mutation),
                tempfile.TemporaryDirectory() as directory,
            ):
                repository, expected, attacker = self._repository(Path(directory))
                if mutation == "read-tree":
                    result = run("git", "read-tree", attacker, cwd=repository)
                elif mutation == "restore":
                    result = run(
                        "git",
                        "restore",
                        "--source",
                        attacker,
                        "--worktree",
                        "--",
                        ".",
                        cwd=repository,
                    )
                else:
                    archive = repository.parent / "attacker.tar"
                    result = run(
                        "git",
                        "archive",
                        "--format=tar",
                        "-o",
                        str(archive),
                        attacker,
                        cwd=repository,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)
                    with tarfile.open(archive, "r:") as bundle:
                        bundle.extractall(repository, filter="data")
                self.assertEqual(result.returncode, 0, result.stderr)
                verification = self._verify(repository, expected)
                self.assertNotEqual(verification.returncode, 0, verification.stdout)


if __name__ == "__main__":
    unittest.main()
