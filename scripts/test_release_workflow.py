import json
import unittest
from pathlib import Path


WORKFLOW = (
    Path(__file__).parent.parent / ".github" / "workflows" / "release-please.yml"
)
PREFLIGHT_WORKFLOW = (
    Path(__file__).parent.parent
    / ".github"
    / "workflows"
    / "browser-store-preflight.yml"
)
SYNC_SCRIPT = Path(__file__).with_name("sync-cargo-workspace-version.ps1")
REFRESH_SCRIPT = Path(__file__).with_name("refresh-release-please-prs.ps1")
ROOT = Path(__file__).parent.parent
MARKETPLACE = ROOT / ".claude-plugin" / "marketplace.json"
ROOT_PLUGIN = ROOT / ".codex-plugin" / "plugin.json"
MARKETPLACE_PLUGIN = ROOT / "plugins" / "dcc-cua-computer-use"


class ReleaseWorkflowTests(unittest.TestCase):
    def test_release_archive_includes_the_plugin_and_mcp_bridge(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")

        self.assertIn("cp -R .claude-plugin target/release/.claude-plugin", workflow)
        self.assertIn("cp -R .codex-plugin target/release/.codex-plugin", workflow)
        self.assertIn("cp .mcp.json target/release/.mcp.json", workflow)
        self.assertIn("cp -R plugins target/release/plugins", workflow)
        self.assertIn(
            "assets skills plugins .claude-plugin .codex-plugin .mcp.json", workflow
        )

    def test_marketplace_uses_a_bounded_installable_plugin_directory(self):
        marketplace = json.loads(MARKETPLACE.read_text(encoding="utf-8"))
        self.assertEqual(marketplace["name"], "dcc-cua")
        self.assertEqual(marketplace["interface"]["displayName"], "DCC-CUA")
        self.assertEqual(len(marketplace["plugins"]), 1)
        entry = marketplace["plugins"][0]
        self.assertEqual(entry["name"], "dcc-cua-computer-use")
        self.assertEqual(
            entry["source"],
            {
                "source": "local",
                "path": "./plugins/dcc-cua-computer-use",
            },
        )
        self.assertEqual(entry["policy"]["installation"], "AVAILABLE")
        self.assertEqual(entry["policy"]["authentication"], "ON_INSTALL")
        self.assertEqual(entry["category"], "Productivity")
        self.assertNotEqual(entry["source"], ".")

    def test_marketplace_plugin_is_slim_and_matches_the_root_bridge(self):
        manifest_path = MARKETPLACE_PLUGIN / ".codex-plugin" / "plugin.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        root_manifest = json.loads(ROOT_PLUGIN.read_text(encoding="utf-8"))
        self.assertEqual(MARKETPLACE_PLUGIN.name, manifest["name"])
        self.assertEqual(manifest["version"], root_manifest["version"])
        self.assertEqual(manifest["mcpServers"], "./.mcp.json")
        self.assertNotIn("skills", manifest)
        self.assertEqual(
            json.loads((MARKETPLACE_PLUGIN / ".mcp.json").read_text(encoding="utf-8")),
            json.loads((ROOT / ".mcp.json").read_text(encoding="utf-8")),
        )
        self.assertEqual(
            {path.name for path in MARKETPLACE_PLUGIN.iterdir()},
            {".codex-plugin", ".mcp.json"},
        )

    def test_browser_store_preflight_is_short_lived_and_non_mutating(self):
        workflow = PREFLIGHT_WORKFLOW.read_text(encoding="utf-8")

        self.assertIn("workflow_dispatch:", workflow)
        self.assertNotIn("push:", workflow)
        self.assertIn("environment: browser-stores", workflow)
        self.assertIn("id-token: write", workflow)
        self.assertIn("token_format: access_token", workflow)
        self.assertIn("access_token_lifetime: 300s", workflow)
        self.assertIn("create_credentials_file: false", workflow)
        self.assertNotIn("publish_browser_extension.py", workflow)
        self.assertNotIn(":upload", workflow)
        self.assertNotIn(":publish", workflow)

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
