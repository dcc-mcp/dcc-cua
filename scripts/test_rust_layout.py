"""Exercise the real Rust layout gate without compiling Rust fixtures."""

from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


GATE = Path(__file__).with_name("check-rust-layout.ps1")


class RustLayoutTests(unittest.TestCase):
    def test_async_test_attribute_order(self):
        fixtures = (
            ("plain", "#[rstest]\n#[tokio::test]", True),
            ("two_cases", "#[rstest]\n#[case(true)]\n#[case(false)]\n#[tokio::test]", True),
            ("named_case", "#[rstest]\n#[case::enabled(true)]\n#[tokio::test(flavor = \"current_thread\")]", True),
            ("missing_rstest", "#[case(true)]\n#[case(false)]\n#[tokio::test]", False),
            ("tokio_before_cases", "#[rstest]\n#[tokio::test]\n#[case(true)]\n#[case(false)]", False),
            ("case_before_rstest", "#[case(true)]\n#[rstest]\n#[tokio::test]", False),
            ("duplicate_rstest", "#[rstest]\n#[case(true)]\n#[rstest]\n#[tokio::test]", False),
            ("reversed", "#[tokio::test]\n#[rstest]\n#[case(true)]", False),
            ("unrelated_attribute", "#[rstest]\n#[ignore]\n#[tokio::test]", False),
        )
        pwsh = shutil.which("pwsh")
        self.assertIsNotNone(pwsh, "PowerShell is required by the Rust layout gate")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "scripts").mkdir()
            tests = root / "crates" / "fixture" / "src" / "tests.rs"
            tests.parent.mkdir(parents=True)
            gate = root / "scripts" / GATE.name
            shutil.copyfile(GATE, gate)
            for name, attributes, valid in fixtures:
                with self.subTest(name=name):
                    tests.write_text(
                        f"use rstest::rstest;\n\n{attributes}\nasync fn fixture() {{}}\n",
                        encoding="utf-8",
                    )
                    result = subprocess.run(
                        [pwsh, "-NoProfile", "-File", str(gate)],
                        capture_output=True,
                        text=True,
                        timeout=30,
                        check=False,
                    )
                    self.assertEqual(
                        result.returncode, 0 if valid else 1,
                        result.stdout + result.stderr,
                    )
                    if not valid:
                        self.assertIn("must place #[rstest]", result.stderr)


if __name__ == "__main__":
    unittest.main()
