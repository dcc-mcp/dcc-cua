import unittest
from pathlib import Path


WORKFLOW = (
    Path(__file__).parent.parent / ".github" / "workflows" / "release-please.yml"
)
SYNC_SCRIPT = Path(__file__).with_name("sync-cargo-workspace-version.ps1")
REFRESH_SCRIPT = Path(__file__).with_name("refresh-release-please-prs.ps1")


class ReleaseWorkflowTests(unittest.TestCase):
    def test_release_archive_includes_the_plugin_and_mcp_bridge(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")

        self.assertIn("cp -R .claude-plugin target/release/.claude-plugin", workflow)
        self.assertIn("cp -R .codex-plugin target/release/.codex-plugin", workflow)
        self.assertIn("cp .mcp.json target/release/.mcp.json", workflow)
        self.assertIn(
            "assets skills .claude-plugin .codex-plugin .mcp.json", workflow
        )

    def test_browser_store_jobs_require_the_protected_user_authorization_environment(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")

        for job in (
            "publish-chrome-web-store:",
            "publish-edge-addons:",
            "publish-firefox-addons:",
        ):
            start = workflow.index(job)
            section = workflow[start : start + 2200]
            self.assertIn("environment: browser-stores", section)
            self.assertIn("DCC_CUA_BROWSER_STORE_PUBLISH_READY == 'true'", section)
        self.assertIn("id-token: write", workflow)
        self.assertIn("google-github-actions/auth@v3", workflow)
        self.assertIn("FIREFOX_JWT_ISSUER: ${{ secrets.FIREFOX_AMO_API_KEY }}", workflow)
        self.assertIn("FIREFOX_JWT_SECRET: ${{ secrets.FIREFOX_AMO_API_SECRET }}", workflow)
        self.assertIn("FIREFOX_SOURCES_ZIP:", workflow)
        self.assertNotIn("--api-key ${{ secrets.", workflow)
        self.assertNotIn("--api-secret ${{ secrets.", workflow)

    def test_release_pr_refresh_handles_both_independent_components(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        script = REFRESH_SCRIPT.read_text(encoding="utf-8")

        self.assertIn("scripts/refresh-release-please-prs.ps1", workflow)
        self.assertIn("release-please--branches--main--components--dcc-cua'", script)
        self.assertIn(
            "release-please--branches--main--components--dcc-cua-browser-extension'",
            script,
        )
        self.assertIn('git checkout -B $branch "origin/main"', script)
        self.assertIn("ManifestKey = '.'", script)
        self.assertIn("ManifestKey = 'browser-extension/chrome'", script)
        self.assertIn("$baseProperty.Value = $componentVersion", script)
        self.assertIn("--force-with-lease", script)
        self.assertNotIn("git merge", script)

    def test_release_pr_is_rebuilt_from_main_without_manifest_merge_conflicts(self):
        script = REFRESH_SCRIPT.read_text(encoding="utf-8")

        self.assertNotIn("git merge --no-edit origin/main", script)
        reset = script.index('git checkout -B $branch "origin/main"')
        merge_manifest = script.index("$baseProperty.Value = $componentVersion")
        restore = script.index("& git @restoreArguments")
        synchronize = script.index(
            "& pwsh -NoProfile -File scripts/sync-cargo-workspace-version.ps1 -Version $version"
        )
        guarded_push = script.index('git push "--force-with-lease=$lease"')

        self.assertLess(reset, restore)
        self.assertLess(reset, merge_manifest)
        self.assertLess(merge_manifest, restore)
        self.assertLess(restore, synchronize)
        self.assertLess(synchronize, guarded_push)

    def test_release_sync_stages_only_release_anchors_and_generated_workspace_files(self):
        script = REFRESH_SCRIPT.read_text(encoding="utf-8")

        self.assertIn("$stageFiles += @('Cargo.toml', 'Cargo.lock')", script)
        self.assertIn("& git add -- @stageFiles", script)
        self.assertNotIn("crates/*/Cargo.toml", script)

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
