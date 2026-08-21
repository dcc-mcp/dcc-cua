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


class ExtensionContractTests(unittest.TestCase):
    def test_release_please_uses_independent_components(self) -> None:
        config = json.loads((REPOSITORY_ROOT / "release-please-config.json").read_text(encoding="utf-8"))
        versions = json.loads(
            (REPOSITORY_ROOT / ".release-please-manifest.json").read_text(encoding="utf-8")
        )
        self.assertIs(config["separate-pull-requests"], True)
        root_component = config["packages"]["."]
        extension = config["packages"]["browser-extension/chrome"]
        self.assertIn("browser-extension/**", root_component["exclude-paths"])
        self.assertEqual("node", extension["release-type"])
        self.assertEqual("dcc-cua-chrome", extension["component"])
        self.assertIs(extension["include-component-in-tag"], True)
        extra_versions = {
            item["path"]: item["jsonpath"] for item in extension["extra-files"]
        }
        self.assertEqual(
            {
                "browser-extension/chrome/component-manifest.json": "$.version",
                "browser-extension/chrome/manifest.json": "$.version",
            },
            extra_versions,
        )
        self.assertEqual("0.0.0", versions["browser-extension/chrome"])

    def test_release_workflow_routes_each_component_independently(self) -> None:
        workflow = (REPOSITORY_ROOT / ".github" / "workflows" / "release-please.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("browser-extension/chrome--release_created", workflow)
        self.assertIn("browser-extension/chrome--tag_name", workflow)
        self.assertIn("package-browser-extension:", workflow)
        self.assertIn("attach-browser-extension-assets:", workflow)
        self.assertIn(
            "release-please--branches--main--components--dcc-cua'",
            workflow,
        )

    def test_manifest_is_minimal_manifest_v3(self) -> None:
        manifest = json.loads((ROOT / "manifest.json").read_text(encoding="utf-8"))
        self.assertEqual(3, manifest["manifest_version"])
        self.assertEqual(
            {"activeTab", "nativeMessaging", "scripting", "storage"},
            set(manifest["permissions"]),
        )
        self.assertNotIn("host_permissions", manifest)
        self.assertEqual("service-worker.js", manifest["background"]["service_worker"])
        self.assertEqual("module", manifest["background"]["type"])

    def test_release_versions_are_single_sourced(self) -> None:
        package = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))
        manifest = json.loads((ROOT / "manifest.json").read_text(encoding="utf-8"))
        component = json.loads((ROOT / "component-manifest.json").read_text(encoding="utf-8"))
        self.assertEqual(package["version"], manifest["version"])
        self.assertEqual(package["version"], component["version"])
        self.assertRegex(package["version"], r"^\d+\.\d+\.\d+$")

    def test_native_host_template_is_exact_origin_only(self) -> None:
        template = json.loads(
            (ROOT / "native-host-manifest.template.json").read_text(encoding="utf-8")
        )
        self.assertEqual("com.dcc_mcp.dcc_cua", template["name"])
        self.assertEqual(["chrome-extension://__DCC_CUA_CHROME_EXTENSION_ID__/"], template["allowed_origins"])
        self.assertNotIn("*", json.dumps(template))

    def test_scripts_do_not_request_remote_code_or_blanket_hosts(self) -> None:
        source = "\n".join(
            (ROOT / name).read_text(encoding="utf-8")
            for name in ("protocol.js", "service-worker.js", "content-script.js")
        )
        for forbidden in ("<all_urls>", "eval(", "new Function(", "executeScript({code"):
            self.assertNotIn(forbidden, source)
        service_worker = (ROOT / "service-worker.js").read_text(encoding="utf-8")
        self.assertIn("requestQueue = requestQueue", service_worker)
        self.assertIn(".then(() => handleNativeMessage(message))", service_worker)

    def test_archive_is_deterministic_and_has_exact_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            first = Path(directory) / "first.zip"
            second = Path(directory) / "second.zip"
            first_hash = PACKAGE_EXTENSION.build_archive(ROOT, first)
            second_hash = PACKAGE_EXTENSION.build_archive(ROOT, second)
            self.assertEqual(first_hash, second_hash)
            with zipfile.ZipFile(first) as archive:
                self.assertEqual(list(PACKAGE_EXTENSION.ARCHIVE_FILES), archive.namelist())
            sidecar = first.with_name(f"{first.name}.sha256").read_text(encoding="utf-8")
            self.assertEqual(f"{first_hash}  {first.name}\n", sidecar)


if __name__ == "__main__":
    unittest.main(verbosity=2)
