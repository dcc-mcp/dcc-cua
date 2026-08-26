import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).parent.parent
REFRESH_SCRIPT = ROOT / "scripts" / "refresh-release-please-prs.ps1"
NATIVE_BRANCH = "release-please--branches--main--components--dcc-cua"
EXTENSION_BRANCH = (
    "release-please--branches--main--components--dcc-cua-browser-extension"
)
ANSI_ESCAPE = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
POWERSHELL_CONTINUATION_MARKER = re.compile(r"(?m)^[ \t]*\|(?=[ \t])")


def _normalize_diagnostic(value: str) -> str:
    without_ansi = ANSI_ESCAPE.sub("", value)
    without_continuation_markers = POWERSHELL_CONTINUATION_MARKER.sub("", without_ansi)
    return " ".join(without_continuation_markers.split())


def run(
    *args: str,
    cwd: Path,
    env: dict[str, str] | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        args,
        cwd=cwd,
        env=env,
        check=False,
        text=True,
        capture_output=True,
    )
    if check and result.returncode != 0:
        raise AssertionError(
            f"command failed ({result.returncode}): {args}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


class RefreshReleasePleasePrsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary_directory.cleanup)
        self.root = Path(self.temporary_directory.name)
        self.origin = self.root / "origin.git"
        self.seed = self.root / "seed"
        self.checkout = self.root / "checkout"
        self.fake_bin = self.root / "fake-bin"
        self.fake_bin.mkdir()
        self.gh_log = self.root / "gh.log"
        self.git_trace = self.root / "git-trace2.json"
        self._write_fake_gh()
        self.base_commit, self.release_head = self._create_origin()

    def _git(
        self, cwd: Path, *args: str, check: bool = True
    ) -> subprocess.CompletedProcess[str]:
        return run("git", *args, cwd=cwd, check=check)

    def _write_fake_gh(self) -> None:
        fake_gh = self.fake_bin / "fake_gh.py"
        fake_gh.write_text(
            """#!/usr/bin/env python3
import json
import os
import sys

with open(os.environ["FAKE_GH_LOG"], "a", encoding="utf-8") as log:
    log.write(json.dumps(sys.argv[1:]) + "\\n")

if sys.argv[1:3] == ["pr", "list"]:
    print(os.environ["FAKE_GH_PULL_REQUESTS"])
    raise SystemExit(0)
if sys.argv[1:3] == ["workflow", "run"]:
    raise SystemExit(0)
print(f"unexpected gh arguments: {sys.argv[1:]}", file=sys.stderr)
raise SystemExit(64)
""",
            encoding="utf-8",
        )
        fake_gh.chmod(fake_gh.stat().st_mode | stat.S_IXUSR)
        if os.name == "nt":
            (self.fake_bin / "gh.cmd").write_text(
                f'@"{sys.executable}" "{fake_gh}" %*\r\n', encoding="utf-8"
            )
        else:
            shutil.copy2(fake_gh, self.fake_bin / "gh")
            (self.fake_bin / "gh").chmod(
                (self.fake_bin / "gh").stat().st_mode | stat.S_IXUSR
            )

    def _write_release_tree(self, root: Path, extension_version: str) -> None:
        (root / "browser-extension" / "chrome").mkdir(parents=True, exist_ok=True)
        (root / ".release-please-manifest.json").write_text(
            json.dumps({".": "1.5.6", "browser-extension/chrome": extension_version}),
            encoding="utf-8",
        )
        (root / "CHANGELOG.md").write_text("# Changelog\n", encoding="utf-8")
        (root / "version.txt").write_text("1.5.6\n", encoding="utf-8")
        extension = root / "browser-extension" / "chrome"
        (extension / "CHANGELOG.md").write_text(
            f"# Browser extension {extension_version}\n", encoding="utf-8"
        )
        (extension / "component-manifest.json").write_text(
            json.dumps({"version": extension_version}), encoding="utf-8"
        )
        (extension / "package.json").write_text(
            json.dumps(
                {"name": "dcc-cua-browser-extension", "version": extension_version}
            ),
            encoding="utf-8",
        )
        (extension / "package-lock.json").write_text(
            json.dumps(
                {
                    "name": "dcc-cua-browser-extension",
                    "version": extension_version,
                    "lockfileVersion": 3,
                }
            ),
            encoding="utf-8",
        )

    def _create_origin(self) -> tuple[str, str]:
        self._git(
            self.root, "init", "--bare", "--initial-branch=main", str(self.origin)
        )
        self._git(self.root, "init", "--initial-branch=main", str(self.seed))
        self._git(self.seed, "config", "user.name", "Fixture Author")
        self._git(self.seed, "config", "user.email", "fixture@example.invalid")
        self._write_release_tree(self.seed, "0.2.0")
        (self.seed / "main.txt").write_text("base\n", encoding="utf-8")
        self._git(self.seed, "add", ".")
        self._git(self.seed, "commit", "-m", "test: seed main")
        base_commit = self._git(self.seed, "rev-parse", "HEAD").stdout.strip()
        self._git(self.seed, "remote", "add", "origin", self.origin.as_uri())
        self._git(self.seed, "push", "origin", "main")

        self._git(self.seed, "checkout", "-b", EXTENSION_BRANCH)
        self._write_release_tree(self.seed, "0.2.1")
        self._git(self.seed, "add", ".")
        self._git(self.seed, "commit", "-m", "chore: release browser extension")
        release_head = self._git(self.seed, "rev-parse", "HEAD").stdout.strip()
        self._git(self.seed, "push", "origin", EXTENSION_BRANCH)
        self._git(self.seed, "checkout", "main")
        return base_commit, release_head

    def _advance_main(self) -> str:
        (self.seed / "main.txt").write_text("advanced\n", encoding="utf-8")
        self._git(self.seed, "add", "main.txt")
        self._git(self.seed, "commit", "-m", "test: advance main")
        commit = self._git(self.seed, "rev-parse", "HEAD").stdout.strip()
        self._git(self.seed, "push", "origin", "main")
        return commit

    def _shallow_detached_checkout(self, commit: str) -> None:
        self._git(self.root, "init", str(self.checkout))
        self._git(self.checkout, "remote", "add", "origin", self.origin.as_uri())
        self._git(self.checkout, "fetch", "--depth", "1", "origin", commit)
        self._git(self.checkout, "checkout", "--detach", "FETCH_HEAD")
        missing_tracking_ref = self._git(
            self.checkout,
            "rev-parse",
            "--verify",
            "refs/remotes/origin/main",
            check=False,
        )
        self.assertNotEqual(missing_tracking_ref.returncode, 0)

    def _environment(self, pull_requests: list[dict[str, object]]) -> dict[str, str]:
        environment = os.environ.copy()
        environment["PATH"] = f"{self.fake_bin}{os.pathsep}{environment['PATH']}"
        environment["FAKE_GH_LOG"] = str(self.gh_log)
        environment["FAKE_GH_PULL_REQUESTS"] = json.dumps(pull_requests)
        environment["GITHUB_SHA"] = self.base_commit
        environment["GIT_TRACE2_EVENT"] = str(self.git_trace)
        return environment

    def _run_refresh(
        self, pull_requests: list[dict[str, object]]
    ) -> subprocess.CompletedProcess[str]:
        pwsh = shutil.which("pwsh")
        self.assertIsNotNone(
            pwsh, "PowerShell is required for the release refresh contract"
        )
        return run(
            pwsh,
            "-NoProfile",
            "-File",
            str(REFRESH_SCRIPT),
            "-Repository",
            "dcc-mcp/dcc-cua",
            cwd=self.checkout,
            env=self._environment(pull_requests),
            check=False,
        )

    def _release_pull_request(self) -> list[dict[str, object]]:
        return [{"number": 210, "headRefName": EXTENSION_BRANCH}]

    def _remote_release_head(self) -> str:
        return self._git(
            self.root, "--git-dir", str(self.origin), "rev-parse", EXTENSION_BRANCH
        ).stdout.strip()

    def _traced_git_commands(self) -> list[list[str]]:
        return [
            event["argv"]
            for line in self.git_trace.read_text(encoding="utf-8").splitlines()
            if (event := json.loads(line)).get("event") == "start"
        ]

    def test_shallow_existing_release_branch_fetches_and_binds_exact_main(self) -> None:
        self._shallow_detached_checkout(self.base_commit)

        result = self._run_refresh(self._release_pull_request())

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        refreshed_head = self._remote_release_head()
        self.assertNotEqual(refreshed_head, self.release_head)
        self.assertEqual(
            self._git(
                self.root,
                "--git-dir",
                str(self.origin),
                "rev-parse",
                f"{refreshed_head}^",
            ).stdout.strip(),
            self.base_commit,
        )
        refreshed_manifest = json.loads(
            self._git(
                self.root,
                "--git-dir",
                str(self.origin),
                "show",
                f"{refreshed_head}:.release-please-manifest.json",
            ).stdout
        )
        self.assertEqual(
            refreshed_manifest,
            {".": "1.5.6", "browser-extension/chrome": "0.2.1"},
        )
        self.assertEqual(
            [
                json.loads(line)
                for line in self.gh_log.read_text(encoding="utf-8").splitlines()
            ],
            [
                [
                    "pr",
                    "list",
                    "--repo",
                    "dcc-mcp/dcc-cua",
                    "--state",
                    "open",
                    "--json",
                    "number,headRefName",
                    "--limit",
                    "20",
                ],
                [
                    "workflow",
                    "run",
                    "ci-checks.yml",
                    "--repo",
                    "dcc-mcp/dcc-cua",
                    "--ref",
                    EXTENSION_BRANCH,
                ],
            ],
        )

    def test_no_release_pr_is_a_noop_without_fetch_or_push(self) -> None:
        self._shallow_detached_checkout(self.base_commit)

        result = self._run_refresh([])

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self._remote_release_head(), self.release_head)
        self.assertNotIn("workflow", self.gh_log.read_text(encoding="utf-8"))
        self.assertNotEqual(
            self._git(
                self.checkout,
                "rev-parse",
                "--verify",
                "refs/remotes/origin/main",
                check=False,
            ).returncode,
            0,
        )

    def test_mismatch_diagnostic_normalization_ignores_ansi_and_wrapping(
        self,
    ) -> None:
        wrapped = (
            f"\x1b[31;1mfetched main commit {self.release_head}\x1b[0m does not\r\n"
            f"\x1b[31;1m|\x1b[0m match expected base {self.base_commit}"
        )

        self.assertEqual(
            _normalize_diagnostic(wrapped),
            (
                f"fetched main commit {self.release_head} does not "
                f"match expected base {self.base_commit}"
            ),
        )

        interposed = (
            f"fetched main commit {self.release_head} does not\r\n"
            "unexpected diagnostic text\r\n"
            f"| match expected base {self.base_commit}"
        )
        self.assertEqual(
            _normalize_diagnostic(interposed),
            (
                f"fetched main commit {self.release_head} does not "
                "unexpected diagnostic text "
                f"match expected base {self.base_commit}"
            ),
        )

        inline_pipe = (
            f"fetched main commit {self.release_head} does not | "
            f"match expected base {self.base_commit}"
        )
        self.assertEqual(_normalize_diagnostic(inline_pipe), inline_pipe)

    def test_mismatched_fetched_main_fails_closed_before_release_mutation(self) -> None:
        self._shallow_detached_checkout(self.base_commit)
        advanced_main = self._advance_main()

        result = self._run_refresh(self._release_pull_request())

        self.assertNotEqual(result.returncode, 0)
        combined = _normalize_diagnostic(result.stdout + result.stderr)
        self.assertIn(self.base_commit, combined)
        self.assertIn(advanced_main, combined)
        self.assertIn("does not match expected base", combined)
        self.assertEqual(self._remote_release_head(), self.release_head)
        commands = self._traced_git_commands()
        release_fetches = [
            command
            for command in commands
            if len(command) > 1
            and command[1] == "fetch"
            and any(EXTENSION_BRANCH in argument for argument in command[2:])
        ]
        pushes = [
            command for command in commands if len(command) > 1 and command[1] == "push"
        ]
        self.assertEqual(release_fetches, [])
        self.assertEqual(pushes, [])

    def test_missing_remote_main_fails_closed_before_release_mutation(self) -> None:
        self._shallow_detached_checkout(self.base_commit)
        self._git(
            self.root,
            "--git-dir",
            str(self.origin),
            "config",
            "receive.denyDeleteCurrent",
            "ignore",
        )
        self._git(self.seed, "push", "origin", "--delete", "main")

        result = self._run_refresh(self._release_pull_request())

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("main fetch failed", result.stdout + result.stderr)
        self.assertEqual(self._remote_release_head(), self.release_head)


if __name__ == "__main__":
    unittest.main()
