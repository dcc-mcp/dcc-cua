import unittest
from pathlib import Path


WORKFLOW = (
    Path(__file__).parent.parent / ".github" / "workflows" / "release-please.yml"
)
SYNC_SCRIPT = Path(__file__).with_name("sync-cargo-workspace-version.ps1")


class ReleaseWorkflowTests(unittest.TestCase):
    def test_release_pr_is_rebuilt_from_main_without_manifest_merge_conflicts(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")

        self.assertNotIn("git merge --no-edit origin/main", workflow)
        reset = workflow.index(
            'git checkout -B $releasePr.headRefName "origin/main"'
        )
        restore = workflow.index(
            "git checkout $releaseRef -- .release-please-manifest.json CHANGELOG.md version.txt"
        )
        synchronize = workflow.index(
            "& pwsh -NoProfile -File $syncScript -Version $version"
        )
        guarded_push = workflow.index(
            'git push --force-with-lease="refs/heads/$($releasePr.headRefName):$releaseHead"'
        )

        self.assertLess(reset, restore)
        self.assertLess(restore, synchronize)
        self.assertLess(synchronize, guarded_push)

    def test_release_sync_stages_only_release_anchors_and_generated_workspace_files(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")

        self.assertIn(
            "git add .release-please-manifest.json CHANGELOG.md version.txt Cargo.toml Cargo.lock",
            workflow,
        )
        self.assertNotIn("git add Cargo.toml Cargo.lock crates/*/Cargo.toml", workflow)

    def test_workspace_sync_refreshes_the_lockfile_before_locked_validation(self):
        script = SYNC_SCRIPT.read_text(encoding="utf-8")

        refresh = script.index("cargo metadata --format-version 1 | ConvertFrom-Json")
        locked_validation = script.index(
            "cargo metadata --locked --format-version 1 --no-deps"
        )
        self.assertNotIn("cargo metadata --format-version 1 --no-deps", script)
        self.assertLess(refresh, locked_validation)


if __name__ == "__main__":
    unittest.main()
