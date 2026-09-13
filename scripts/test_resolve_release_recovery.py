"""Recovery never invents a release identity or overwrites published assets."""

import json
import subprocess
import unittest
from unittest.mock import patch

from scripts import resolve_release_recovery as recovery


class RecoveryTests(unittest.TestCase):
    def test_identity_and_empty_asset_guards(self):
        good = dict(
            tagName="v1.9.0",
            targetCommitish="a" * 40,
            assets=[],
            isDraft=False,
            isPrerelease=False,
        )
        recovery.validate_release(good, "v1.9.0", "a" * 40)
        for key, value in (
            ("targetCommitish", "b" * 40),
            ("tagName", "v1.8.3"),
            ("assets", [{}]),
            ("assets", None),
            ("isDraft", True),
            ("isPrerelease", True),
        ):
            with self.subTest(key=key), self.assertRaises(ValueError):
                recovery.validate_release({**good, key: value}, "v1.9.0", "a" * 40)

    def test_both_releases_keep_distinct_source_commits(self):
        responses = [
            json.dumps(
                dict(
                    tagName="v1.9.0",
                    targetCommitish="a" * 40,
                    assets=[],
                    isDraft=False,
                    isPrerelease=False,
                )
            ),
            "",
            "a" * 40,
            "",
            "1.9.0",
            json.dumps(
                dict(
                    tagName="dcc-cua-browser-extension-v0.2.2",
                    targetCommitish="b" * 40,
                    assets=[],
                    isDraft=False,
                    isPrerelease=False,
                )
            ),
            "",
            "b" * 40,
            "",
            '{"version":"0.2.2"}',
        ]
        with patch.object(recovery, "command", side_effect=responses) as command:
            result = recovery.resolve(
                "both",
                "dcc-mcp/dcc-cua",
                "c" * 40,
                {".": "1.9.0", "browser-extension/chrome": "0.2.2"},
            )
        self.assertEqual(result["source_sha"], "a" * 40)
        self.assertEqual(result["extension_source_sha"], "b" * 40)
        self.assertEqual(result["extension_version"], "0.2.2")
        command.assert_any_call(
            "git", "merge-base", "--is-ancestor", "a" * 40, "c" * 40
        )
        command.assert_any_call(
            "git", "merge-base", "--is-ancestor", "b" * 40, "c" * 40
        )

    def test_unrelated_source_and_wrong_tagged_version_are_rejected(self):
        metadata = json.dumps(
            dict(
                tagName="v1.9.0",
                targetCommitish="a" * 40,
                assets=[],
                isDraft=False,
                isPrerelease=False,
            )
        )
        for tail in ([subprocess.CalledProcessError(1, "git")], ["", "1.8.3"]):
            with (
                self.subTest(tail=tail),
                patch.object(
                    recovery, "command", side_effect=[metadata, "", "a" * 40, *tail]
                ),
            ):
                with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                    recovery.resolve(
                        "native", "dcc-mcp/dcc-cua", "c" * 40, {".": "1.9.0"}
                    )


if __name__ == "__main__":
    unittest.main()
