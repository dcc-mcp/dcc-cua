"""Keep runtime, native adapters and GUI fixtures on one immutable CUA SDK."""

from pathlib import Path
import re
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[1]
SDK_PACKAGES = {
    "cua-driver-contract",
    "cua-driver-sdk",
    "cua-driver-testkit",
    "cursor-overlay",
    "platform-macos",
    "platform-windows",
}


class SdkDependencyContractTests(unittest.TestCase):
    def test_runtime_and_testkit_share_one_immutable_source(self):
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        dependencies = workspace["workspace"]["dependencies"]
        sources = set()
        for name in SDK_PACKAGES:
            with self.subTest(package=name):
                dependency = dependencies[name]
                self.assertRegex(dependency["rev"], r"^[0-9a-f]{40}$")
                self.assertNotIn("branch", dependency)
                self.assertNotIn("tag", dependency)
                sources.add((dependency["git"], dependency["rev"]))
        self.assertEqual(len(sources), 1, "CUA packages must use one source revision")
        git, revision = sources.pop()

        inherited = set()
        for member in workspace["workspace"]["members"]:
            manifest = tomllib.loads((ROOT / member / "Cargo.toml").read_text(encoding="utf-8"))
            sections = [manifest, *manifest.get("target", {}).values()]
            for section in sections:
                for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
                    for alias, dependency in section.get(kind, {}).items():
                        name = dependency.get("package", alias) if isinstance(dependency, dict) else alias
                        if name in SDK_PACKAGES:
                            with self.subTest(member=member, package=name):
                                self.assertIsInstance(dependency, dict)
                                self.assertIs(dependency.get("workspace"), True)
                                self.assertFalse({"git", "rev", "path", "version"} & dependency.keys())
                            inherited.add(name)
        self.assertEqual(inherited, SDK_PACKAGES)

        lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
        expected_source = f"git+{git}?rev={revision}#{revision}"
        resolved = set()
        for package in lock["package"]:
            source = package.get("source", "")
            if package["name"] in SDK_PACKAGES:
                self.assertEqual(source, expected_source, package["name"])
                resolved.add(package["name"])
            if re.match(r"git\+https://github\.com/(?:loonghao|trycua)/cua(?:\.git)?\?", source):
                self.assertEqual(source, expected_source, package["name"])
        self.assertEqual(resolved, SDK_PACKAGES)


if __name__ == "__main__":
    unittest.main()
