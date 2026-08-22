from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
REPOSITORY_ROOT = ROOT.parents[1]
SPEC = importlib.util.spec_from_file_location("package_extension", ROOT / "scripts" / "package_extension.py")
assert SPEC and SPEC.loader
PACKAGE_EXTENSION = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PACKAGE_EXTENSION)


def json_file(path: Path) -> dict[str, object]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise AssertionError(f"{path} must contain a JSON object")
    return value


class ExtensionContractTests(unittest.TestCase):
    def test_release_please_uses_independent_components(self) -> None:
        config = json_file(REPOSITORY_ROOT / "release-please-config.json")
        versions = json_file(REPOSITORY_ROOT / ".release-please-manifest.json")
        self.assertIs(config["separate-pull-requests"], True)
        self.assertEqual("d009004466a62c0716a8701d23ac568158789537", config["bootstrap-sha"])
        packages = config["packages"]
        assert isinstance(packages, dict)
        root_component = packages["."]
        extension = packages["browser-extension/chrome"]
        assert isinstance(root_component, dict) and isinstance(extension, dict)
        self.assertIn("browser-extension/chrome", root_component["exclude-paths"])
        self.assertEqual("node", extension["release-type"])
        self.assertEqual("dcc-cua-browser-extension", extension["component"])
        self.assertIs(extension["include-component-in-tag"], True)
        self.assertNotIn("release-as", extension)
        self.assertEqual(
            [{"type": "json", "path": "component-manifest.json", "jsonpath": "$.version"}],
            extension["extra-files"],
        )
        self.assertRegex(str(versions["browser-extension/chrome"]), r"^\d+\.\d+\.\d+$")

    def test_release_workflow_routes_each_component_independently(self) -> None:
        workflow = (REPOSITORY_ROOT / ".github" / "workflows" / "release-please.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("browser-extension/chrome--release_created", workflow)
        self.assertIn("extension_release_created", workflow)
        self.assertIn("for browser in chrome edge firefox", workflow)
        self.assertIn("package-browser-extension:", workflow)
        self.assertIn("attach-browser-extension-assets:", workflow)

    def test_wxt_builds_minimal_mv3_for_all_targets(self) -> None:
        expected_permissions = {"activeTab", "nativeMessaging", "scripting", "storage"}
        for browser in PACKAGE_EXTENSION.SUPPORTED_BROWSERS:
            manifest = json_file(ROOT / ".output" / f"{browser}-mv3" / "manifest.json")
            self.assertEqual(3, manifest["manifest_version"], browser)
            self.assertEqual(expected_permissions, set(manifest["permissions"]), browser)
            self.assertNotIn("host_permissions", manifest, browser)
            self.assertIn("background", manifest, browser)
        firefox = json_file(ROOT / ".output" / "firefox-mv3" / "manifest.json")
        settings = firefox["browser_specific_settings"]
        assert isinstance(settings, dict)
        gecko = settings["gecko"]
        assert isinstance(gecko, dict)
        self.assertEqual("dcc-cua@dcc-mcp.org", gecko["id"])
        self.assertEqual("140.0", gecko["strict_min_version"])
        data = gecko["data_collection_permissions"]
        assert isinstance(data, dict)
        self.assertEqual(
            {"browsingActivity", "websiteContent", "websiteActivity"},
            set(data["required"]),
        )

    def test_release_versions_are_single_sourced(self) -> None:
        package = json_file(ROOT / "package.json")
        component = json_file(ROOT / "component-manifest.json")
        versions = json_file(REPOSITORY_ROOT / ".release-please-manifest.json")
        self.assertEqual(package["version"], component["version"])
        self.assertEqual(package["version"], versions["browser-extension/chrome"])
        self.assertRegex(str(package["version"]), r"^\d+\.\d+\.\d+$")
        for browser in PACKAGE_EXTENSION.SUPPORTED_BROWSERS:
            manifest = json_file(ROOT / ".output" / f"{browser}-mv3" / "manifest.json")
            self.assertEqual(package["version"], manifest["version"])

    def test_native_host_templates_allow_only_exact_store_identities(self) -> None:
        chromium = json_file(ROOT / "native-host" / "chromium.template.json")
        firefox = json_file(ROOT / "native-host" / "firefox.template.json")
        self.assertEqual("com.dcc_mcp.dcc_cua", chromium["name"])
        self.assertEqual(
            [
                "chrome-extension://__DCC_CUA_CHROME_EXTENSION_ID__/",
                "chrome-extension://__DCC_CUA_EDGE_EXTENSION_ID__/",
            ],
            chromium["allowed_origins"],
        )
        self.assertEqual(["__DCC_CUA_FIREFOX_EXTENSION_ID__"], firefox["allowed_extensions"])
        self.assertNotIn("*", json.dumps([chromium, firefox]))

    def test_source_does_not_request_remote_code_or_blanket_hosts(self) -> None:
        sources = [ROOT / "protocol.ts", *sorted((ROOT / "entrypoints").glob("*.ts"))]
        source = "\n".join(path.read_text(encoding="utf-8") for path in sources)
        for forbidden in ("<all_urls>", "eval(", "new Function(", "executeScript({code"):
            self.assertNotIn(forbidden, source)

    def test_archives_are_deterministic_and_match_wxt_outputs(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            for browser in PACKAGE_EXTENSION.SUPPORTED_BROWSERS:
                first = Path(directory) / f"{browser}-first.zip"
                second = Path(directory) / f"{browser}-second.zip"
                first_hash = PACKAGE_EXTENSION.build_archive(ROOT, first, browser)
                second_hash = PACKAGE_EXTENSION.build_archive(ROOT, second, browser)
                self.assertEqual(first_hash, second_hash, browser)
                built = ROOT / ".output" / f"{browser}-mv3"
                expected = [path.relative_to(built).as_posix() for path in PACKAGE_EXTENSION.archive_files(built)]
                with zipfile.ZipFile(first) as archive:
                    self.assertEqual(expected, archive.namelist(), browser)
                sidecar = first.with_name(f"{first.name}.sha256").read_text(encoding="utf-8")
                self.assertEqual(f"{first_hash}  {first.name}\n", sidecar)


if __name__ == "__main__":
    unittest.main(verbosity=2)
